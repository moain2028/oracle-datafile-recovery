//! Row-piece decoding.
//!
//! A heap-table row piece is (all lengths in bytes):
//!
//! ```text
//! flag(1) lock(1) cc(1) [dscn: wrap(2) base(4)] [nrid: dba(4) slot(2)] [hrid: dba(4) slot(2)] col_1 … col_cc
//! ```
//!
//! * `dscn` (row-dependency SCN) is present on every row of a block whose
//!   `kdbh` flag has [`super::kdb::kdbh_flag::ROW_DEPENDENCIES`] (tables
//!   created `ROWDEPENDENCIES`); all zero while the commit SCN still lives in
//!   the ITL — LV-018/019. **Ignoring it would shift every column by six
//!   bytes**, which is why the block flag is threaded through [`PieceFormat`].
//! * `nrid` is present when the piece is **not** the last one (flag `L`
//!   clear) and points at the next piece (chained / migrated rows) — LV-008.
//! * `hrid` is present on the destination piece of a migrated row: flag `F`
//!   set, `H` clear — LV-010.
//! * Column length byte: `0..=250` = length, `0xFE` = next two bytes hold the
//!   length **in the file byte order** (LV-009 — *refuted* for the published
//!   "always big-endian" claim: in a real 26ai little-endian datafile every
//!   one of 21 `bootstrap$` rows with a > 250-byte `SQL_TEXT` ends exactly
//!   where the next row starts only under little-endian; big-endian would
//!   overrun the block), `0xFF` = NULL; `0xFB..=0xFD` never occur and are
//!   rejected.
//! * Trailing NULL columns are not stored: `cc` counts stored columns only.
//! * Cluster pieces (`K`/`C` flags) carry their own header — see
//!   [`super::cluster`]. Combinations without a published layout (chained
//!   cluster pieces, cluster rows with row dependencies, a migrated/chained
//!   piece with row dependencies) are [`DecodeError::Unsupported`]: never a
//!   guess.
//!
//! Sources: Oracle Concepts \[O1\]; Lewis \[W6\]; Widlake \[W7\]; oradul \[G1\];
//! row-dependency BBED dump (aliyun, 2012); cluster sources in [`super::cluster`].

use super::cluster::ClusterPart;
use super::{RowPiece, flag};
use crate::block::Scn;
use crate::bytes::{ByteOrder, Cursor, read_u8};
use crate::error::{DecodeError, DecodeResult};

const WHAT: &str = "row piece";

/// Block-level facts that change the row-piece layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PieceFormat {
    /// Every row carries a 6-byte `dscn` after `cc` (`kdbh` flag `0x20`).
    pub row_dependencies: bool,
}

/// Header fields between `cc` and the first column.
struct Header {
    cluster: Option<ClusterPart>,
    next: Option<(u32, u16)>,
    head: Option<(u32, u16)>,
    dscn: Option<Scn>,
}

fn rid(c: &mut Cursor<'_>) -> DecodeResult<(u32, u16)> {
    Ok((c.u32()?, c.u16()?))
}

/// Cluster key / member header (after `cc`).
fn cluster_header(
    c: &mut Cursor<'_>,
    fl: u8,
    off: usize,
    fmt: PieceFormat,
) -> DecodeResult<Header> {
    let is_key = fl & flag::K != 0;
    if is_key && fl & flag::C != 0 {
        return Err(DecodeError::malformed(
            WHAT,
            off,
            "both cluster-key and cluster-member flags set",
        ));
    }
    if fl & (flag::H | flag::F | flag::L) != flag::H | flag::F | flag::L
        || fl & (flag::P | flag::N) != 0
    {
        return Err(DecodeError::Unsupported("chained cluster row piece"));
    }
    if fmt.row_dependencies {
        return Err(DecodeError::Unsupported(
            "cluster row piece with row dependencies",
        ));
    }
    let part = if is_key {
        let current = c.u16()?;
        let committed = c.u16()?;
        let prev = rid(c)?;
        let next = rid(c)?;
        ClusterPart::Key {
            current,
            committed,
            prev,
            next,
        }
    } else {
        ClusterPart::Member { key_index: c.u8()? }
    };
    Ok(Header {
        cluster: Some(part),
        next: None,
        head: None,
        dscn: None,
    })
}

/// Heap-table header (after `cc`): optional `dscn`, `nrid`, `hrid`.
fn heap_header(c: &mut Cursor<'_>, fl: u8, fmt: PieceFormat) -> DecodeResult<Header> {
    let has_next = fl & flag::L == 0;
    let has_head = fl & flag::F != 0 && fl & flag::H == 0;
    let dscn = if fmt.row_dependencies {
        if has_next || has_head {
            return Err(DecodeError::Unsupported(
                "chained or migrated row piece with row dependencies",
            ));
        }
        let wrap = c.u16()?;
        let base = c.u32()?;
        Some(Scn::new(wrap, base))
    } else {
        None
    };
    let next = if has_next { Some(rid(c)?) } else { None };
    let head = if has_head { Some(rid(c)?) } else { None };
    Ok(Header {
        cluster: None,
        next,
        head,
        dscn,
    })
}

/// `cc` column values starting at the cursor.
fn columns<'a>(c: &mut Cursor<'a>, buf: &'a [u8], cc: u8) -> DecodeResult<Vec<Option<&'a [u8]>>> {
    let mut out = Vec::with_capacity(usize::from(cc));
    for _ in 0..cc {
        let pos = c.position();
        let len = match read_u8(buf, pos)? {
            0xFF => {
                c.skip(1)?;
                out.push(None);
                continue;
            }
            0xFE => {
                c.skip(1)?;
                usize::from(c.u16()?)
            }
            l @ 0..=250 => {
                c.skip(1)?;
                usize::from(l)
            }
            _ => {
                return Err(DecodeError::malformed(
                    WHAT,
                    pos,
                    "reserved length byte 0xFB..=0xFD",
                ));
            }
        };
        out.push(Some(c.take(len)?));
    }
    Ok(out)
}

/// Parse the row piece starting at `off` in `heap` (block bytes up to, not
/// including, the tail check — a piece can never extend into it).
///
/// # Errors
/// [`DecodeError`] if the piece is malformed, runs past the heap, or uses a
/// layout combination that is not documented yet.
pub fn parse_piece(
    heap: &[u8],
    off: usize,
    slot: u16,
    order: ByteOrder,
    fmt: PieceFormat,
) -> DecodeResult<RowPiece<'_>> {
    let mut c = Cursor::new(heap, order);
    c.seek(off)?;
    let fl = c.u8()?;
    let lock = c.u8()?;
    let cc = c.u8()?;
    let h = if fl & (flag::K | flag::C) != 0 {
        cluster_header(&mut c, fl, off, fmt)?
    } else {
        heap_header(&mut c, fl, fmt)?
    };
    let cols = columns(&mut c, heap, cc)?;
    let len = c.position().saturating_sub(off);
    Ok(RowPiece {
        slot,
        flag: fl,
        lock,
        columns: cols,
        next: h.next,
        head: h.head,
        dscn: h.dscn,
        cluster: h.cluster,
        len,
    })
}
