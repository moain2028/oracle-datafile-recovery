//! Segment headers and extent maps: which blocks belong to a segment.
//!
//! Three block types carry extent maps (`lab-verify` LV-039..041):
//!
//! **Freelist-managed segment header (MSSM, `type_kcbh = 0x10`)** —
//! `ktech` (72 bytes @20), `ktemh` (16 bytes @92), `ktetb[]` @108:
//!
//! | off | field               | meaning                                  |
//! |-----|---------------------|------------------------------------------|
//! | 24  | `tsn_ktech`         | tablespace number                        |
//! | 28  | `lastmap_ktech`     | last extent-map block (0 = none)         |
//! | 32  | `mapcount_ktech`    | number of extra map blocks               |
//! | 36  | `extents_ktech`     | total extents in the segment             |
//! | 40  | `blocks_ktech`      | total blocks in the segment              |
//! | 44  | `mapend_ktech` u16  | end offset of the map area               |
//! | 92  | `count_ktemh`       | extents listed **in this block**         |
//! | 96  | `next_ktemh`        | next map block (`0x24`-style chain)      |
//! | 100 | `obj_ktemh`         | `DATA_OBJECT_ID` of the segment          |
//! | 104 | `flag_ktemh`        | flags                                    |
//! | 108 | `ktetb[i]`          | `(dba u32, nbk u32)` per extent          |
//!
//! **Pagetable segment header (ASSM, `0x23`)** — the same map header
//! `{count, next, obj, flag}` sits at @264 and the extent map starts at
//! **@280**; the block holds at most [`MAX_ENTRIES_PAGETABLE`] entries, the
//! rest live in a chain of extent-map blocks.
//!
//! **Pagetable extent-map block (EMB, `0x24`)** — `{count @20, next @24,
//! obj @28, flag @32}` then `(dba, len)` pairs from **@36**, at most
//! [`MAX_ENTRIES_EMB`] entries.
//!
//! Evidence (all real bytes, decoded in the unit tests): 10gR2 BBED dump of
//! file 1 block 377 — the segment header of `bootstrap$` itself (obj 56,
//! one extent at 1/378 × 7 blocks; itpub 17172228); 12.2 BBED dump @272 of a
//! `0x23` header (obj# `0x11e2b`, extent 0 `0x03c000a0` × 8; CSDN
//! renyanjie123 2022 — editing byte 272 changed the object id, which pins the
//! offset); two 19c `0x24` memory dumps (modb.pro 44559: `#extents 1 next 0
//! obj# 13806 flag 0x10000000`, and `#extents 8 … obj# 90171`). `next` @268
//! in `0x23` follows from the field order confirmed in both other layouts
//! (T4; `lab-verify` LV-040).
//!
//! Rules: every count is bounded by what physically fits in the block; a
//! zero `dba` or zero length ends the map; the walker rejects cycles and
//! map blocks that belong to a different object. The map is a **hint**: the
//! row reader still checks each block's own type and `objd`.

// Offsets are small constants plus `i * 8` with `i` bounded by the block
// size (< 4096), so no arithmetic here can overflow; all reads are checked.
#![allow(clippy::arithmetic_side_effects)]

use crate::block::{BlockHeader, Rdba, RdbaScheme};
use crate::bytes::{ByteOrder, read_u32};
use crate::error::{DecodeError, DecodeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const WHAT: &str = "segment header";
/// `type_kcbh` of a freelist-managed segment header.
pub const TYPE_FREELIST_HEADER: u8 = 0x10;
/// `type_kcbh` of an ASSM (pagetable) segment header.
pub const TYPE_PAGETABLE_HEADER: u8 = 0x23;
/// `type_kcbh` of an ASSM extent-map block.
pub const TYPE_PAGETABLE_EXTENT_MAP: u8 = 0x24;
/// Entries a `0x23` header can hold (published: 307 for 8 KiB blocks).
pub const MAX_ENTRIES_PAGETABLE: usize = 307;
/// Entries a `0x24` block can hold (published: 508 for 8 KiB blocks).
pub const MAX_ENTRIES_EMB: usize = 508;
/// Upper bound on extent-map blocks followed per segment (a 1 TB segment
/// with 8 MB extents has ~128 k extents ≈ 260 map blocks).
pub const MAX_MAP_BLOCKS: usize = 4096;
/// Upper bound on blocks a single segment may enumerate (guards against a
/// forged map claiming the whole file). 2^26 blocks × 8 KiB = 512 GiB.
pub const MAX_SEGMENT_BLOCKS: u64 = 1 << 26;

/// Which layout the map came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MapKind {
    /// `0x10` freelist-managed segment header.
    FreelistHeader,
    /// `0x23` pagetable (ASSM) segment header.
    PagetableHeader,
    /// `0x24` pagetable extent-map block.
    ExtentMapBlock,
}

/// One extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Extent {
    /// First block of the extent.
    pub dba: Rdba,
    /// Number of blocks.
    pub blocks: u32,
}

/// Extent map held by one block (header or map block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtentMap {
    /// Layout.
    pub kind: MapKind,
    /// `DATA_OBJECT_ID` recorded in the map header.
    pub objd: u32,
    /// Total extents of the segment (`extents_ktech`), when the layout has it.
    pub total_extents: Option<u32>,
    /// Total blocks of the segment (`blocks_ktech`), when the layout has it.
    pub total_blocks: Option<u32>,
    /// Entries declared for this block (`count`).
    pub declared: u32,
    /// Entries actually read (≤ `declared`, ≤ capacity, stops at a null entry).
    pub extents: Vec<Extent>,
    /// Next map block in the chain, if any.
    pub next: Option<Rdba>,
}

impl ExtentMap {
    /// Parse the extent map of a segment header or extent-map block,
    /// dispatching on `type_kcbh`.
    ///
    /// # Errors
    /// [`DecodeError::Malformed`] for other block types or inconsistent
    /// counts; [`DecodeError::Truncated`] for short buffers.
    pub fn parse(block: &[u8], order: ByteOrder) -> DecodeResult<Self> {
        let h = BlockHeader::parse(block, order)?;
        match h.type_byte {
            TYPE_FREELIST_HEADER => Self::parse_at(block, order, MapKind::FreelistHeader, 92, 108),
            TYPE_PAGETABLE_HEADER => {
                Self::parse_at(block, order, MapKind::PagetableHeader, 264, 280)
            }
            TYPE_PAGETABLE_EXTENT_MAP => {
                Self::parse_at(block, order, MapKind::ExtentMapBlock, 20, 36)
            }
            _ => Err(DecodeError::malformed(
                WHAT,
                0,
                "block type carries no extent map",
            )),
        }
    }

    /// Shared body: map header `{count, next, obj, flag}` at `hdr`, entries at `map`.
    fn parse_at(
        block: &[u8],
        order: ByteOrder,
        kind: MapKind,
        hdr: usize,
        map: usize,
    ) -> DecodeResult<Self> {
        let declared = read_u32(block, hdr, order)?;
        let next = Rdba(read_u32(block, hdr + 4, order)?);
        let objd = read_u32(block, hdr + 8, order)?;
        let (total_extents, total_blocks) = if kind == MapKind::FreelistHeader {
            (
                Some(read_u32(block, 36, order)?),
                Some(read_u32(block, 40, order)?),
            )
        } else {
            (None, None)
        };
        let capacity = match kind {
            MapKind::FreelistHeader => block.len().saturating_sub(map + 4) / 8,
            MapKind::PagetableHeader => MAX_ENTRIES_PAGETABLE,
            MapKind::ExtentMapBlock => MAX_ENTRIES_EMB,
        }
        .min(block.len().saturating_sub(map + 4) / 8);
        let n = usize::try_from(declared).unwrap_or(usize::MAX);
        if n > capacity {
            return Err(DecodeError::malformed(
                WHAT,
                hdr,
                "declared extent count exceeds block capacity",
            ));
        }
        if let Some(t) = total_extents
            && declared > t
        {
            return Err(DecodeError::malformed(
                WHAT,
                hdr,
                "map count exceeds total extent count",
            ));
        }
        let mut extents = Vec::with_capacity(n);
        for i in 0..n {
            let o = map + 8 * i;
            let dba = Rdba(read_u32(block, o, order)?);
            let blocks = read_u32(block, o + 4, order)?;
            if dba.is_null() || blocks == 0 {
                break;
            }
            extents.push(Extent { dba, blocks });
        }
        Ok(Self {
            kind,
            objd,
            total_extents,
            total_blocks,
            declared,
            extents,
            next: (!next.is_null()).then_some(next),
        })
    }
}

/// Why a segment could not be fully enumerated. Partial results are still
/// returned alongside the error so damaged segments remain readable
/// (REQ-004.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum WalkIssue {
    /// A map block was not available from the source.
    MissingMapBlock(Rdba),
    /// A map block did not decode.
    BadMapBlock(Rdba, DecodeError),
    /// A map block names a different object than the header.
    ForeignMapBlock {
        /// Block address.
        rdba: Rdba,
        /// `objd` found there.
        found: u32,
        /// `objd` expected.
        expected: u32,
    },
    /// The `next` chain revisits a block.
    Cycle(Rdba),
    /// More than [`MAX_MAP_BLOCKS`] map blocks.
    TooManyMapBlocks,
    /// More than [`MAX_SEGMENT_BLOCKS`] blocks claimed.
    TooManyBlocks,
    /// The header said `total_extents` but the chain listed a different number.
    ExtentCountMismatch {
        /// From the header.
        declared: u32,
        /// Counted across the chain.
        found: usize,
    },
}

/// The enumerated blocks of one segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SegmentBlocks {
    /// `DATA_OBJECT_ID` from the header.
    pub objd: u32,
    /// Extents in map order.
    pub extents: Vec<Extent>,
    /// Problems met while walking (the result is partial if non-empty).
    pub issues: Vec<WalkIssue>,
}

impl SegmentBlocks {
    /// Every `(relative file, block)` of the segment in extent order under
    /// `scheme`. For bigfile the file is `None` (single file). The segment
    /// header block itself is included when it lies inside an extent (it
    /// does for both MSSM and ASSM); the reader filters by block type.
    pub fn blocks(&self, scheme: RdbaScheme) -> impl Iterator<Item = (Option<u32>, u32)> + '_ {
        self.extents.iter().flat_map(move |e| {
            let (file, first) = e.dba.split(scheme);
            (0..e.blocks).map_while(move |i| first.checked_add(i).map(|b| (file, b)))
        })
    }

    /// Total number of blocks.
    pub fn len(&self) -> u64 {
        self.extents.iter().map(|e| u64::from(e.blocks)).sum()
    }

    /// Whether no extent was found.
    pub fn is_empty(&self) -> bool {
        self.extents.is_empty()
    }
}

/// Walk a segment's extent map starting from its header block. `fetch`
/// returns the bytes of a block by rdba (`None` if unavailable).
///
/// Never fails outright: problems are reported in `issues` and whatever was
/// enumerated before them is returned, except when the header itself does
/// not decode (then `Err`).
///
/// # Errors
/// If `header` is not a segment header with an extent map.
pub fn walk_segment<F>(
    header: &[u8],
    order: ByteOrder,
    header_rdba: Rdba,
    mut fetch: F,
) -> DecodeResult<SegmentBlocks>
where
    F: FnMut(Rdba) -> Option<Vec<u8>>,
{
    let first = ExtentMap::parse(header, order)?;
    let objd = first.objd;
    let mut out = SegmentBlocks {
        objd,
        extents: first.extents.clone(),
        issues: Vec::new(),
    };
    let mut seen: HashSet<u32> = HashSet::from([header_rdba.0]);
    let mut next = first.next;
    let mut maps = 1usize;
    while let Some(rdba) = next {
        if maps >= MAX_MAP_BLOCKS {
            out.issues.push(WalkIssue::TooManyMapBlocks);
            break;
        }
        if !seen.insert(rdba.0) {
            out.issues.push(WalkIssue::Cycle(rdba));
            break;
        }
        let Some(bytes) = fetch(rdba) else {
            out.issues.push(WalkIssue::MissingMapBlock(rdba));
            break;
        };
        let map = match ExtentMap::parse(&bytes, order) {
            Ok(m) if m.kind == MapKind::ExtentMapBlock => m,
            Ok(_) => {
                out.issues.push(WalkIssue::BadMapBlock(
                    rdba,
                    DecodeError::malformed(WHAT, 0, "chained block is not an extent-map block"),
                ));
                break;
            }
            Err(e) => {
                out.issues.push(WalkIssue::BadMapBlock(rdba, e));
                break;
            }
        };
        if map.objd != objd {
            out.issues.push(WalkIssue::ForeignMapBlock {
                rdba,
                found: map.objd,
                expected: objd,
            });
            break;
        }
        out.extents.extend(map.extents);
        next = map.next;
        maps += 1;
        if out.len() > MAX_SEGMENT_BLOCKS {
            out.issues.push(WalkIssue::TooManyBlocks);
            break;
        }
    }
    if out.len() > MAX_SEGMENT_BLOCKS {
        out.extents.clear();
        out.issues.push(WalkIssue::TooManyBlocks);
    }
    if let Some(t) = first.total_extents
        && out.issues.is_empty()
        && usize::try_from(t).ok() != Some(out.extents.len())
    {
        out.issues.push(WalkIssue::ExtentCountMismatch {
            declared: t,
            found: out.extents.len(),
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let h: String = s.chars().filter(char::is_ascii_hexdigit).collect();
        (0..h.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
            .collect()
    }

    /// 10gR2, file 1 block 377 = segment header of `bootstrap$` (first 128
    /// bytes verbatim from the published BBED dump; the rest of the 512-byte
    /// dump was zero).
    const BOOT_SEG_HDR_10G: &str = "\
10a20000 79014000 d3000000 00000204 33e70000 00000000 00000000 00000000 \
00000000 01000000 07000000 20100000 00000000 03000000 07000000 7d014000 \
00000000 00000000 01000000 03000000 00000000 00000000 00000000 01000000 \
00000000 38000000 00000040 7a014000 07000000 00000000 00000000 00000000";

    fn boot_hdr() -> Vec<u8> {
        let mut b = hex(BOOT_SEG_HDR_10G);
        b.resize(8192, 0);
        b
    }

    #[test]
    fn golden_10g_bootstrap_segment_header() {
        let m = ExtentMap::parse(&boot_hdr(), ByteOrder::Little).unwrap();
        assert_eq!(m.kind, MapKind::FreelistHeader);
        assert_eq!(m.objd, 56, "bootstrap$ obj# in 10g (LV-034)");
        assert_eq!((m.total_extents, m.total_blocks), (Some(1), Some(7)));
        assert_eq!(m.declared, 1);
        assert_eq!(m.next, None);
        assert_eq!(
            m.extents,
            vec![Extent {
                dba: Rdba(0x0040_017A),
                blocks: 7
            }]
        );
        assert_eq!(
            (m.extents[0].dba.file(), m.extents[0].dba.block()),
            (1, 378),
            "first data block = the golden bootstrap$ block of dict_bootstrap.rs"
        );
        let w = walk_segment(&boot_hdr(), ByteOrder::Little, Rdba(0x0040_0179), |_| None).unwrap();
        assert!(w.issues.is_empty(), "{:?}", w.issues);
        let blocks: Vec<_> = w.blocks(RdbaScheme::Smallfile).collect();
        assert_eq!(blocks.len(), 7);
        assert_eq!(blocks[0], (Some(1), 378));
        assert_eq!(blocks[6], (Some(1), 384));
    }

    /// 12.2 `0x23` header: published bytes at offsets 272..287 (obj#, flag,
    /// extent 0). Bytes 0..272 are reconstructed (kcbh + the map header count).
    #[test]
    fn golden_pagetable_header_offsets() {
        let mut b = vec![0u8; 8192];
        b[0] = TYPE_PAGETABLE_HEADER;
        b[1] = 0xA2;
        b[264..268].copy_from_slice(&27u32.to_le_bytes());
        b[272..288].copy_from_slice(&hex("2b1e0100 00000010 a000c003 08000000"));
        // Fill the remaining 26 entries so the declared count is honoured.
        for i in 1..27u32 {
            let o = 280 + 8 * i as usize;
            b[o..o + 4].copy_from_slice(&(0x03C0_00A0 + 128 * i).to_le_bytes());
            b[o + 4..o + 8].copy_from_slice(&128u32.to_le_bytes());
        }
        let m = ExtentMap::parse(&b, ByteOrder::Little).unwrap();
        assert_eq!(m.kind, MapKind::PagetableHeader);
        assert_eq!(m.objd, 73259, "0x11e2b at @272");
        assert_eq!(m.extents.len(), 27);
        assert_eq!(
            m.extents[0],
            Extent {
                dba: Rdba(0x03C0_00A0),
                blocks: 8
            }
        );
        assert_eq!(m.total_extents, None);
    }

    /// 19c `0x24` extent-map block: first 48 bytes verbatim from a published
    /// memory dump (words printed as little-endian u32).
    fn emb(words: &[u32]) -> Vec<u8> {
        let mut b = Vec::new();
        for w in words {
            b.extend(w.to_le_bytes());
        }
        b.resize(8192, 0);
        b
    }

    #[test]
    fn golden_emb_blocks() {
        let one = emb(&[
            0x0000_A224,
            0x014F_8088,
            0x0009_A484,
            0x0402_0000,
            0x0000_3D24,
            1,
            0,
            0x35EE,
            0x1000_0000,
            0x014F_8080,
            0x2000,
            0,
        ]);
        let m = ExtentMap::parse(&one, ByteOrder::Little).unwrap();
        assert_eq!(m.kind, MapKind::ExtentMapBlock);
        assert_eq!((m.objd, m.declared, m.next), (13806, 1, None));
        assert_eq!(
            m.extents,
            vec![Extent {
                dba: Rdba(0x014F_8080),
                blocks: 8192
            }]
        );
        let eight = emb(&[
            0x0000_A224,
            0x01B1_2088,
            0x0081_4AFB,
            0x0401_0000,
            0x0000_9694,
            8,
            0,
            0x0001_603B,
            0x1000_0000,
            0x01B1_2080,
            0x2000,
            0x01B1_4080,
            0x2000,
            0x01B1_6080,
            0x2000,
            0x01B1_8080,
            0x2000,
            0x01B1_A080,
            0x2000,
            0x01B1_C080,
            0x2000,
            0x01B1_E080,
            0x2000,
            0x01B2_0080,
            0x2000,
        ]);
        let m = ExtentMap::parse(&eight, ByteOrder::Little).unwrap();
        assert_eq!((m.objd, m.extents.len()), (90171, 8));
        assert_eq!(m.extents[7].dba, Rdba(0x01B2_0080));
    }

    #[test]
    fn walker_follows_chain_and_rejects_cycles_and_foreign_blocks() {
        // Header (0x10) with next -> A ; A (0x24) next -> B ; B next -> A (cycle).
        let mut h = boot_hdr();
        h[96..100].copy_from_slice(&0x0040_1000u32.to_le_bytes());
        h[36..40].copy_from_slice(&3u32.to_le_bytes()); // total extents 3
        let a = emb(&[
            0xA224,
            0x0040_1000,
            0,
            0,
            0,
            1,
            0x0040_1001,
            56,
            0,
            0x0040_2000,
            4,
            0,
        ]);
        let b = emb(&[
            0xA224,
            0x0040_1001,
            0,
            0,
            0,
            1,
            0x0040_1000,
            56,
            0,
            0x0040_3000,
            4,
            0,
        ]);
        let fetch = |r: Rdba| match r.0 {
            0x0040_1000 => Some(a.clone()),
            0x0040_1001 => Some(b.clone()),
            _ => None,
        };
        let w = walk_segment(&h, ByteOrder::Little, Rdba(0x0040_0179), fetch).unwrap();
        assert_eq!(w.extents.len(), 3);
        assert_eq!(w.issues, vec![WalkIssue::Cycle(Rdba(0x0040_1000))]);

        // Foreign map block (different objd) stops the walk and is reported.
        let foreign = emb(&[0xA224, 0x0040_1000, 0, 0, 0, 1, 0, 57, 0, 0x0040_2000, 4, 0]);
        let w = walk_segment(&h, ByteOrder::Little, Rdba(0x0040_0179), |_| {
            Some(foreign.clone())
        })
        .unwrap();
        assert_eq!(w.extents.len(), 1);
        assert!(matches!(
            w.issues[0],
            WalkIssue::ForeignMapBlock {
                found: 57,
                expected: 56,
                ..
            }
        ));

        // Missing map block.
        let w = walk_segment(&h, ByteOrder::Little, Rdba(0x0040_0179), |_| None).unwrap();
        assert_eq!(
            w.issues,
            vec![WalkIssue::MissingMapBlock(Rdba(0x0040_1000))]
        );
    }

    #[test]
    fn bounds_are_enforced() {
        let mut h = boot_hdr();
        h[92..96].copy_from_slice(&5000u32.to_le_bytes());
        assert!(
            ExtentMap::parse(&h, ByteOrder::Little).is_err(),
            "count > capacity"
        );
        let mut h = boot_hdr();
        h[92..96].copy_from_slice(&2u32.to_le_bytes()); // declared 2 > total 1
        assert!(ExtentMap::parse(&h, ByteOrder::Little).is_err());
        assert!(ExtentMap::parse(&[0u8; 8192], ByteOrder::Little).is_err());
        assert!(ExtentMap::parse(&boot_hdr()[..100], ByteOrder::Little).is_err());
        // A forged extent claiming 2^32-1 blocks is refused by the walker.
        let mut h = boot_hdr();
        h[112..116].copy_from_slice(&u32::MAX.to_le_bytes());
        h[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        let w = walk_segment(&h, ByteOrder::Little, Rdba(0x0040_0179), |_| None).unwrap();
        assert_eq!(w.extents, Vec::new());
        assert!(w.issues.contains(&WalkIssue::TooManyBlocks));
    }
}
