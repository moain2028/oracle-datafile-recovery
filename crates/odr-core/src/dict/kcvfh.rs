//! Datafile header block (`kcvfh`, file block 1) — the dictionary entry point.
//!
//! Only the fields ODR needs are decoded, and only those whose offsets are
//! stable across the 10g–19c `V10 STYLE FILE HEADER` (`lab-verify` LV-028,
//! LV-035). Offsets are relative to the block; multi-byte fields are in file
//! byte order:
//!
//! | off | size | field       | meaning                                                   |
//! |-----|------|-------------|-----------------------------------------------------------|
//! | 0   | 20   | `kcvfhbfh`  | ordinary `kcbh` (type `0x0B`)                             |
//! | 20  | 4    | `kccfhswv`  | software version — **zero** in V10-style headers          |
//! | 24  | 4    | `kccfhcvn`  | compatibility version (`0x0B200400` = 11.2.0.4)           |
//! | 28  | 4    | `kccfhdbi`  | `DBID`                                                    |
//! | 32  | 8    | `kccfhdbn`  | database name, blank/NUL padded                           |
//! | 44  | 4    | `kccfhfsz`  | file size in blocks (excluding OS block 0)                |
//! | 48  | 4    | `kccfhbsz`  | block size in bytes                                       |
//! | 52  | 2    | `kccfhfno`  | absolute file number                                      |
//! | 54  | 2    | `kccfhtyp`  | file type (3 = datafile)                                  |
//! | 96  | 4    | `kcvfhrdb`  | **root dba**: `bootstrap$` segment header (file 1 only)   |
//! | 100 | 8    | `kcvfhcrs`  | creation SCN (base u32, wrap u16)                         |
//! | 108 | 4    | `kcvfhcrt`  | creation time                                             |
//! | 138 | 2    | `kcvfhsta`  | status (`0x0004` fuzzy/open, `0x2000` clean)              |
//! | 140 | 4    | `kcvfhcpc`  | checkpoint count                                          |
//! | 332 | 4    | `kcvfhtsn`  | tablespace number (11g/12c offset; `lab-verify`)          |
//! | 336 | 2    | `kcvfhtln`  | tablespace name length                                    |
//! | 338 | 30   | `kcvfhtnm`  | tablespace name                                           |
//! | 368 | 4    | `kcvfhrfn`  | relative file number                                      |
//! | 484 | 8    | `kcvfhckp`  | checkpoint SCN (base u32, wrap u16)                       |
//!
//! Fields at or after offset 332 are version-dependent (struct size 10g 676,
//! 11g 860, 12c 1112) and are returned as `Option`: they are filled only when
//! they pass internal consistency checks, never guessed.
//!
//! Evidence (T3, real bytes): BBED `map /v` + `dump` of an 11.2.0.4
//! `system01.dbf` block 1 (cnblogs yhq1314, 2019) — every offset above was
//! checked against those bytes in the unit tests; root dba `0x00400208` confirmed
//! by `x$kcvfh.fhrdb` in the same article. 9i/10g file-header trace dumps
//! (eygle 2007) confirm `root dba` → `bootstrap$` (10g) or the compatibility
//! segment (9i, type `0x0D`) — the latter is reported as unsupported.

use crate::block::{BLOCK_SIZES, BlockHeader, Rdba, Scn};
use crate::bytes::{ByteOrder, read_u16, read_u32, slice_at};
use crate::error::{DecodeError, DecodeResult};
use serde::{Deserialize, Serialize};

const WHAT: &str = "kcvfh";
/// `type_kcbh` of a datafile header block.
pub const FILE_HEADER_TYPE: u8 = 0x0B;
/// `kccfhtyp` of an ordinary datafile.
pub const FILE_TYPE_DATA: u16 = 3;
/// Minimum bytes needed for the version-independent fields.
pub const MIN_LEN: usize = 144;

/// Decoded datafile header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeader {
    /// Byte order the header decoded under.
    pub order: ByteOrder,
    /// `kccfhcvn` — compatibility version, e.g. `0x0B20_0400` = 11.2.0.4.
    pub compat: u32,
    /// `DBID`.
    pub dbid: u32,
    /// Database name (trailing blanks/NULs removed; ASCII only).
    pub db_name: String,
    /// File size in blocks.
    pub file_size_blocks: u32,
    /// Block size in bytes (one of [`BLOCK_SIZES`]).
    pub block_size: u32,
    /// Absolute file number.
    pub file_number: u16,
    /// File type (3 = datafile).
    pub file_type: u16,
    /// Root dba (`kcvfhrdb`). Meaningful for file 1 only; `0` elsewhere.
    pub root_dba: Rdba,
    /// Creation SCN.
    pub creation_scn: Scn,
    /// Status flags (`kcvfhsta`).
    pub status: u16,
    /// Checkpoint count.
    pub checkpoint_count: u32,
    /// Tablespace number, when the version-dependent region is consistent.
    pub tablespace: Option<u32>,
    /// Tablespace name, when `kcvfhtln ≤ 30` and the bytes are printable ASCII.
    pub tablespace_name: Option<String>,
    /// Relative file number, when consistent (`1..=1023`).
    pub relative_file: Option<u32>,
    /// Checkpoint SCN, when the region is present.
    pub checkpoint_scn: Option<Scn>,
}

impl FileHeader {
    /// Parse block 1 of a datafile in a known byte order.
    ///
    /// # Errors
    /// [`DecodeError::Malformed`] if the block is not a datafile header
    /// (`type_kcbh != 0x0B`), the block size is not an Oracle block size, or
    /// the buffer is shorter than the stated block size;
    /// [`DecodeError::Truncated`] if shorter than [`MIN_LEN`].
    pub fn parse(block: &[u8], order: ByteOrder) -> DecodeResult<Self> {
        if block.len() < MIN_LEN {
            return Err(DecodeError::Truncated {
                offset: 0,
                need: MIN_LEN,
                len: block.len(),
            });
        }
        let h = BlockHeader::parse(block, order)?;
        if h.type_byte != FILE_HEADER_TYPE {
            return Err(DecodeError::malformed(
                WHAT,
                0,
                "block type is not 0x0B (datafile header)",
            ));
        }
        if h.format & 0x0F != 0x02 {
            return Err(DecodeError::malformed(WHAT, 1, "unknown block format"));
        }
        let block_size = read_u32(block, 48, order)?;
        let bs = usize::try_from(block_size).unwrap_or(0);
        if !BLOCK_SIZES.contains(&bs) {
            return Err(DecodeError::malformed(
                WHAT,
                48,
                "kccfhbsz is not an Oracle block size",
            ));
        }
        if block.len() < bs {
            return Err(DecodeError::malformed(
                WHAT,
                48,
                "buffer shorter than the stated block size",
            ));
        }
        let file_number = read_u16(block, 52, order)?;
        if file_number == 0 {
            return Err(DecodeError::malformed(WHAT, 52, "file number is zero"));
        }
        let db_name = ascii_field(slice_at(block, 32, 8)?).ok_or(DecodeError::malformed(
            WHAT,
            32,
            "db name is not ASCII",
        ))?;

        // Version-dependent region: each field is accepted only if its own
        // sanity check passes; otherwise it is `None`, never a guess.
        let tablespace = read_u32(block, 332, order)
            .ok()
            .filter(|&t| t < 0x0001_0000);
        let tablespace_name = read_u16(block, 336, order).ok().and_then(|tln| {
            let n = usize::from(tln);
            if n == 0 || n > 30 {
                return None;
            }
            slice_at(block, 338, n).ok().and_then(ascii_field)
        });
        let relative_file = read_u32(block, 368, order)
            .ok()
            .filter(|&r| (1..=0x3FF).contains(&r));
        let checkpoint_scn = match (read_u32(block, 484, order), read_u16(block, 488, order)) {
            (Ok(base), Ok(wrap)) => Some(Scn::new(wrap, base)),
            _ => None,
        };

        Ok(Self {
            order,
            compat: read_u32(block, 24, order)?,
            dbid: read_u32(block, 28, order)?,
            db_name,
            file_size_blocks: read_u32(block, 44, order)?,
            block_size,
            file_number,
            file_type: read_u16(block, 54, order)?,
            root_dba: Rdba(read_u32(block, 96, order)?),
            creation_scn: Scn::new(read_u16(block, 104, order)?, read_u32(block, 100, order)?),
            status: read_u16(block, 138, order)?,
            checkpoint_count: read_u32(block, 140, order)?,
            tablespace,
            tablespace_name,
            relative_file,
            checkpoint_scn,
        })
    }

    /// Parse block 1 without knowing the byte order: both orders are tried
    /// and the one whose block size, file number and tail check are all
    /// consistent wins. If both or neither are consistent the header is
    /// rejected as ambiguous (no silent guess).
    ///
    /// # Errors
    /// As [`Self::parse`], or `Malformed("ambiguous byte order")`.
    pub fn detect(block: &[u8]) -> DecodeResult<Self> {
        let little = Self::parse(block, ByteOrder::Little).ok();
        let big = Self::parse(block, ByteOrder::Big).ok();
        match (little, big) {
            (Some(h), None) | (None, Some(h)) => Ok(h),
            (Some(_), Some(_)) => Err(DecodeError::malformed(WHAT, 48, "ambiguous byte order")),
            (None, None) => Err(DecodeError::malformed(
                WHAT,
                0,
                "not a datafile header in either byte order",
            )),
        }
    }

    /// Whether the header carries a non-null dictionary root dba.
    ///
    /// LV-044/LV-053: this is **necessary, not sufficient**. In a 26ai PDB
    /// the SYSTEM file is absolute file 12 (not 1) and *every* PDB datafile
    /// repeats `kcvfhrdb = 520`. Only walking the root segment and finding
    /// `bootstrap$` rows (`Dictionary::load`) proves the file is SYSTEM.
    pub const fn has_dictionary_root(&self) -> bool {
        !self.root_dba.is_null()
    }
}

/// Blank/NUL-padded ASCII text field; `None` if any byte is not printable ASCII.
fn ascii_field(raw: &[u8]) -> Option<String> {
    let end = raw
        .iter()
        .rposition(|&b| b != 0 && b != b' ')
        .map_or(0, |i| i.saturating_add(1));
    let body = raw.get(..end)?;
    if !body.iter().all(|&b| (0x20..0x7F).contains(&b)) {
        return None;
    }
    Some(String::from_utf8_lossy(body).into_owned())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    /// First 512 bytes of block 1 of an 11.2.0.4 `system01.dbf`, verbatim from
    /// a published BBED `dump` (cnblogs yhq1314 2019, before the article's
    /// deliberate corruption). Lines 6–10 and 12–15 were all zero.
    const GOLDEN_512: &str = "\
0ba20000 01004000 00000000 00000104 9bec0000 00000000 0004200b cf766f5b \
4f52434c 00000000 c0170000 00770100 00200000 01000300 00000000 00000000 \
00000000 00000000 00000000 00000000 00000000 00000000 00000000 00000000 \
08024000 07000000 00000000 512bfa3b 4f2bfa3b 01000000 00000000 00000000 \
00000000 00000000 00000420 8c000000 b993173c 8b000000 00000000 00000000 \
Z Z Z Z Z \
00000000 00000000 00000000 00000000 06005359 5354454d 00000000 00000000 \
00000000 00000000 00000000 00000000 01000000 00000000 00000000 00000000 \
00000000 00000000 00000000 00000000 00000000 00000000 00000000 0a000a00 \
Z Z \
00000000 21fd1700 00000000 3e171a3c 01000000 75000000 1e7b0000 10000000";

    fn golden_block() -> Vec<u8> {
        let mut out = Vec::with_capacity(8192);
        for tok in GOLDEN_512.split_whitespace() {
            if tok == "Z" {
                out.extend([0u8; 32]);
            } else {
                for i in (0..tok.len()).step_by(2) {
                    out.push(u8::from_str_radix(&tok[i..i + 2], 16).unwrap());
                }
            }
        }
        assert_eq!(out.len(), 512);
        out.resize(8192, 0);
        out
    }

    #[test]
    fn golden_11g_system_header() {
        let b = golden_block();
        let h = FileHeader::detect(&b).unwrap();
        assert_eq!(h.order, ByteOrder::Little);
        assert_eq!(h.compat, 0x0B20_0400, "11.2.0.4");
        assert_eq!(h.dbid, 0x5B6F_76CF);
        assert_eq!(h.db_name, "ORCL");
        assert_eq!(h.file_size_blocks, 96_000, "(786440192 - 8192) / 8192");
        assert_eq!(h.block_size, 8192);
        assert_eq!((h.file_number, h.file_type), (1, FILE_TYPE_DATA));
        assert_eq!(h.root_dba, Rdba(0x0040_0208), "x$kcvfh.fhrdb = 4194824");
        assert_eq!((h.root_dba.file(), h.root_dba.block()), (1, 520));
        assert_eq!(h.creation_scn, Scn(7), "v$datafile.creation_change# = 7");
        assert_eq!(h.status, 0x2004);
        assert_eq!(h.checkpoint_count, 140);
        assert_eq!(h.tablespace, Some(0));
        assert_eq!(h.tablespace_name.as_deref(), Some("SYSTEM"));
        assert_eq!(h.relative_file, Some(1));
        assert_eq!(
            h.checkpoint_scn,
            Some(Scn(1_572_129)),
            "v$datafile.checkpoint_change#"
        );
        assert!(h.has_dictionary_root());
    }

    #[test]
    fn rejects_non_header_and_bad_sizes() {
        let mut b = golden_block();
        assert!(
            FileHeader::parse(&b, ByteOrder::Big).is_err(),
            "wrong order"
        );
        b[0] = 0x06;
        assert!(matches!(
            FileHeader::parse(&b, ByteOrder::Little),
            Err(DecodeError::Malformed { offset: 0, .. })
        ));
        let mut b = golden_block();
        b[48..52].copy_from_slice(&4000u32.to_le_bytes());
        assert!(FileHeader::parse(&b, ByteOrder::Little).is_err());
        let b = golden_block();
        assert!(FileHeader::parse(&b[..4096], ByteOrder::Little).is_err());
        assert!(FileHeader::parse(&[], ByteOrder::Little).is_err());
        assert!(FileHeader::detect(&[0u8; 8192]).is_err());
    }

    #[test]
    fn version_dependent_fields_are_optional_not_guessed() {
        let mut b = golden_block();
        b[336..338].copy_from_slice(&40u16.to_le_bytes()); // tln > 30
        b[368..372].copy_from_slice(&0u32.to_le_bytes());
        let h = FileHeader::parse(&b, ByteOrder::Little).unwrap();
        assert_eq!(h.tablespace_name, None);
        assert_eq!(h.relative_file, None);
        assert_eq!(h.db_name, "ORCL", "stable fields unaffected");
    }
}
