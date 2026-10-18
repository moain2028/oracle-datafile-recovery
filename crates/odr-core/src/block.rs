//! Generic Oracle block header (`kcbh`, 20 bytes), checksum and tail check.
//!
//! Layout (offsets in bytes, multi-byte fields in file byte order):
//!
//! | off | size | field       | meaning                                         |
//! |-----|------|-------------|-------------------------------------------------|
//! | 0   | 1    | `type_kcbh` | block type (see [`BlockType`])                  |
//! | 1   | 1    | `frmt_kcbh` | block format (low nibble `0x2` for 8i+)         |
//! | 2   | 2    | spare       |                                                 |
//! | 4   | 4    | `rdba_kcbh` | relative data block address                     |
//! | 8   | 4    | `bas_kcbh`  | SCN base                                        |
//! | 12  | 2    | `wrp_kcbh`  | SCN wrap                                        |
//! | 14  | 1    | `seq_kcbh`  | sequence within SCN                             |
//! | 15  | 1    | `flg_kcbh`  | flags (`0x04` = checksum present)               |
//! | 16  | 2    | `chkval`    | checksum: XOR of all 16-bit words of the block  |
//! | 18  | 2    | spare       |                                                 |
//!
//! The last 4 bytes of every block (`tailchk`) repeat
//! `(scn_base & 0xFFFF) << 16 | type << 8 | seq` in file byte order.
//!
//! Sources: Stawiarski (2018) \[W3\]; Dyke, block types \[W2\]; Oracle Concepts \[O1\].
//! Every constant here must be confirmed against lab-generated files (M0) —
//! entries marked `lab-verify` are not yet confirmed.

use crate::bytes::{ByteOrder, read_u8, read_u16, read_u32};
use crate::error::{DecodeError, DecodeResult};
use serde::{Deserialize, Serialize};

/// Size of `kcbh`.
pub const KCBH_LEN: usize = 20;
/// Size of the tail check.
pub const TAIL_LEN: usize = 4;
/// Flag bit: checksum is present and must verify.
pub const FLG_CHECKSUM: u8 = 0x04;
/// Valid Oracle block sizes.
pub const BLOCK_SIZES: [usize; 5] = [2048, 4096, 8192, 16384, 32768];

/// Block type (`type_kcbh`). Unknown values are preserved, never rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BlockType {
    /// 0x02 — undo block.
    Undo,
    /// 0x06 — transactional data block (table / index / cluster).
    Data,
    /// 0x0B — datafile header (`kcvfh`).
    FileHeader,
    /// 0x0E — undo segment header. `lab-verify`
    UndoHeader,
    /// 0x10 — segment header (freelist-managed). `lab-verify`
    SegmentHeader,
    /// 0x1D — locally-managed file space header. `lab-verify`
    FileSpaceHeader,
    /// 0x1E — LMT space bitmap block. `lab-verify`
    SpaceBitmap,
    /// 0x20 / 0x21 / 0x22 — ASSM level-1/2/3 bitmap block. `lab-verify`
    AssmBitmap(u8),
    /// 0x23 — ASSM segment header (pagetable). `lab-verify`
    AssmSegmentHeader,
    /// Any other value.
    Other(u8),
}

impl From<u8> for BlockType {
    fn from(v: u8) -> Self {
        match v {
            0x02 => Self::Undo,
            0x06 => Self::Data,
            0x0B => Self::FileHeader,
            0x0E => Self::UndoHeader,
            0x10 => Self::SegmentHeader,
            0x1D => Self::FileSpaceHeader,
            0x1E => Self::SpaceBitmap,
            0x20..=0x22 => Self::AssmBitmap(v),
            0x23 => Self::AssmSegmentHeader,
            other => Self::Other(other),
        }
    }
}

/// How the 32 bits of an [`Rdba`] are split. The scheme is a property of the
/// **tablespace**, not of the block, and nothing inside a data block says
/// which one applies — the caller must know (from `kcvfh`, `TS$.FLAGS` or
/// the operator) and must never derive a file number from a bigfile rdba
/// (`lab-verify` LV-031).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RdbaScheme {
    /// Smallfile tablespace: 10-bit relative file number, 22-bit block number.
    Smallfile,
    /// Bigfile tablespace (one file; the default for `SYSTEM`/`SYSAUX`/`USERS`
    /// since 23ai): all 32 bits are the block number.
    Bigfile,
}

/// Relative data block address. See [`RdbaScheme`] for how to split it; the
/// `file`/`block` accessors are the smallfile interpretation and exist for
/// the many Oracle structures that are smallfile-only in practice (the
/// `SYSTEM` tablespace of every release up to 21c, bootstrap `EXTENTS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Rdba(pub u32);

impl Rdba {
    /// Build a smallfile rdba from a relative file number and a block number.
    /// Returns `None` if either does not fit its field (10 / 22 bits).
    pub const fn smallfile(file: u32, block: u32) -> Option<Self> {
        if file > 0x3FF || block > 0x003F_FFFF {
            return None;
        }
        Some(Self((file << 22) | block))
    }
    /// Relative file number (smallfile interpretation).
    pub const fn file(self) -> u32 {
        self.0 >> 22
    }
    /// Block number (smallfile interpretation).
    pub const fn block(self) -> u32 {
        self.0 & 0x003F_FFFF
    }
    /// `(relative file, block)` under `scheme`. For bigfile the relative file
    /// is `None`: a bigfile tablespace has exactly one file and the rdba does
    /// not name it.
    pub const fn split(self, scheme: RdbaScheme) -> (Option<u32>, u32) {
        match scheme {
            RdbaScheme::Smallfile => (Some(self.file()), self.block()),
            RdbaScheme::Bigfile => (None, self.0),
        }
    }
    /// Whether this is the null address (`0x00000000`), which Oracle uses for
    /// "no block" in every pointer field.
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// System Change Number (48 bits: 16-bit wrap + 32-bit base).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Scn(pub u64);

impl Scn {
    /// Build from wrap and base.
    pub const fn new(wrap: u16, base: u32) -> Self {
        Self(((wrap as u64) << 32) | base as u64)
    }
}

/// Parsed `kcbh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Raw block type byte.
    pub type_byte: u8,
    /// Block format byte.
    pub format: u8,
    /// Relative data block address.
    pub rdba: Rdba,
    /// SCN of last change.
    pub scn: Scn,
    /// Sequence within SCN.
    pub seq: u8,
    /// Flags.
    pub flags: u8,
    /// Stored checksum value.
    pub checksum: u16,
}

impl BlockHeader {
    /// Parse `kcbh` from the start of `block`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if `block` is shorter than [`KCBH_LEN`].
    pub fn parse(block: &[u8], order: ByteOrder) -> DecodeResult<Self> {
        if block.len() < KCBH_LEN {
            return Err(DecodeError::Truncated {
                offset: 0,
                need: KCBH_LEN,
                len: block.len(),
            });
        }
        Ok(Self {
            type_byte: read_u8(block, 0)?,
            format: read_u8(block, 1)?,
            rdba: Rdba(read_u32(block, 4, order)?),
            scn: Scn::new(read_u16(block, 12, order)?, read_u32(block, 8, order)?),
            seq: read_u8(block, 14)?,
            flags: read_u8(block, 15)?,
            checksum: read_u16(block, 16, order)?,
        })
    }

    /// Typed block type.
    pub fn block_type(&self) -> BlockType {
        BlockType::from(self.type_byte)
    }

    /// Whether the checksum flag is set.
    pub const fn has_checksum(&self) -> bool {
        self.flags & FLG_CHECKSUM != 0
    }

    /// Expected tail check value for this header.
    #[allow(clippy::cast_possible_truncation)] // masking to 16 bits is intended
    pub const fn expected_tail(&self) -> u32 {
        ((self.scn.0 as u32 & 0xFFFF) << 16) | ((self.type_byte as u32) << 8) | self.seq as u32
    }
}

/// XOR of all 16-bit words of `block`. A block with a correct checksum
/// (`chkval` included) folds to `0`. Byte-order independent: folding bytes
/// pairwise yields 0 iff folding words does.
///
/// Performance: processes 8 bytes per step; the compiler auto-vectorises it.
pub fn xor_fold(block: &[u8]) -> u16 {
    let mut acc: u64 = 0;
    let (words, rem) = block.as_chunks::<8>();
    for w in words {
        acc ^= u64::from_le_bytes(*w);
    }
    let mut tail = [0u8; 8];
    if let Some(dst) = tail.get_mut(..rem.len()) {
        dst.copy_from_slice(rem);
    }
    acc ^= u64::from_le_bytes(tail);
    #[allow(clippy::cast_possible_truncation)] // folding 64 → 16 bits is intended
    let folded = (acc ^ (acc >> 16) ^ (acc >> 32) ^ (acc >> 48)) as u16;
    folded
}

/// Verdict for one block. Order of checks is fixed so results are reproducible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BlockStatus {
    /// Header, tail and (if flagged) checksum all consistent.
    Valid,
    /// Every byte is zero (never formatted, or wiped).
    Zeroed,
    /// Block shorter than the block size (end of a truncated file).
    Truncated,
    /// Format byte is not a known Oracle format (encrypted / foreign data).
    Foreign,
    /// Tail check does not match header.
    TailMismatch,
    /// Checksum flag set but XOR fold is non-zero.
    ChecksumFail,
}

/// Classify one block. Never panics, for any input.
pub fn classify(block: &[u8], block_size: usize, order: ByteOrder) -> BlockStatus {
    if block.len() < block_size || block.len() < KCBH_LEN + TAIL_LEN {
        return BlockStatus::Truncated;
    }
    if block.iter().all(|&b| b == 0) {
        return BlockStatus::Zeroed;
    }
    let Ok(h) = BlockHeader::parse(block, order) else {
        return BlockStatus::Truncated;
    };
    if h.format & 0x0F != 0x02 {
        return BlockStatus::Foreign;
    }
    let tail_off = block_size.saturating_sub(TAIL_LEN);
    match read_u32(block, tail_off, order) {
        Ok(t) if t == h.expected_tail() => {}
        _ => return BlockStatus::TailMismatch,
    }
    let Some(body) = block.get(..block_size) else {
        return BlockStatus::Truncated;
    };
    if h.has_checksum() && xor_fold(body) != 0 {
        return BlockStatus::ChecksumFail;
    }
    BlockStatus::Valid
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn rdba_split() {
        let r = Rdba((5 << 22) | 0x04D2);
        assert_eq!((r.file(), r.block()), (5, 1234));
        assert_eq!(Rdba::smallfile(5, 1234), Some(r));
        assert_eq!(Rdba::smallfile(1024, 0), None);
        assert_eq!(Rdba::smallfile(0, 1 << 22), None);
        assert_eq!(r.split(RdbaScheme::Smallfile), (Some(5), 1234));
        // A bigfile rdba never yields a file number, even when the top bits
        // happen to be non-zero (block numbers above 4M).
        assert_eq!(r.split(RdbaScheme::Bigfile), (None, (5 << 22) | 0x04D2));
        assert!(Rdba(0).is_null());
        assert!(!r.is_null());
    }

    #[test]
    fn xor_fold_matches_naive() {
        let data: Vec<u8> = (0..8192u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let naive = data
            .as_chunks::<2>()
            .0
            .iter()
            .fold(0u16, |a, c| a ^ u16::from_le_bytes(*c));
        assert_eq!(xor_fold(&data), naive);
    }

    #[test]
    fn classify_never_panics_on_short_or_zero() {
        assert_eq!(
            classify(&[], 8192, ByteOrder::Little),
            BlockStatus::Truncated
        );
        assert_eq!(
            classify(&[0u8; 8192], 8192, ByteOrder::Little),
            BlockStatus::Zeroed
        );
        assert_eq!(
            classify(&[0xFF; 8192], 8192, ByteOrder::Big),
            BlockStatus::Foreign
        );
    }
}
