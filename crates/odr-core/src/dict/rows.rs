//! Row readers over a [`BlockSource`]: every row of a heap segment or of one
//! table inside an indexed cluster, with provenance, chained/migrated rows
//! assembled, deleted rows and non-head pieces skipped.
//!
//! Safety rules (REQ-001.4 / REQ-004.1):
//! * a block that is not a data block (`type_kcbh != 0x06`), that belongs
//!   to another object (`ktbbh.objd != objd`), or that fails to parse, is
//!   recorded in [`SegmentRows::skipped`] and the walk continues;
//! * a row that fails to decode is returned as `Err` for that row only;
//! * chained / migrated pieces are fetched through the same source and
//!   bounded by [`crate::row::assemble::MAX_PIECES`].

use super::segment::SegmentBlocks;
use super::source::{BlockAddr, BlockSource};
use crate::block::{BlockHeader, Rdba};
use crate::bytes::ByteOrder;
use crate::error::DecodeError;
use crate::row::assemble::MAX_PIECES;
use crate::row::{
    AssembleError, ClusterPart, DataBlock, PieceSource, Provenance, RowPiece, assemble,
    cluster_columns,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Why a block of the segment produced no rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum Skip {
    /// Not readable from the source.
    Unavailable,
    /// Not a table data block (bitmap, segment header, …).
    NotDataBlock(u8),
    /// Data block of another object.
    ForeignObject(u32),
    /// Block does not decode as a data block.
    Malformed(DecodeError),
}

/// One row with its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRow {
    /// File / block / physical slot / SCN of the head piece.
    pub provenance: Provenance,
    /// Row number as Oracle prints it in the ROWID: the physical slot for a
    /// heap table, but for a cluster member the index **inside its own
    /// `kdbt` section** (`slot - kdbt[tab].first`) — lab-verify LV-046.
    pub rowid_row: u16,
    /// Stored column bytes in segment order (trailing NULLs absent).
    pub columns: Vec<Option<Vec<u8>>>,
    /// Addresses of all pieces (head first) when the row was chained/migrated.
    pub pieces: Vec<(u32, u16)>,
}

/// Why a row could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RowError {
    /// The head piece did not decode.
    Decode(DecodeError),
    /// Pieces could not be joined.
    Assemble(AssembleError),
    /// A cluster member points at a key that does not decode.
    ClusterKey(DecodeError),
}

/// Output of a segment read.
#[derive(Debug, Default)]
pub struct SegmentRows {
    /// Rows, in block/slot order.
    pub rows: Vec<Result<RawRow, (Provenance, RowError)>>,
    /// Blocks that yielded nothing, with the reason.
    pub skipped: Vec<(BlockAddr, Skip)>,
    /// Data blocks successfully read.
    pub data_blocks: usize,
}

/// Continuation blocks of one chained/migrated row, fetched up front so the
/// assembler can borrow from them. Bounded by [`MAX_PIECES`] hops and a
/// visited set, so a corrupt `nrid` loop terminates.
fn prefetch_chain<S: BlockSource + ?Sized>(
    src: &S,
    from_file: u32,
    head: &RowPiece<'_>,
) -> HashMap<u32, Vec<u8>> {
    let order = src.order();
    let mut map: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut seen: HashSet<(u32, u16)> = HashSet::new();
    let mut next = head.next;
    let mut hops = 0usize;
    while let Some((rdba, slot)) = next {
        if hops >= MAX_PIECES || !seen.insert((rdba, slot)) {
            break;
        }
        hops = hops.saturating_add(1);
        if let std::collections::hash_map::Entry::Vacant(e) = map.entry(rdba) {
            let Some(addr) = src.resolve(Rdba(rdba), from_file) else {
                break;
            };
            let Some(bytes) = src.block(addr) else { break };
            e.insert(bytes);
        }
        let Some(bytes) = map.get(&rdba) else { break };
        next = DataBlock::parse(bytes, order)
            .ok()
            .and_then(|db| db.row(slot).ok().flatten())
            .and_then(|p| p.next);
    }
    map
}

/// `PieceSource` over prefetched blocks.
struct MapPieces<'a> {
    map: &'a HashMap<u32, Vec<u8>>,
    order: ByteOrder,
    objd: u32,
}

impl PieceSource for MapPieces<'_> {
    fn piece(&self, rdba: u32, slot: u16) -> Option<Result<RowPiece<'_>, DecodeError>> {
        let bytes = self.map.get(&rdba)?;
        let db = match DataBlock::parse(bytes, self.order) {
            Ok(db) => db,
            Err(e) => return Some(Err(e)),
        };
        if db.ktb.objd != self.objd {
            return Some(Err(DecodeError::malformed(
                "row piece",
                0,
                "continuation block belongs to another object",
            )));
        }
        db.row(slot).transpose()
    }
}

/// Read every row of a heap-organised segment (`objd` = `DATA_OBJECT_ID`).
pub fn read_heap<S: BlockSource + ?Sized>(
    src: &S,
    seg: &SegmentBlocks,
    from_file: u32,
    objd: u32,
) -> SegmentRows {
    read_impl(src, seg, from_file, objd, None)
}

/// Read every row of table `tab` inside an indexed cluster segment; key
/// columns are prepended (padded to `clucols`).
pub fn read_cluster<S: BlockSource + ?Sized>(
    src: &S,
    seg: &SegmentBlocks,
    from_file: u32,
    objd: u32,
    tab: usize,
    clucols: usize,
) -> SegmentRows {
    read_impl(src, seg, from_file, objd, Some((tab, clucols)))
}

#[allow(clippy::too_many_lines)] // one linear walk; splitting would obscure the skip/err paths
fn read_impl<S: BlockSource + ?Sized>(
    src: &S,
    seg: &SegmentBlocks,
    from_file: u32,
    objd: u32,
    cluster: Option<(usize, usize)>,
) -> SegmentRows {
    let order = src.order();
    let mut out = SegmentRows::default();
    let Some(scheme) = src.scheme(from_file) else {
        return out;
    };
    for (rel, block) in seg.blocks(scheme) {
        let Some(file) = src.absolute_file(rel, from_file) else {
            continue;
        };
        let addr = BlockAddr { file, block };
        let Some(bytes) = src.block(addr) else {
            out.skipped.push((addr, Skip::Unavailable));
            continue;
        };
        let Ok(h) = BlockHeader::parse(&bytes, order) else {
            out.skipped.push((addr, Skip::Unavailable));
            continue;
        };
        if h.type_byte != 0x06 {
            out.skipped.push((addr, Skip::NotDataBlock(h.type_byte)));
            continue;
        }
        let db = match DataBlock::parse(&bytes, order) {
            Ok(db) => db,
            Err(e) => {
                out.skipped.push((addr, Skip::Malformed(e)));
                continue;
            }
        };
        if db.ktb.objd != objd {
            out.skipped.push((addr, Skip::ForeignObject(db.ktb.objd)));
            continue;
        }
        out.data_blocks = out.data_blocks.saturating_add(1);
        let rdba = h.rdba.0;
        for (slot, r) in db.rows() {
            let prov = Provenance {
                file,
                rdba,
                slot,
                scn: h.scn.0,
            };
            let piece = match r {
                Ok(p) => p,
                Err(e) => {
                    out.rows.push(Err((prov, RowError::Decode(e))));
                    continue;
                }
            };
            if piece.is_deleted() {
                continue;
            }
            match cluster {
                None => {
                    if !piece.is_head() {
                        continue; // continuation piece: reached through its head
                    }
                    let chain = if piece.next.is_some() {
                        prefetch_chain(src, from_file, &piece)
                    } else {
                        HashMap::new()
                    };
                    let pieces = MapPieces {
                        map: &chain,
                        order,
                        objd,
                    };
                    match assemble(&pieces, rdba, &piece) {
                        Ok(a) => out.rows.push(Ok(RawRow {
                            provenance: prov,
                            rowid_row: slot,
                            columns: a.columns,
                            pieces: if a.pieces.len() > 1 {
                                a.pieces
                            } else {
                                Vec::new()
                            },
                        })),
                        Err(AssembleError::Deleted) => {}
                        Err(e) => out.rows.push(Err((prov, RowError::Assemble(e)))),
                    }
                }
                Some((tab, clucols)) => {
                    if db.table_of(slot) != Some(tab) || !piece.is_cluster_member() {
                        continue;
                    }
                    let Some(ClusterPart::Member { key_index }) = piece.cluster else {
                        continue;
                    };
                    // LV-046: ROWID row number is relative to the table's kdbt entry.
                    let rowid_row = db
                        .tables
                        .get(tab)
                        .map_or(slot, |t| slot.saturating_sub(t.first));
                    match db.cluster_key(key_index) {
                        Ok(key) => match cluster_columns(&key, &piece, clucols) {
                            Ok(cols) => out.rows.push(Ok(RawRow {
                                provenance: prov,
                                rowid_row,
                                columns: cols.into_iter().map(|c| c.map(<[u8]>::to_vec)).collect(),
                                pieces: Vec::new(),
                            })),
                            Err(e) => out.rows.push(Err((prov, RowError::ClusterKey(e)))),
                        },
                        Err(e) => out.rows.push(Err((prov, RowError::ClusterKey(e)))),
                    }
                }
            }
        }
    }
    out
}
