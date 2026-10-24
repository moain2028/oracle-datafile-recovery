//! BASIC-compressed data blocks (`kdbhflag` 0x40, "9ir2" format) — `lab-verify` LV-071.
//!
//! Every fact below was measured on the three `T_COMPRESS` blocks of the 26ai
//! golden set v2 (`odr_odr_assm01.dbf` 5124–5126, 2,000 rows) and checked
//! against Oracle's own `ALTER SYSTEM DUMP DATAFILE` trace of the same blocks
//! (`bindmp:` raw bytes next to decoded `col` values) and against the
//! `rows.psv` truth (2,000/2,000 rows byte-identical).
//!
//! Layout after the 14-byte `kdbh`:
//!
//! | off | size | field            | measured                                   |
//! |-----|------|------------------|--------------------------------------------|
//! | 14  | 2    | `r0_9ir2`        | 0                                          |
//! | 16  | 2    | `mec_kdbh9ir2`   | 35 = number of **multi-entry** symbols     |
//! | 18  | 2    | `shcf_kdbh9ir2`  | 0                                          |
//! | 20  | 1    | `ncols`          | 3 = columns per row (length of `perm`)     |
//! | 21  | 1    | `flag_9ir2`      | 0x23 (`--R---OC`)                          |
//! | 22  | n    | `perm_9ir2[n]`   | `{2 0 1}`: stored position of table column |
//! |     | pad  |                  | to even length                             |
//!
//! Then the normal `kdbt[ntab]` and `kdbr[nrow]`. `kdbt[0]` is the **symbol
//! table**, `kdbt[1]` the data rows; `fsbo == 14 + ext + 4·ntab + 2·nrow`.
//!
//! A **symbol** is `00 <refcount> <entries>`; a **data row** is
//! `<flag> <lock> <entries>` (`flag` 0x2C = `H F L`, lock 0). `entries` is a
//! sequence of columns in *stored* order, parsed as:
//!
//! * first byte `< 0xC8` → it is a **count**; that many entries follow, each
//!   either a **symbol reference** (byte `< 0xC8`, index into `kdbt[0]`;
//!   the symbol's entries are spliced in, recursively) or an **inline
//!   literal** (byte `0xC8 + len`, then `len` bytes);
//! * first byte `≥ 0xC8` → a single inline literal with the count omitted.
//!
//! `0xFA..=0xFF` were never seen (they are reserved for long/escaped lengths
//! in Oracle's `kdr` encoding) and are refused. `refcount` equalled the number
//! of references to the symbol (from rows and from other symbols) in all 3
//! blocks; it is not needed to decode but is **checked** by
//! [`CompressedBlock::structural_check`]: a forged reference byte (checksum
//! recomputed) moves one count down and another up, and the whole block is
//! then rejected — measured: forge seeds whose byte landed on a reference
//! went from wrong-verified to all-rejected. A forged byte inside a literal
//! is not detectable from the block alone (same declared limit as plain
//! blocks). Every row expanded to exactly `ncols` columns and every symbol
//! with index `< mec` had more than one entry — both invariants are enforced
//! so that a block that *looks* compressed but deviates from the measured
//! grammar is refused rather than decoded wrongly.
//!
//! NULL columns were not present in the corpus; a literal of length 0 is
//! stored as `0xC8` (seen for none) — it is decoded as an empty value, which
//! the type layer treats like a stored zero-length column. Chained rows, OLTP
//! (`0x40` with `flag_9ir2` bit for "archive"/OLTP), cluster blocks and
//! `ntab != 2` are refused.

#![allow(clippy::arithmetic_side_effects)] // offsets bounded by block size (u16) — see kdb.rs

use super::kdb::{KDBH_LEN, Kdbh, TableDir, kdbh_flag};
use super::ktb::Ktbbh;
use crate::block::TAIL_LEN;
use crate::bytes::{ByteOrder, read_u8, read_u16};
use crate::error::{DecodeError, DecodeResult};

const WHAT: &str = "kdbh9ir2";
/// Fixed part of the 9ir2 extension before `perm`.
const EXT_FIXED: usize = 8;
/// First byte value of an inline literal (`0xC8 + len`).
const LITERAL_BASE: u8 = 0xC8;
/// Largest literal length byte seen/accepted (`0xC8 + 49`); `0xFA..` are
/// escape codes we do not understand and refuse.
const LITERAL_MAX: u8 = 0xF9;
/// Recursion bound for symbol → symbol references (measured depth ≤ 2).
const MAX_DEPTH: u8 = 8;
/// A data row is `H F L` with nothing else set.
const ROW_FLAG: u8 = super::flag::H | super::flag::F | super::flag::L;

/// Decoded 9ir2 header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header9ir2 {
    /// `mec_kdbh9ir2`: number of multi-entry symbols (they come first).
    pub mec: u16,
    /// `flag_9ir2`.
    pub flag: u8,
    /// `perm_9ir2`: `perm[table_col] = stored position`.
    pub perm: Vec<u8>,
}

/// A validated BASIC-compressed block.
#[derive(Debug, Clone)]
pub struct CompressedBlock<'a> {
    block: &'a [u8],
    order: ByteOrder,
    /// Transaction header.
    pub ktb: Ktbbh,
    /// Block offset of `kdbh`.
    pub kdbh_off: usize,
    /// Data header.
    pub kdbh: Kdbh,
    /// 9ir2 extension.
    pub ext: Header9ir2,
    /// Symbol table directory (`kdbt[0]`).
    pub symbols: TableDir,
    /// Data rows directory (`kdbt[1]`).
    pub data: TableDir,
    /// Row directory block offset.
    dir: usize,
}

/// One decoded compressed row: columns in **table** order (after `perm`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedRow<'a> {
    /// Physical row-directory slot.
    pub slot: u16,
    /// Flag byte.
    pub flag: u8,
    /// Lock byte.
    pub lock: u8,
    /// Column bytes in table order.
    pub columns: Vec<&'a [u8]>,
    /// Byte range of the row inside the block.
    pub span: (usize, usize),
}

impl<'a> CompressedBlock<'a> {
    /// Whether `kdbh` at `off` announces a compressed block (flag 0x40 and
    /// two tables). Does not validate anything else.
    pub fn looks_compressed(block: &[u8], off: usize) -> bool {
        matches!(
            (read_u8(block, off), read_u8(block, off + 1)),
            (Ok(f), Ok(2)) if f & kdbh_flag::COMPRESSED != 0
        )
    }

    /// Parse and structurally validate a BASIC-compressed block whose `kdbh`
    /// starts at `kdbh_off` (the caller has already located it the way
    /// [`super::DataBlock::parse`] does — MSSM/ASSM).
    ///
    /// # Errors
    /// [`DecodeError::Unsupported`] for OLTP / unknown flag bits;
    /// [`DecodeError::Malformed`] when any measured invariant fails.
    pub fn parse(block: &'a [u8], order: ByteOrder, kdbh_off: usize) -> DecodeResult<Self> {
        let ktb = Ktbbh::parse(block, order)?;
        let hdr = Kdbh::read_raw(block, order, kdbh_off)?;
        let bad = |reason| Err(DecodeError::malformed(WHAT, kdbh_off, reason));
        if hdr.flag & kdbh_flag::COMPRESSED == 0 {
            return bad("kdbh flag 0x40 not set");
        }
        if hdr.ntab != 2 {
            return bad("compressed block must have exactly 2 tables (symbols + rows)");
        }
        let e = kdbh_off + KDBH_LEN;
        let mec = read_u16(block, e + 2, order)?;
        let ncols = usize::from(read_u8(block, e + 6)?);
        let flag = read_u8(block, e + 7)?;
        // Only the exact flag combination measured on BASIC blocks is
        // accepted: `--R---OC` = 0x23. Anything else (OLTP, archive, HCC
        // precursors) is a different format.
        if flag != 0x23 {
            return Err(DecodeError::Unsupported(
                "compressed data block: flag_9ir2 is not the BASIC layout (0x23)",
            ));
        }
        if ncols == 0 || ncols > 255 {
            return bad("ncols out of range");
        }
        let perm: Vec<u8> = block
            .get(e + EXT_FIXED..e + EXT_FIXED + ncols)
            .ok_or(DecodeError::Truncated {
                offset: e + EXT_FIXED,
                need: ncols,
                len: block.len(),
            })?
            .to_vec();
        // perm must be a permutation of 0..ncols.
        let mut seen = vec![false; ncols];
        for &p in &perm {
            match seen.get_mut(usize::from(p)) {
                Some(s) if !*s => *s = true,
                _ => return bad("perm_9ir2 is not a permutation"),
            }
        }
        let mut ext_len = EXT_FIXED + ncols;
        ext_len += ext_len & 1;
        let kdbt = e + ext_len;
        let symbols = TableDir {
            first: read_u16(block, kdbt, order)?,
            rows: read_u16(block, kdbt + 2, order)?,
        };
        let data = TableDir {
            first: read_u16(block, kdbt + 4, order)?,
            rows: read_u16(block, kdbt + 6, order)?,
        };
        if symbols.first != 0
            || data.first != symbols.rows
            || u32::from(data.first) + u32::from(data.rows) != u32::from(hdr.nrow)
        {
            return bad("kdbt inconsistent with nrow");
        }
        if usize::from(mec) > usize::from(symbols.rows) {
            return bad("mec exceeds symbol count");
        }
        let dir = kdbt + 8;
        let dir_end = dir + 2 * usize::from(hdr.nrow);
        if kdbh_off + usize::from(hdr.fsbo) != dir_end {
            return bad("fsbo != 14 + ext + 8 + 2*nrow");
        }
        let heap_end = block.len().saturating_sub(TAIL_LEN);
        if hdr.fseo < hdr.fsbo || kdbh_off + usize::from(hdr.fseo) > heap_end {
            return bad("fseo outside [fsbo, block end]");
        }
        if hdr.frre != 0xFFFF {
            return bad("compressed block with free directory entries");
        }
        let cb = Self {
            block,
            order,
            ktb,
            kdbh_off,
            kdbh: hdr,
            ext: Header9ir2 { mec, flag, perm },
            symbols,
            data,
            dir,
        };
        cb.check_symbols()?;
        Ok(cb)
    }

    /// Byte order.
    pub const fn order(&self) -> ByteOrder {
        self.order
    }

    /// Row-directory entry `i` as a block offset, checked to lie inside the
    /// row heap `[kdbh+fseo, block-tail)`.
    fn offset(&self, i: u16) -> DecodeResult<usize> {
        if i >= self.kdbh.nrow {
            return Err(DecodeError::malformed("kdbr", 0, "slot index out of range"));
        }
        let rel = read_u16(self.block, self.dir + 2 * usize::from(i), self.order)?;
        let abs = self.kdbh_off + usize::from(rel);
        let heap_start = self.kdbh_off + usize::from(self.kdbh.fseo);
        let heap_end = self.block.len().saturating_sub(TAIL_LEN);
        if (heap_start..heap_end).contains(&abs) {
            Ok(abs)
        } else {
            Err(DecodeError::malformed(
                "kdbr",
                abs,
                "compressed row offset outside the heap",
            ))
        }
    }

    /// Every symbol must parse (references may point at any index `< nsym`,
    /// bounded by depth), and the multi-entry symbols must be exactly the
    /// first `mec` (measured on 3 blocks: 35 of 46).
    fn check_symbols(&self) -> DecodeResult<()> {
        for s in 0..self.symbols.rows {
            let off = self.offset(s)?;
            let hdr = read_u8(self.block, off)?;
            if hdr != 0 {
                return Err(DecodeError::malformed(
                    WHAT,
                    off,
                    "symbol header byte is not zero",
                ));
            }
            let mut n = 0usize;
            let mut cols = Vec::new();
            self.expand(off + 2, &mut cols, &mut n, 0)?;
            let multi = n > 1;
            if multi != (s < self.ext.mec) {
                return Err(DecodeError::malformed(
                    WHAT,
                    off,
                    "multi-entry symbols are not exactly the first mec",
                ));
            }
        }
        Ok(())
    }

    /// Parse the entry list at `at`, appending expanded columns to `out`;
    /// `top_entries` receives the number of *top-level* entries. Returns the
    /// offset just past the list.
    fn expand(
        &self,
        at: usize,
        out: &mut Vec<&'a [u8]>,
        top_entries: &mut usize,
        depth: u8,
    ) -> DecodeResult<usize> {
        if depth >= MAX_DEPTH {
            return Err(DecodeError::malformed(
                WHAT,
                at,
                "symbol reference chain too deep",
            ));
        }
        let first = read_u8(self.block, at)?;
        let (count, mut p) = if first >= LITERAL_BASE {
            (1usize, at)
        } else {
            (usize::from(first), at + 1)
        };
        *top_entries = count;
        for _ in 0..count {
            let c = read_u8(self.block, p)?;
            p += 1;
            if c < LITERAL_BASE {
                let idx = u16::from(c);
                if idx >= self.symbols.rows {
                    return Err(DecodeError::malformed(
                        WHAT,
                        p - 1,
                        "symbol reference outside kdbt[0]",
                    ));
                }
                let soff = self.offset(idx)?;
                if read_u8(self.block, soff)? != 0 {
                    return Err(DecodeError::malformed(
                        WHAT,
                        soff,
                        "referenced symbol has a non-zero header",
                    ));
                }
                let mut inner = 0usize;
                self.expand(soff + 2, out, &mut inner, depth + 1)?;
            } else if c <= LITERAL_MAX {
                let len = usize::from(c - LITERAL_BASE);
                let v = self.block.get(p..p + len).ok_or(DecodeError::Truncated {
                    offset: p,
                    need: len,
                    len: self.block.len(),
                })?;
                out.push(v);
                p += len;
            } else {
                return Err(DecodeError::Unsupported(
                    "compressed data block: length escape 0xFA..0xFF not understood",
                ));
            }
        }
        Ok(p)
    }

    /// Structural check above the checksum (LV-062 applied to compressed
    /// blocks): every symbol's stored reference count must equal the number
    /// of references to it from data rows and from other symbols. Returns
    /// the first mismatch. Parsing errors inside rows are not faults here
    /// (they are reported per row).
    pub fn structural_check(&self) -> Option<super::kdb::StructuralFault> {
        let nsym = usize::from(self.symbols.rows);
        let mut refs = vec![0usize; nsym];
        let mut count_refs = |at: usize| {
            let Ok(first) = read_u8(self.block, at) else {
                return;
            };
            let (count, mut p) = if first >= LITERAL_BASE {
                (1usize, at)
            } else {
                (usize::from(first), at + 1)
            };
            for _ in 0..count {
                let Ok(c) = read_u8(self.block, p) else {
                    return;
                };
                p += 1;
                if c < LITERAL_BASE {
                    if let Some(r) = refs.get_mut(usize::from(c)) {
                        *r = r.saturating_add(1);
                    }
                } else if c <= LITERAL_MAX {
                    p += usize::from(c - LITERAL_BASE);
                } else {
                    return;
                }
            }
        };
        for s in 0..self.kdbh.nrow {
            if let Ok(off) = self.offset(s) {
                count_refs(off + 2);
            }
        }
        for s in 0..self.symbols.rows {
            let Ok(off) = self.offset(s) else { continue };
            let Ok(declared) = read_u8(self.block, off + 1) else {
                continue;
            };
            let referenced = refs.get(usize::from(s)).copied().unwrap_or(0);
            if usize::from(declared) != referenced {
                return Some(super::kdb::StructuralFault::SymbolRefCount {
                    symbol: s,
                    declared,
                    referenced,
                });
            }
        }
        None
    }

    /// Decode data row at physical slot `slot` (must lie in `kdbt[1]`).
    /// Columns are returned in **table** order.
    ///
    /// # Errors
    /// Any violation of the measured grammar for this row only.
    pub fn row(&self, slot: u16) -> DecodeResult<CompressedRow<'a>> {
        if slot < self.data.first || slot >= self.kdbh.nrow {
            return Err(DecodeError::malformed(
                "kdbr",
                0,
                "slot is not a data row of this compressed block",
            ));
        }
        let off = self.offset(slot)?;
        let flag = read_u8(self.block, off)?;
        let lock = read_u8(self.block, off + 1)?;
        if flag != ROW_FLAG {
            return Err(DecodeError::Unsupported(
                "compressed row that is not a complete single piece (H F L)",
            ));
        }
        let mut stored = Vec::with_capacity(self.ext.perm.len());
        let mut n = 0usize;
        let end = self.expand(off + 2, &mut stored, &mut n, 0)?;
        if stored.len() != self.ext.perm.len() {
            return Err(DecodeError::malformed(
                WHAT,
                off,
                "row does not expand to ncols columns",
            ));
        }
        let mut columns = Vec::with_capacity(stored.len());
        for &p in &self.ext.perm {
            let Some(&v) = stored.get(usize::from(p)) else {
                return Err(DecodeError::malformed(WHAT, off, "perm index out of range"));
            };
            columns.push(v);
        }
        Ok(CompressedRow {
            slot,
            flag,
            lock,
            columns,
            span: (off, end.saturating_sub(off)),
        })
    }

    /// Iterate every data row: `(slot, result)`.
    pub fn rows(&self) -> impl Iterator<Item = (u16, DecodeResult<CompressedRow<'a>>)> + '_ {
        (self.data.first..self.kdbh.nrow).map(|s| (s, self.row(s)))
    }
}
