//! Data-block layer (`ktbbh` → `kdbh` → `kdbt` → `kdbr`) and row pieces.
//! **Owner: Lead** (`lead/*`).
//!
//! Contract (stable — change only via ADR):
//! * [`RowPiece`] borrows from the block buffer (zero-copy).
//! * Column values are raw bytes; typed decoding is in [`crate::types`].
//! * Malformed directories/rows yield `Err` for that row only, never for the
//!   whole block.

use crate::block::Scn;
use serde::{Deserialize, Serialize};

pub mod assemble;
pub mod cluster;
pub mod kdb;
pub mod ktb;
pub mod piece;

pub use assemble::{AssembleError, AssembledRow, PieceSource, assemble};
pub use cluster::{ClusterPart, cluster_columns};
pub use kdb::{DataBlock, Layout, Slot, StructuralFault, kdbh_flag};
pub use ktb::data_object_id;
pub use piece::PieceFormat;

/// Row-piece flag bits (`flag` byte, \[O1\], \[W6\]).
pub mod flag {
    /// Cluster key.
    pub const K: u8 = 0x80;
    /// Cluster member.
    pub const C: u8 = 0x40;
    /// Head piece of row.
    pub const H: u8 = 0x20;
    /// Deleted.
    pub const D: u8 = 0x10;
    /// First data piece.
    pub const F: u8 = 0x08;
    /// Last data piece.
    pub const L: u8 = 0x04;
    /// First column continues from previous piece.
    pub const P: u8 = 0x02;
    /// Last column continues in next piece.
    pub const N: u8 = 0x01;
}

/// One row piece inside a data block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowPiece<'a> {
    /// Slot number in the row directory.
    pub slot: u16,
    /// Flag byte (see [`flag`]).
    pub flag: u8,
    /// ITL lock byte.
    pub lock: u8,
    /// Column values; `None` = NULL.
    pub columns: Vec<Option<&'a [u8]>>,
    /// Next-piece pointer (rdba, slot) when the row continues elsewhere.
    pub next: Option<(u32, u16)>,
    /// Head-piece pointer on the destination piece of a migrated row.
    pub head: Option<(u32, u16)>,
    /// Row-dependency SCN (`ROWDEPENDENCIES` tables only; zero while the
    /// commit SCN is still held by the ITL).
    pub dscn: Option<Scn>,
    /// Cluster header for indexed-cluster key / member rows.
    pub cluster: Option<cluster::ClusterPart>,
    /// Total encoded length of the piece in bytes.
    pub len: usize,
}

impl RowPiece<'_> {
    /// Whether the row was deleted (flag `D`).
    pub const fn is_deleted(&self) -> bool {
        self.flag & flag::D != 0
    }
    /// Whether this piece is the head of the row (flag `H`).
    pub const fn is_head(&self) -> bool {
        self.flag & flag::H != 0
    }
    /// Whether this is a cluster-key row (flag `K`).
    pub const fn is_cluster_key(&self) -> bool {
        self.flag & flag::K != 0
    }
    /// Whether this is a cluster member row (flag `C`).
    pub const fn is_cluster_member(&self) -> bool {
        self.flag & flag::C != 0
    }
    /// Whether the row is complete in this single piece (`H`, `F` and `L`).
    pub const fn is_complete(&self) -> bool {
        self.flag & (flag::H | flag::F | flag::L) == flag::H | flag::F | flag::L
    }
}

/// Provenance of an extracted row (REQ-003.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Provenance {
    /// Index of the evidence file in the case.
    pub file: u32,
    /// Block RDBA.
    pub rdba: u32,
    /// Row directory slot.
    pub slot: u16,
    /// Block SCN.
    pub scn: u64,
}
