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
//!   Both `nrid` and `hrid` are **big-endian on every platform** (LV-049,
//!   verified on a 26ai little-endian file) — unlike the `0xFE` length word.
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
    /// Every row carries a `dscn` after `cc` (`kdbh` flag `0x20`).
    pub row_dependencies: bool,
    /// Width of that `dscn`: 6 bytes (wrap u16 + base u32) in the published
    /// 11g dumps, **7 bytes** on 26ai (LV-052: `00 80 00 00 00 00 00` before
    /// the first column in every `T_ROWDEP` row). [`super::DataBlock::parse`]
    /// settles it per block from the row-packing invariant; never guessed.
    pub dscn_len: u8,
}

impl PieceFormat {
    /// Plain heap rows (no `dscn`).
    pub const PLAIN: Self = Self {
        row_dependencies: false,
        dscn_len: 6,
    };
}

/// Header fields between `cc` and the first column.
struct Header {
    cluster: Option<ClusterPart>,
    next: Option<(u32, u16)>,
    head: Option<(u32, u16)>,
    dscn: Option<Scn>,
}

/// `nrid` / `hrid`: dba (4) + slot (2), **always big-endian** — independent
/// of the file byte order (LV-049: 26ai little-endian file stores
/// `04 00 05 A8 00 00` for 16/1448 slot 0; a little-endian read would give
/// file 672 block 0x4004).
fn rid(c: &mut Cursor<'_>) -> DecodeResult<(u32, u16)> {
    let b = c.take(6)?;
    let dba = u32::from_be_bytes([
        *b.first().unwrap_or(&0),
        *b.get(1).unwrap_or(&0),
        *b.get(2).unwrap_or(&0),
        *b.get(3).unwrap_or(&0),
    ]);
    let slot = u16::from_be_bytes([*b.get(4).unwrap_or(&0), *b.get(5).unwrap_or(&0)]);
    Ok((dba, slot))
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
    // LV-057 (26ai SYSTEM, C_OBJ#/C_USER#): a *migrated* cluster member is
    // stored with flags `F L` (no `H`) and, after `cc`, `cki` followed by a
    // 6-byte `hrid` (big-endian) pointing at the head stub. Its columns are
    // complete and its key row is the local `cki` one, so it is read like
    // any member; only the stub (H without F/L) is skipped. Anything else
    // (a member split across blocks: `P`/`N`, or `F` without `L`) stays
    // unsupported rather than guessed.
    let migrated_member = !is_key && fl & (flag::H | flag::F | flag::L) == flag::F | flag::L;
    if !(migrated_member || fl & (flag::H | flag::F | flag::L) == flag::H | flag::F | flag::L)
        || fl & (flag::P | flag::N) != 0
    {
        return Err(DecodeError::Unsupported("chained cluster row piece"));
    }
    if fmt.row_dependencies {
        return Err(DecodeError::Unsupported(
            "cluster row piece with row dependencies",
        ));
    }
    let mut head = None;
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
        let key_index = c.u8()?;
        if migrated_member {
            head = Some(rid(c)?);
        }
        ClusterPart::Member { key_index }
    };
    Ok(Header {
        cluster: Some(part),
        next: None,
        head,
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
        // Extra byte(s) beyond the 6-byte SCN (LV-052); their meaning is not
        // established, so they are skipped, not interpreted.
        c.skip(usize::from(fmt.dscn_len.saturating_sub(6)))?;
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
