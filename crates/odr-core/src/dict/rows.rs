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
use crate::block::{BlockHeader, BlockStatus, Rdba, classify};
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
    /// Block is zeroed or truncated (LV-059).
    Integrity(BlockStatus),
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
    /// Integrity of the block(s) the row was read from (LV-059). Anything
    /// but [`BlockStatus::Valid`] means the bytes may be altered even though
    /// they decode: the caller must not label such values *verified*.
    pub block_status: BlockStatus,
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
    /// The slot sits in a block that failed its integrity check and the
    /// piece could not be decoded or was flagged deleted / continuation —
    /// flags that cannot be trusted there (LV-061). The slot is reported,
    /// never dropped, so every directory slot of a damaged block is
    /// accounted for.
    Integrity(BlockStatus),
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

/// The worse of two block statuses (`Valid` is best).
fn worst(a: BlockStatus, b: BlockStatus) -> BlockStatus {
    if a == BlockStatus::Valid { b } else { a }
}

/// Worst integrity status among the continuation blocks of a chain.
fn chain_status(chain: &HashMap<u32, Vec<u8>>, order: ByteOrder) -> BlockStatus {
    chain
        .values()
        .map(|b| classify(b, b.len(), order))
        .fold(BlockStatus::Valid, worst)
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
        // LV-059: integrity first. A block whose checksum or tail check
        // fails is still decoded (REQ-004.1: partial damage is data too)
        // but every row from it is tagged so it cannot be reported as
        // verified — 26ai bitrot lab: 3 of 20,000 rows decoded cleanly
        // with one wrong byte inside a VARCHAR2.
        let status = classify(&bytes, bytes.len(), order);
        if matches!(status, BlockStatus::Zeroed | BlockStatus::Truncated) {
            out.skipped.push((addr, Skip::Integrity(status)));
            continue;
        }
        // LV-061: in a block that failed integrity the type byte, the row
        // flags and the row directory are themselves suspect. Such a block
        // is either enumerated slot by slot (every slot reported) or, when
        // it does not even parse, skipped *as an integrity failure* — never
        // as "not a data block (type 0x96)" from a garbage byte (ransom
        // lab) and never with rows silently dropped (torn lab: flag bytes
        // 0x15 `D L N` and 0x0B `F P N` hid two rows).
        let trusted = status == BlockStatus::Valid;
        let Ok(h) = BlockHeader::parse(&bytes, order) else {
            out.skipped.push((
                addr,
                if trusted {
                    Skip::Unavailable
                } else {
                    Skip::Integrity(status)
                },
            ));
            continue;
        };
        if h.type_byte != 0x06 {
            out.skipped.push((
                addr,
                if trusted {
                    Skip::NotDataBlock(h.type_byte)
                } else {
                    Skip::Integrity(status)
                },
            ));
            continue;
        }
        let db = match DataBlock::parse(&bytes, order) {
            Ok(db) => db,
            Err(e) => {
                out.skipped.push((
                    addr,
                    if trusted {
                        Skip::Malformed(e)
                    } else {
                        Skip::Integrity(status)
                    },
                ));
                continue;
            }
        };
        if db.ktb.objd != objd {
            out.skipped.push((addr, Skip::ForeignObject(db.ktb.objd)));
            continue;
        }
        out.data_blocks = out.data_blocks.saturating_add(1);
        let rdba = h.rdba.0;
        // A trusted block iterates its used slots; an untrusted one iterates
        // every directory slot, so that nothing can hide behind an "unused"
        // offset or a flag byte (LV-061).
        let slots: Vec<(u16, Result<Option<RowPiece<'_>>, DecodeError>)> = if trusted {
            db.rows().map(|(s, r)| (s, r.map(Some))).collect()
        } else {
            (0..db.kdbh.nrow).map(|s| (s, db.row(s))).collect()
        };
        for (slot, r) in slots {
            let prov = Provenance {
                file,
                rdba,
                slot,
                scn: h.scn.0,
            };
            // Reason used for every flag-based skip inside an untrusted block.
            let integrity = || Err((prov, RowError::Integrity(status)));
            let piece = match r {
                Ok(Some(p)) => p,
                Ok(None) => {
                    // Unused slot: nothing in a trusted block; in an
                    // untrusted block the directory itself is suspect.
                    if !trusted {
                        out.rows.push(integrity());
                    }
                    continue;
                }
                Err(e) => {
                    // LV-058: a row whose flag byte says *deleted* (`D`) may
                    // have had its bytes partly reused by a newer row (26ai
                    // OBJ$ block 5265: slot 22 deleted, slot 21 written 2
                    // bytes above it). Oracle itself never reads it; it is
                    // not an error of the block, so it is skipped silently.
                    // Only in a block that passed integrity: in a torn
                    // block the flag byte itself may be garbage, and the
                    // row must then be *rejected*, never dropped (LV-061).
                    if !trusted {
                        out.rows.push(integrity());
                    } else if db
                        .row_flag(slot)
                        .is_some_and(|f| f & crate::row::flag::D != 0)
                    {
                        // skipped silently (LV-058)
                    } else {
                        out.rows.push(Err((prov, RowError::Decode(e))));
                    }
                    continue;
                }
            };
            if piece.is_deleted() {
                if !trusted {
                    out.rows.push(integrity());
                }
                continue;
            }
            match cluster {
                None => {
                    if !piece.is_head() {
                        // continuation piece: reached through its head —
                        // unless the flag that says so is untrusted.
                        if !trusted {
                            out.rows.push(integrity());
                        }
                        continue;
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
                            block_status: worst(status, chain_status(&chain, order)),
                            columns: a.columns,
                            pieces: if a.pieces.len() > 1 {
                                a.pieces
                            } else {
                                Vec::new()
                            },
                        })),
                        Err(AssembleError::Deleted) => {
                            if !trusted {
                                out.rows.push(integrity());
                            }
                        }
                        Err(e) if trusted => {
                            out.rows.push(Err((prov, RowError::Assemble(e))));
                        }
                        Err(_) => out.rows.push(integrity()),
                    }
                }
                Some((tab, clucols)) => {
                    // Table membership comes from `kdbt`, not from the row's
                    // flag byte: a slot of another table is never this
                    // table's row, trusted or not.
                    if db.table_of(slot) != Some(tab) {
                        continue;
                    }
                    let Some(ClusterPart::Member { key_index }) = piece.cluster else {
                        if !trusted {
                            out.rows.push(integrity());
                        }
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
                                block_status: status,
                                columns: cols.into_iter().map(|c| c.map(<[u8]>::to_vec)).collect(),
                                pieces: Vec::new(),
                            })),
                            Err(e) if trusted => {
                                out.rows.push(Err((prov, RowError::ClusterKey(e))));
                            }
                            Err(_) => out.rows.push(integrity()),
                        },
                        Err(e) if trusted => {
                            out.rows.push(Err((prov, RowError::ClusterKey(e))));
                        }
                        Err(_) => out.rows.push(integrity()),
                    }
                }
            }
        }
    }
    out
}
