//! Data header (`kdbh`), table directory (`kdbt`) and row directory (`kdbr`).
//!
//! `kdbh` (14 bytes, offsets relative to its start) — `lab-verify` LV-005/006/007:
//!
//! | off | size | field      | meaning                                    |
//! |-----|------|------------|--------------------------------------------|
//! | 0   | 1    | `kdbhflag` | flags                                      |
//! | 1   | 1    | `kdbhntab` | number of tables (> 1 only for clusters)   |
//! | 2   | 2    | `kdbhnrow` | number of row-directory entries            |
//! | 4   | 2    | `kdbhfrre` | first free directory entry (−1 = none)     |
//! | 6   | 2    | `kdbhfsbo` | free space begin offset                    |
//! | 8   | 2    | `kdbhfseo` | free space end offset (= lowest row)       |
//! | 10  | 2    | `kdbhavsp` | available space                            |
//! | 12  | 2    | `kdbhtosp` | total space after commit                   |
//!
//! Then `kdbt[ntab]` (4 bytes each: first slot u16, row count u16) and
//! `kdbr[nrow]` (u16 offsets, **relative to the start of `kdbh`**).
//! Invariant: `fsbo == 14 + 4·ntab + 2·nrow`.
//!
//! `kdbhflag` bits that change how the block must be read are in
//! [`kdbh_flag`]. Compressed blocks (basic / OLTP, bit `0x40`) insert extra
//! `9ir2` header fields before `kdbt` and store rows as symbol-table
//! references; they are reported as [`DecodeError::Unsupported`] — decoding
//! them as plain rows would produce wrong values.
//!
//! **Where `kdbh` starts** differs between freelist (MSSM) blocks — right after
//! the ITL array — and ASSM blocks, which have 8 more bytes first. Instead of
//! trusting a flag, [`DataBlock::parse`] tries both and keeps the one whose
//! header satisfies every structural invariant; if both or neither do, the
//! block is rejected (no silent guess).

// Offset arithmetic below combines a bounded base (kdbh offset <= 0x2C + 24*255 + 8)
// with `u16`/`u8` header values, so every sum stays below 2^18 and cannot overflow
// `usize`; every resulting offset is then bounds-checked by the `read_*` helpers.
#![allow(clippy::arithmetic_side_effects)]

use super::RowPiece;
use super::ktb::Ktbbh;
use super::piece::{PieceFormat, parse_piece};
use crate::block::TAIL_LEN;
use crate::bytes::{ByteOrder, read_u8, read_u16};
use crate::error::{DecodeError, DecodeResult};

const WHAT: &str = "kdbh";
/// Size of `kdbh`.
pub const KDBH_LEN: usize = 14;
/// Extra bytes before `kdbh` on ASSM blocks (`lab-verify` LV-005).
pub const ASSM_GAP: usize = 8;

/// `kdbhflag` bits (block dumps print them as `76543210` / `flag=…`).
/// `lab-verify` LV-018/022.
pub mod kdbh_flag {
    /// `flag=-0------`: compressed block (basic or OLTP) with a symbol table
    /// in `kdbt[0]` and `9ir2` header extensions.
    pub const COMPRESSED: u8 = 0x40;
    /// `flag=--R-----`: rows carry a 6-byte dependency SCN (`ROWDEPENDENCIES`).
    pub const ROW_DEPENDENCIES: u8 = 0x20;
    /// `flag=-------K`: cluster block. **Not reliable** — published dumps of
    /// cluster blocks show it both set and clear; use `ntab` and row flags.
    pub const CLUSTER: u8 = 0x01;
}

/// Parsed `kdbh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kdbh {
    /// Flags.
    pub flag: u8,
    /// Number of tables in this block.
    pub ntab: u8,
    /// Number of row-directory entries.
    pub nrow: u16,
    /// First free directory entry (`0xFFFF` = none).
    pub frre: u16,
    /// Free space begin offset.
    pub fsbo: u16,
    /// Free space end offset.
    pub fseo: u16,
    /// Available space.
    pub avsp: u16,
    /// Total space.
    pub tosp: u16,
}

/// One `kdbt` entry: the slots `first .. first + rows` belong to this table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableDir {
    /// First row-directory slot of this table.
    pub first: u16,
    /// Number of slots.
    pub rows: u16,
}

/// Where `kdbh` was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Freelist-managed: directly after the ITL array.
    Mssm,
    /// Automatic segment space management: 8 bytes later.
    Assm,
}

/// A row-directory slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Points at row data at this block offset.
    Row(usize),
    /// Unused / free entry (offset outside the row heap).
    Unused,
}

/// A validated table data block. Borrowing, zero-copy.
#[derive(Debug, Clone)]
pub struct DataBlock<'a> {
    block: &'a [u8],
    order: ByteOrder,
    /// Transaction header.
    pub ktb: Ktbbh,
    /// Detected layout.
    pub layout: Layout,
    /// Block offset of `kdbh`.
    pub kdbh_off: usize,
    /// Data header.
    pub kdbh: Kdbh,
    /// Table directory.
    pub tables: Vec<TableDir>,
    /// Settled `dscn` width for ROWDEPENDENCIES blocks (LV-052).
    pub dscn_len: u8,
}

impl<'a> DataBlock<'a> {
    /// Parse and structurally validate a table data block (`type_kcbh == 0x06`,
    /// already classified `Valid` by the scanner).
    ///
    /// # Errors
    /// [`DecodeError::Malformed`] if no layout satisfies the invariants, or if
    /// both do (ambiguous); [`DecodeError::Truncated`] for short blocks.
    pub fn parse(block: &'a [u8], order: ByteOrder) -> DecodeResult<Self> {
        let ktb = Ktbbh::parse(block, order)?;
        let base = ktb.itl_end();
        let mssm = Kdbh::parse_checked(block, order, base);
        let assm = Kdbh::parse_checked(block, order, base.saturating_add(ASSM_GAP));
        let (layout, kdbh_off, kdbh) = match (mssm, assm) {
            (Ok(h), Err(_)) => (Layout::Mssm, base, h),
            (Err(_), Ok(h)) => (Layout::Assm, base.saturating_add(ASSM_GAP), h),
            (Ok(_), Ok(_)) => {
                return Err(DecodeError::malformed(
                    WHAT,
                    base,
                    "ambiguous MSSM/ASSM layout",
                ));
            }
            // Prefer the more specific diagnosis when one candidate is a
            // recognised-but-unsupported format (both are errors either way).
            (Err(e @ DecodeError::Unsupported(_)), Err(_)) | (Err(_), Err(e)) => return Err(e),
        };
        let mut tables = Vec::with_capacity(usize::from(kdbh.ntab));
        let mut expected_first = 0u16;
        for t in 0..usize::from(kdbh.ntab) {
            let o = kdbh_off + KDBH_LEN + 4 * t;
            let td = TableDir {
                first: read_u16(block, o, order)?,
                rows: read_u16(block, o + 2, order)?,
            };
            let end = u32::from(td.first) + u32::from(td.rows);
            if td.first != expected_first || end > u32::from(kdbh.nrow) {
                return Err(DecodeError::malformed(
                    "kdbt",
                    o,
                    "table directory inconsistent with nrow",
                ));
            }
            expected_first = u16::try_from(end).unwrap_or(u16::MAX);
            tables.push(td);
        }
        let mut db = Self {
            block,
            order,
            ktb,
            layout,
            kdbh_off,
            kdbh,
            tables,
            dscn_len: 6,
        };
        if db.kdbh.flag & kdbh_flag::ROW_DEPENDENCIES != 0 && db.kdbh.nrow > 0 {
            db.dscn_len = db.settle_dscn_len().ok_or(DecodeError::malformed(
                WHAT,
                kdbh_off,
                "ROWDEPENDENCIES block: dscn width cannot be settled (LV-052)",
            ))?;
        }
        Ok(db)
    }

    /// Byte order of the block.
    pub const fn order(&self) -> ByteOrder {
        self.order
    }

    /// Row-piece layout implied by the block header.
    pub const fn piece_format(&self) -> PieceFormat {
        PieceFormat {
            row_dependencies: self.kdbh.flag & kdbh_flag::ROW_DEPENDENCIES != 0,
            dscn_len: self.dscn_len,
        }
    }

    /// Decide the `dscn` width of a ROWDEPENDENCIES block (LV-052) from the
    /// row-packing invariant: rows never overlap, and the highest row ends
    /// exactly at the heap end (rows are stacked downward from the tail).
    /// An updated row may leave a hole below its old position (26ai
    /// `T_ROWDEP` id 100), so interior rows may end *before* the next one but
    /// never after it. Returns the single width in `6..=8` for which **all**
    /// rows satisfy this; `None` if none or more than one does (then the
    /// block is rejected rather than read with a guessed layout).
    fn settle_dscn_len(&self) -> Option<u8> {
        let heap_end = self.block.len().saturating_sub(TAIL_LEN);
        let heap = self.block.get(..heap_end).unwrap_or_default();
        let mut offs: Vec<usize> = (0..self.kdbh.nrow)
            .filter_map(|i| match self.slot(i) {
                Ok(Slot::Row(o)) => Some(o),
                _ => None,
            })
            .collect();
        offs.sort_unstable();
        offs.dedup();
        let mut fits = Vec::new();
        for len in 6u8..=8 {
            let fmt = PieceFormat {
                row_dependencies: true,
                dscn_len: len,
            };
            let ok = offs.iter().enumerate().all(|(k, &o)| {
                let Ok(p) = parse_piece(heap, o, 0, self.order, fmt) else {
                    return false;
                };
                match offs.get(k + 1) {
                    Some(&next) => o + p.len <= next,
                    None => o + p.len == heap_end,
                }
            });
            if ok {
                fits.push(len);
            }
        }
        match fits.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }

    /// Index into [`Self::tables`] of the table that owns slot `i`, or `None`
    /// if no `kdbt` entry covers it. In a cluster block table 0 is the key
    /// table and table `t ≥ 1` has `TAB$.TAB# == t`.
    pub fn table_of(&self, i: u16) -> Option<usize> {
        self.tables
            .iter()
            .position(|t| i >= t.first && u32::from(i) < u32::from(t.first) + u32::from(t.rows))
    }

    /// The cluster-key row a member row refers to (`cki` indexes `kdbt[0]`).
    ///
    /// # Errors
    /// [`DecodeError::Malformed`] if `key_index` is outside `kdbt[0]` or the
    /// slot is unused; any decoding error of the key row.
    pub fn cluster_key(&self, key_index: u8) -> DecodeResult<RowPiece<'a>> {
        let bad = |reason| Err(DecodeError::malformed("cluster key", 0, reason));
        let Some(keys) = self.tables.first() else {
            return bad("block has no table directory");
        };
        if u16::from(key_index) >= keys.rows {
            return bad("cki outside kdbt[0]");
        }
        let slot = keys.first.saturating_add(u16::from(key_index));
        match self.row(slot)? {
            Some(p) if p.is_cluster_key() => Ok(p),
            Some(_) => bad("cki slot is not a cluster-key row"),
            None => bad("cki slot is unused"),
        }
    }

    /// Read row-directory slot `i`.
    ///
    /// # Errors
    /// [`DecodeError::Malformed`] if `i >= nrow`.
    pub fn slot(&self, i: u16) -> DecodeResult<Slot> {
        if i >= self.kdbh.nrow {
            return Err(DecodeError::malformed("kdbr", 0, "slot index out of range"));
        }
        let dir = self.kdbh_off + KDBH_LEN + 4 * usize::from(self.kdbh.ntab);
        let rel = read_u16(self.block, dir + 2 * usize::from(i), self.order)?;
        let abs = self.kdbh_off + usize::from(rel);
        let heap_start = self.kdbh_off + usize::from(self.kdbh.fseo);
        let heap_end = self.block.len().saturating_sub(TAIL_LEN);
        Ok(if (heap_start..heap_end).contains(&abs) {
            Slot::Row(abs)
        } else {
            Slot::Unused
        })
    }

    /// Decode the row piece in slot `i`. `None` for unused slots.
    ///
    /// # Errors
    /// Any decoding error for that row only.
    pub fn row(&self, i: u16) -> DecodeResult<Option<RowPiece<'a>>> {
        match self.slot(i)? {
            Slot::Unused => Ok(None),
            Slot::Row(off) => {
                let heap_end = self.block.len().saturating_sub(TAIL_LEN);
                let heap = self.block.get(..heap_end).unwrap_or_default();
                parse_piece(heap, off, i, self.order, self.piece_format()).map(Some)
            }
        }
    }

    /// Iterate all slots: `(slot, result)`; unused slots are skipped. An error
    /// in one row never stops the iteration (REQ-001.4 applied to rows).
    pub fn rows(&self) -> impl Iterator<Item = (u16, DecodeResult<RowPiece<'a>>)> + '_ {
        (0..self.kdbh.nrow).filter_map(|i| match self.row(i) {
            Ok(None) => None,
            Ok(Some(p)) => Some((i, Ok(p))),
            Err(e) => Some((i, Err(e))),
        })
    }
}

impl Kdbh {
    /// Parse `kdbh` at `off` and check every invariant against the block.
    fn parse_checked(block: &[u8], order: ByteOrder, off: usize) -> DecodeResult<Self> {
        let h = Self {
            flag: read_u8(block, off)?,
            ntab: read_u8(block, off + 1)?,
            nrow: read_u16(block, off + 2, order)?,
            frre: read_u16(block, off + 4, order)?,
            fsbo: read_u16(block, off + 6, order)?,
            fseo: read_u16(block, off + 8, order)?,
            avsp: read_u16(block, off + 10, order)?,
            tosp: read_u16(block, off + 12, order)?,
        };
        let space = block.len().saturating_sub(TAIL_LEN).saturating_sub(off);
        let dir_end = KDBH_LEN + 4 * usize::from(h.ntab) + 2 * usize::from(h.nrow);
        let bad = |reason| Err(DecodeError::malformed(WHAT, off, reason));
        // Compressed blocks always have ntab >= 2 (symbol table + data) and
        // fail the fsbo invariant below (their header is longer), so they can
        // never be mis-read as plain rows; this only sharpens the diagnosis.
        if h.flag & kdbh_flag::COMPRESSED != 0 && h.ntab >= 2 {
            return Err(DecodeError::Unsupported("compressed data block (9ir2)"));
        }
        // LV-048: a freshly formatted, never-used data block has
        // ntab = 0, nrow = 0, fsbo = 14 (seen 26ai: 321 blocks). It holds no
        // rows but is well-formed; `rows()` yields nothing for it.
        if h.ntab == 0 && (h.nrow != 0 || h.frre != 0xFFFF) {
            return bad("ntab is zero but rows are present");
        }
        if usize::from(h.fsbo) != dir_end {
            return bad("fsbo != 14 + 4*ntab + 2*nrow");
        }
        if h.fseo < h.fsbo || usize::from(h.fseo) > space {
            return bad("fseo outside [fsbo, block end]");
        }
        if usize::from(h.avsp) > space || usize::from(h.tosp) > space {
            return bad("free space larger than block");
        }
        if h.frre != 0xFFFF && h.frre >= h.nrow {
            return bad("free-list head outside row directory");
        }
        Ok(h)
    }
}
