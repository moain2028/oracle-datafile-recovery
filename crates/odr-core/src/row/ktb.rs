//! Transaction header of a data block (`ktbbh`) and its ITL entries.
//!
//! Layout (block offsets; multi-byte fields in file byte order) — `lab-verify` LV-003/004:
//!
//! | off    | size   | field        | meaning                                    |
//! |--------|--------|--------------|--------------------------------------------|
//! | 0x14   | 1      | `ktbbhtyp`   | 1 = data (table/cluster), 2 = index        |
//! | 0x18   | 4      | `ktbbhsid`   | `DATA_OBJECT_ID` of the owning segment     |
//! | 0x1C   | 8      | `ktbbhcsc`   | cleanout SCN (base u32, wrap u16, pad)     |
//! | 0x24   | 1      | `ktbbhict`   | number of ITL entries (**one byte**)       |
//! | 0x25   | 1      | —            | reserved/other; `0x1F` seen on 26ai blocks |
//! | 0x26   | 1      | `ktbbhflg`   | flags (`0x01` = on freelist, MSSM)         |
//! | 0x27   | 1      | `ktbbhfsl`   | ITL freelist slot                          |
//! | 0x28   | 4      | `ktbbhfnx`   | next block on freelist (rdba)              |
//! | 0x2C   | 24·itc | `ktbbhitl[]` | ITL entries                                |
//!
//! ITL entry (24 bytes): `xid` (usn u16, slot u16, sqn u32), `uba` (dba u32,
//! seq u16, rec u8, pad u8), flag+lock count (u16: high nibble flags, low 12
//! bits locked rows), `fsc`/`scn wrap` (u16), commit SCN base (u32).
//!
//! Sources: published BBED `map /v` struct listings; Lewis, *Oracle Core* \[A11\].
//! `ktbbhict` width: lab 26ai block dumps print `itc: 2` for raw bytes
//! `02 1F` at 0x24..0x26 (LV-047) — a 16-bit read would give 7938.

// Offset arithmetic below combines a bounded base (kdbh offset <= 0x2C + 24*255 + 8)
// with `u16`/`u8` header values, so every sum stays below 2^18 and cannot overflow
// `usize`; every resulting offset is then bounds-checked by the `read_*` helpers.
#![allow(clippy::arithmetic_side_effects)]

use crate::block::{KCBH_LEN, Scn};
use crate::bytes::{ByteOrder, read_u8, read_u16, read_u32};
use crate::error::{DecodeError, DecodeResult};

const WHAT: &str = "ktbbh";
/// Offset of `ktbbh` within the block (directly after `kcbh`).
pub const KTBBH_OFF: usize = KCBH_LEN;
/// Offset of the first ITL entry.
pub const ITL_OFF: usize = 0x2C;
/// Size of one ITL entry.
pub const ITL_LEN: usize = 24;
/// Upper bound for the ITL count. Oracle caps `MAXTRANS` at 255 (the
/// one-byte `ktbbhict` cannot exceed it; kept for callers' bounds).
pub const MAX_ITL: u16 = 255;

/// `ktbbhtyp` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SegmentKind {
    /// 1 — table or cluster data.
    Data,
    /// 2 — index.
    Index,
    /// Anything else (kept, not rejected).
    Other(u8),
}

/// Parsed `ktbbh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ktbbh {
    /// Segment kind.
    pub kind: SegmentKind,
    /// `DATA_OBJECT_ID` of the owning segment.
    pub objd: u32,
    /// Cleanout SCN.
    pub cleanout_scn: Scn,
    /// Number of ITL entries.
    pub itl_count: u16,
    /// Flags.
    pub flags: u8,
    /// Next block on the freelist (MSSM only).
    pub freelist_next: u32,
}

impl Ktbbh {
    /// Parse `ktbbh` from a whole block buffer.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if the block is too short for the header and
    /// all ITL entries; [`DecodeError::Malformed`] if the ITL count is absurd.
    pub fn parse(block: &[u8], order: ByteOrder) -> DecodeResult<Self> {
        let typ = read_u8(block, 0x14)?;
        let objd = read_u32(block, 0x18, order)?;
        let base = read_u32(block, 0x1C, order)?;
        let wrap = read_u16(block, 0x20, order)?;
        // LV-047: one byte. Byte 0x25 is not part of the count.
        let itl_count = u16::from(read_u8(block, 0x24)?);
        if itl_count == 0 {
            return Err(DecodeError::malformed(WHAT, 0x24, "ITL count is zero"));
        }
        let h = Self {
            kind: match typ {
                1 => SegmentKind::Data,
                2 => SegmentKind::Index,
                o => SegmentKind::Other(o),
            },
            objd,
            cleanout_scn: Scn::new(wrap, base),
            itl_count,
            flags: read_u8(block, 0x26)?,
            freelist_next: read_u32(block, 0x28, order)?,
        };
        // The whole ITL array must fit in the block.
        let end = h.itl_end();
        if end > block.len() {
            return Err(DecodeError::Truncated {
                offset: ITL_OFF,
                need: end - ITL_OFF,
                len: block.len(),
            });
        }
        Ok(h)
    }

    /// Offset just past the last ITL entry (where `kdbh` begins on MSSM blocks).
    pub fn itl_end(&self) -> usize {
        // itl_count <= 255, so this cannot overflow.
        ITL_OFF + ITL_LEN * usize::from(self.itl_count)
    }

    /// Parse ITL entry `i` (0-based).
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if `i` is out of range or the block is short.
    pub fn itl(&self, block: &[u8], order: ByteOrder, i: u16) -> DecodeResult<Itl> {
        if i >= self.itl_count {
            return Err(DecodeError::malformed(
                WHAT,
                ITL_OFF,
                "ITL index out of range",
            ));
        }
        let o = ITL_OFF + ITL_LEN * usize::from(i);
        let flag_lck = read_u16(block, o + 16, order)?;
        Ok(Itl {
            xid: (
                read_u16(block, o, order)?,
                read_u16(block, o + 2, order)?,
                read_u32(block, o + 4, order)?,
            ),
            uba: (
                read_u32(block, o + 8, order)?,
                read_u16(block, o + 12, order)?,
                read_u8(block, o + 14)?,
            ),
            flags: u8::try_from(flag_lck >> 12).unwrap_or(0),
            locked_rows: flag_lck & 0x0FFF,
            scn: Scn::new(
                read_u16(block, o + 18, order)?,
                read_u32(block, o + 20, order)?,
            ),
        })
    }
}

/// One Interested Transaction List entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Itl {
    /// Transaction id: (undo segment, slot, sequence).
    pub xid: (u16, u16, u32),
    /// Undo block address: (dba, sequence, record).
    pub uba: (u32, u16, u8),
    /// Flag nibble (C = committed, B, U, T — see Lewis \[A11\]).
    pub flags: u8,
    /// Number of rows locked by this transaction.
    pub locked_rows: u16,
    /// Commit SCN (or free-space credit when uncommitted).
    pub scn: Scn,
}

/// `DATA_OBJECT_ID` of a data or index block — the key Astra's scanner uses to
/// group blocks into segments. Stable contract (ADR-0006).
///
/// # Errors
/// As [`Ktbbh::parse`].
pub fn data_object_id(block: &[u8], order: ByteOrder) -> DecodeResult<u32> {
    Ktbbh::parse(block, order).map(|h| h.objd)
}
