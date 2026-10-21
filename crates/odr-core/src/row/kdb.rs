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

/// A structural invariant violated by a block that passed its checksum
/// (see [`DataBlock::structural_check`], LV-062). Every variant names the
/// evidence so a forensic report can cite it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub enum StructuralFault {
    /// The block's own `rdba` does not match where it was read from: a
    /// misdirected write or a swapped block (Oracle's "block mismatch").
    Misplaced {
        /// Block number in the `kcbh` header.
        header_block: u32,
        /// Block number of the position it was read from.
        found_at: u32,
    },
    /// A row's lock byte points past the ITL array (`kdbchk` 6101).
    LockOutsideItl {
        /// Row-directory slot.
        slot: u16,
        /// The lock byte.
        lock: u8,
    },
    /// An ITL declares fewer locked rows than reference it (`kdbchk` 6108).
    ItlLockCount {
        /// ITL index (1-based, as in the row's lock byte).
        itl: u16,
        /// `lck` in the ITL entry.
        declared: u16,
        /// Rows whose lock byte equals `itl`.
        referenced: u16,
    },
    /// The free-slot chain from `frre` is broken (`kdbchk` 6106).
    FreeChain {
        /// Slot where the chain left the directory, cycled or hit a used slot.
        slot: u16,
    },
    /// A slot that does not decode although its flag byte does not say
    /// *deleted* (every undecodable slot in a healthy block is a reused
    /// deleted stub, LV-058).
    UndecodableLiveRow {
        /// The slot.
        slot: u16,
        /// Raw flag byte.
        flag: u8,
    },
    /// An unused directory entry is not reachable from `frre`.
    UnusedSlotOffChain {
        /// The slot.
        slot: u16,
    },
    /// Two live rows share bytes.
    RowOverlap {
        /// Block offset of the second row.
        offset: usize,
    },
    /// A live row lies outside `[kdbh + fseo, block − tail)`.
    RowOutsideHeap {
        /// Block offset.
        offset: usize,
        /// Row length.
        len: usize,
    },
    /// `avsp` claims more free space than the rows leave (`kdbchk` 6110).
    AvailableSpace {
        /// `kdbhavsp`.
        avsp: u16,
        /// Upper bound from the rows.
        max: usize,
    },
}

impl std::fmt::Display for StructuralFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Misplaced {
                header_block,
                found_at,
            } => write!(
                f,
                "header says block {header_block} but it was read at block {found_at}"
            ),
            Self::LockOutsideItl { slot, lock } => {
                write!(f, "slot {slot} lock byte {lock} exceeds ITL count")
            }
            Self::ItlLockCount {
                itl,
                declared,
                referenced,
            } => write!(
                f,
                "ITL {itl} declares {declared} locked rows but {referenced} rows reference it"
            ),
            Self::FreeChain { slot } => write!(f, "free-slot chain broken at slot {slot}"),
            Self::UndecodableLiveRow { slot, flag } => {
                write!(
                    f,
                    "slot {slot} (flag {flag:#04x}, not deleted) does not decode"
                )
            }
            Self::UnusedSlotOffChain { slot } => {
                write!(f, "unused slot {slot} not on the free chain")
            }
            Self::RowOverlap { offset } => write!(f, "live rows overlap at offset {offset:#x}"),
            Self::RowOutsideHeap { offset, len } => {
                write!(f, "row at {offset:#x} (+{len}) outside the row heap")
            }
            Self::AvailableSpace { avsp, max } => {
                write!(f, "avsp {avsp} exceeds the space the rows leave ({max})")
            }
        }
    }
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

    /// The raw flag byte of slot `i` without decoding the piece (`None` for
    /// unused slots). Lets callers tell a deleted-and-overwritten row from a
    /// genuinely malformed one (LV-058).
    pub fn row_flag(&self, i: u16) -> Option<u8> {
        match self.slot(i).ok()? {
            Slot::Unused => None,
            Slot::Row(off) => self.block.get(off).copied(),
        }
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

    /// Structural validation **above** the checksum (LV-062): the invariants
    /// a healthy Oracle data block always satisfies, independent of
    /// `kcbh` checksum/tail. A block whose bytes were altered and whose
    /// checksum was recomputed ("forge") passes [`crate::block::classify`]
    /// yet may break one of these. Only invariants that held on **every**
    /// Valid data block of the 26ai golden files (17,683 SYSTEM + 394 ASSM +
    /// 6 MSSM + 1 bigfile) are used; the published `kdbchk` equalities
    /// (6110 `avsp == dtl − used − fsc`, 6111 `tosp`, exact 6108 lock
    /// counts) do **not** hold on real blocks after row updates and are
    /// deliberately *not* enforced.
    ///
    /// Checks (named after the closest `kdbchk` code):
    /// * **6101** — every stored row's `lock` byte is `≤ itc` (0 = unlocked);
    /// * **live-undecodable** — a slot that fails to decode must carry `D`;
    /// * **6108'** — per ITL, `lck ≥` rows that reference it (Oracle keeps
    ///   `lck` as an upper bound after partial cleanout, never below);
    /// * **6106'** — the free-slot chain from `frre` visits only unused slots,
    ///   stays inside the directory, has no cycle and ends in `0xFFFF`; and
    ///   every unused slot is on that chain;
    /// * **overlap** — live (non-deleted) rows never overlap each other and
    ///   lie inside `[kdbh+fseo, block−tail)`;
    /// * **6110'** — `avsp ≤ dtl − dir − Σ live max(len, 9)` (deleted stubs
    ///   are free space to Oracle; `avsp` can be stale-low, never too high).
    ///
    /// Returns the first violated rule, or `None` if the block is
    /// structurally consistent. Never panics.
    #[allow(clippy::too_many_lines)] // one pass over the directory; each rule is a few lines
    pub fn structural_check(&self) -> Option<StructuralFault> {
        let itc = self.ktb.itl_count;
        let heap_end = self.block.len().saturating_sub(TAIL_LEN);
        let heap_start = self.kdbh_off.saturating_add(usize::from(self.kdbh.fseo));
        let mut locks_by_itl: Vec<u16> = vec![0; usize::from(itc).saturating_add(1)];
        let mut live: Vec<(usize, usize)> = Vec::new();
        let mut used_min9: usize = 0;
        let mut unused: Vec<u16> = Vec::new();
        for i in 0..self.kdbh.nrow {
            match self.slot(i) {
                Ok(Slot::Unused) => unused.push(i),
                Ok(Slot::Row(o)) => {
                    // An undecodable slot is normal only for a deleted stub
                    // whose bytes were reused (LV-058): on 2,254 such slots
                    // in healthy 26ai blocks the flag byte always carries
                    // `D` and the lock byte is always `≤ itc`. A slot that
                    // does not decode and breaks either is a fault.
                    let Ok(Some(p)) = self.row(i) else {
                        let flag = self.block.get(o).copied().unwrap_or(0);
                        let lock = self.block.get(o.saturating_add(1)).copied().unwrap_or(0);
                        if flag & super::flag::D == 0 {
                            return Some(StructuralFault::UndecodableLiveRow { slot: i, flag });
                        }
                        if u16::from(lock) > itc {
                            return Some(StructuralFault::LockOutsideItl { slot: i, lock });
                        }
                        continue;
                    };
                    if u16::from(p.lock) > itc {
                        return Some(StructuralFault::LockOutsideItl {
                            slot: i,
                            lock: p.lock,
                        });
                    }
                    if let Some(c) = locks_by_itl.get_mut(usize::from(p.lock)) {
                        *c = c.saturating_add(1);
                    }
                    // Deleted stubs are free space to Oracle (their bytes
                    // may already be reused, LV-058); only live rows count.
                    if !p.is_deleted() {
                        used_min9 = used_min9.saturating_add(p.len.max(9));
                        live.push((o, p.len));
                    }
                }
                Err(_) => {}
            }
        }
        for i in 1..=itc {
            let Ok(itl) = self.ktb.itl(self.block, self.order, i.saturating_sub(1)) else {
                continue;
            };
            let referenced = locks_by_itl.get(usize::from(i)).copied().unwrap_or(0);
            if itl.locked_rows < referenced {
                return Some(StructuralFault::ItlLockCount {
                    itl: i,
                    declared: itl.locked_rows,
                    referenced,
                });
            }
        }
        // Free-slot chain.
        let dir = self
            .kdbh_off
            .saturating_add(KDBH_LEN)
            .saturating_add(4 * usize::from(self.kdbh.ntab));
        let mut on_chain: Vec<u16> = Vec::new();
        let mut cur = self.kdbh.frre;
        let mut hops: u32 = 0;
        while cur != 0xFFFF {
            if cur >= self.kdbh.nrow || on_chain.contains(&cur) || hops > u32::from(u16::MAX) {
                return Some(StructuralFault::FreeChain { slot: cur });
            }
            if !matches!(self.slot(cur), Ok(Slot::Unused)) {
                return Some(StructuralFault::FreeChain { slot: cur });
            }
            on_chain.push(cur);
            hops = hops.saturating_add(1);
            let Ok(next) = read_u16(
                self.block,
                dir.saturating_add(2 * usize::from(cur)),
                self.order,
            ) else {
                return Some(StructuralFault::FreeChain { slot: cur });
            };
            cur = next;
        }
        if let Some(&s) = unused.iter().find(|s| !on_chain.contains(s)) {
            return Some(StructuralFault::UnusedSlotOffChain { slot: s });
        }
        // Live rows: inside the heap, no overlap.
        live.sort_unstable();
        for w in live.windows(2) {
            if let [(o1, l1), (o2, _)] = *w
                && o1.saturating_add(l1) > o2
            {
                return Some(StructuralFault::RowOverlap { offset: o2 });
            }
        }
        if let Some(&(o, l)) = live
            .iter()
            .find(|&&(o, l)| o < heap_start || o.saturating_add(l) > heap_end)
        {
            return Some(StructuralFault::RowOutsideHeap { offset: o, len: l });
        }
        // avsp upper bound.
        let dtl = heap_end.saturating_sub(self.kdbh_off);
        let dir_len = KDBH_LEN
            .saturating_add(4 * usize::from(self.kdbh.ntab))
            .saturating_add(2 * usize::from(self.kdbh.nrow));
        let free_max = dtl.saturating_sub(dir_len).saturating_sub(used_min9);
        if usize::from(self.kdbh.avsp) > free_max {
            return Some(StructuralFault::AvailableSpace {
                avsp: self.kdbh.avsp,
                max: free_max,
            });
        }
        None
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
