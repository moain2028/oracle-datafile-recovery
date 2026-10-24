//! Out-of-line LOB data (M5 part 2, lab-verify LV-070).
//!
//! When a LOB does not fit in the row, its locator (see
//! [`crate::types::lob`]) carries the addresses of the blocks holding the
//! data. This module fetches them through a [`BlockSource`], checks every
//! block (integrity, type, LOB id) and assembles the bytes — or says exactly
//! why it could not.
//!
//! Chunk block layouts **measured on 26ai** (`T_LOB_BASIC` / `T_LOB_SECURE`,
//! golden `20261024T085903Z`, compared with `DBMS_LOB.SUBSTR`):
//!
//! ```text
//! BasicFile chunk block, type 0x28 (`kdlb`):
//!   kcbh (20)   | 0x14: 00 00 00 01 (?) | 0x18: ..
//!   0x18..0x22  LOB id (10 bytes)  — must equal the locator's lobid
//!   0x28        chunk sequence u32 BE (0 for the first block, 1 for the next…)
//!   0x30 ..     01 0f 00 04 00 00 00 00 ??
//!   0x38        data starts here; runs to the end of the chunk payload.
//!               The last block holds (total − previous blocks) bytes; the
//!               rest of the block is padding (lab: 00 20 …), so the data
//!               length must come from the LOB total length, not the block.
//!
//! SecureFile data block, type 0x06 with `ktbbh` + a 24-byte ITL + a LOB
//! header (`kdli`): LOB id at 0x54..0x5E, data starts at 0x80. The data
//! length of the whole LOB is in the locator (`bytes`); each block carries
//! `min(remaining, 8192 − 0x80 − 4)` bytes.
//! ```
//!
//! Rules: a chunk block that fails `classify`, is not of the expected type,
//! or carries another LOB id ⇒ [`LobReadError`], never a partial value.
//! Both the `BasicFile` total (not in the locator) and the `SecureFile` total
//! (in the locator) must be consistent with the chunk map or the LOB is
//! refused.

use super::source::{BlockAddr, BlockSource};
use crate::block::{BlockStatus, Rdba, classify};
use crate::types::lob::{LobData, LobLocator, LobStorage};
use serde::Serialize;

/// Where LOB data starts inside a `BasicFile` chunk block (type 0x28).
pub const BASIC_DATA_OFF: usize = 0x38;
/// LOB id position inside a `BasicFile` chunk block.
pub const BASIC_LOBID_OFF: usize = 0x18;
/// Where LOB data starts inside a `SecureFile` data block (type 0x06, kdli).
pub const SECURE_DATA_OFF: usize = 0x80;
/// LOB id position inside a `SecureFile` data block.
pub const SECURE_LOBID_OFF: usize = 0x54;

/// Why an out-of-line LOB could not be assembled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum LobReadError {
    /// The locator is inline — nothing to read out of line.
    NotOutOfLine,
    /// The locator carries no chunk address.
    NoChunks,
    /// A chunk block could not be resolved or read.
    Unavailable {
        /// Rdba as written in the locator.
        rdba: u32,
    },
    /// A chunk block failed its integrity check (LV-059 for LOBs).
    Integrity {
        /// Rdba.
        rdba: u32,
        /// Status.
        status: BlockStatus,
    },
    /// A chunk block is not of the expected type.
    WrongType {
        /// Rdba.
        rdba: u32,
        /// Block type byte found.
        found: u8,
    },
    /// The chunk block belongs to another LOB.
    ForeignLob {
        /// Rdba.
        rdba: u32,
    },
    /// The data length declared by the locator exceeds what the chunks hold.
    LengthMismatch {
        /// Bytes the locator declares.
        declared: u64,
        /// Bytes the chunks can hold.
        available: u64,
    },
    /// `BasicFile`: the locator does not say how long the LOB is and the
    /// chunk padding cannot be told from data — refused rather than
    /// guessed (declared limit; the LOB index would give the length).
    UnknownLength,
}

/// Assembled out-of-line LOB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobBytes {
    /// Data bytes (CLOB: UCS-2 in a Unicode database — decode with the
    /// same rule as inline data).
    pub data: Vec<u8>,
    /// Blocks read, in order.
    pub blocks: Vec<u32>,
    /// Worst integrity status seen (always `Valid` on success).
    pub status: BlockStatus,
}

/// Read the out-of-line data of `locator`. `from_file` is the file the row
/// (and its rdbas) live in. `total_len` is the LOB length in **bytes** when
/// known from outside the locator (`BasicFile` has none inside); `None` uses
/// the locator's own declared length (`SecureFile`).
///
/// # Errors
/// See [`LobReadError`]. On any error nothing is returned — a LOB is
/// either complete and from clean blocks, or absent with a reason.
pub fn read_out_of_line<S: BlockSource + ?Sized>(
    src: &S,
    from_file: u32,
    locator: &LobLocator<'_>,
    total_len: Option<u64>,
) -> Result<LobBytes, LobReadError> {
    let LobData::OutOfLine { bytes, chunks } = &locator.data else {
        return Err(LobReadError::NotOutOfLine);
    };
    if chunks.is_empty() {
        return Err(LobReadError::NoChunks);
    }
    let Some(declared) = total_len.or(*bytes) else {
        return Err(LobReadError::UnknownLength);
    };
    let order = src.order();
    let (expect_type, data_off, lobid_off) = match locator.storage {
        LobStorage::BasicFile => (0x28u8, BASIC_DATA_OFF, BASIC_LOBID_OFF),
        LobStorage::SecureFile => (0x06u8, SECURE_DATA_OFF, SECURE_LOBID_OFF),
    };
    let mut data: Vec<u8> = Vec::new();
    let mut blocks = Vec::new();
    let mut remaining = declared;
    'chunks: for &(first, nblk) in chunks {
        for i in 0..nblk.max(1) {
            if remaining == 0 {
                break 'chunks;
            }
            let rdba = first.saturating_add(i);
            let addr: BlockAddr = src
                .resolve(Rdba(rdba), from_file)
                .ok_or(LobReadError::Unavailable { rdba })?;
            let block = src.block(addr).ok_or(LobReadError::Unavailable { rdba })?;
            let bs = block.len();
            let status = classify(&block, bs, order);
            if status != BlockStatus::Valid {
                return Err(LobReadError::Integrity { rdba, status });
            }
            let found = block.first().copied().unwrap_or(0);
            if found != expect_type {
                return Err(LobReadError::WrongType { rdba, found });
            }
            let id = block
                .get(lobid_off..lobid_off.saturating_add(10))
                .ok_or(LobReadError::WrongType { rdba, found })?;
            if id != locator.lobid {
                return Err(LobReadError::ForeignLob { rdba });
            }
            let payload_max = bs
                .saturating_sub(crate::block::TAIL_LEN)
                .saturating_sub(data_off);
            let take = usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .min(payload_max);
            let slice = block
                .get(data_off..data_off.saturating_add(take))
                .ok_or(LobReadError::WrongType { rdba, found })?;
            data.extend_from_slice(slice);
            blocks.push(rdba);
            remaining = remaining.saturating_sub(u64::try_from(take).unwrap_or(u64::MAX));
        }
    }
    if remaining != 0 {
        return Err(LobReadError::LengthMismatch {
            declared,
            available: declared.saturating_sub(remaining),
        });
    }
    Ok(LobBytes {
        data,
        blocks,
        status: BlockStatus::Valid,
    })
}
