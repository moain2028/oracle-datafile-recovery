//! Block size and byte order of a datafile — from its own header when
//! present, inferred from the blocks themselves when it is not (REQ-001.3).
//!
//! Three independent witnesses, tried in order; each is recorded so the
//! report can say *how* the layout was decided:
//!
//! 1. **`kcvfh`** (block 1) — must parse at some legal block size / byte
//!    order *and* declare that same size (LV-073).
//! 2. **OS block 0** — on every 26ai file measured (LV-076): `u32 @20` =
//!    block size, `u32 @24` = block count, `u32 @28` = `0x7A7B7C7D`.
//! 3. **`kcbh` signatures** — sample windows across the file and, for every
//!    candidate `(size, order)`, count blocks whose header parses with
//!    Oracle's format nibble, whose tail matches, **and whose `rdba` names
//!    their own position**. A wrong size doubles or halves the index so the
//!    `rdba` test fails; a wrong byte order fails it too. The winner needs a
//!    clear margin or the file is refused (no silent guess).

use crate::block::{BLOCK_SIZES, BlockHeader, KCBH_LEN, RdbaScheme, TAIL_LEN};
use crate::bytes::{ByteOrder, read_u32};
use crate::dict::kcvfh::FileHeader;
use serde::{Deserialize, Serialize};

/// Magic at offset 28 of OS block 0 (LV-076, `lab-verify` on 11g/21c).
pub const BLOCK0_MAGIC: u32 = 0x7A7B_7C7D;

/// Which witness decided the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutSource {
    /// Block 1 (`kcvfh`) parsed and declared the size.
    FileHeader,
    /// Block 1 unusable; block 0 carried the magic and a legal size.
    OsBlock,
    /// Blocks 0 and 1 unusable; inferred from `kcbh` signatures.
    Inferred,
    /// Given on the command line with `--force`.
    Forced,
}

/// The decided layout and the evidence behind it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// Block size in bytes.
    pub block_size: usize,
    /// Byte order.
    pub order: ByteOrder,
    /// Who decided.
    pub source: LayoutSource,
    /// Block count declared by block 0 (`@24`), if block 0 was readable.
    pub declared_blocks_os: Option<u32>,
    /// Block count declared by `kcvfh` (`kccfhfsz`), if block 1 parsed.
    pub declared_blocks_header: Option<u32>,
    /// Signature-inference details (present whenever inference ran, even if
    /// another witness won — useful as corroboration).
    pub inference: Option<Inference>,
}

/// Result of `kcbh` signature inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inference {
    /// Every candidate with its score, best first.
    pub candidates: Vec<Candidate>,
    /// Whether the best candidate cleared the acceptance rule.
    pub accepted: bool,
}

/// One `(block size, byte order)` hypothesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Block size.
    pub block_size: usize,
    /// Byte order.
    pub order: ByteOrder,
    /// Non-zero blocks examined.
    pub examined: u32,
    /// Blocks that parsed, tail-matched and named their own position.
    pub self_addressed: u32,
}

impl Candidate {
    /// `self_addressed / examined` in per-mille (0 when nothing examined).
    pub fn score_milli(&self) -> u32 {
        self.self_addressed
            .saturating_mul(1000)
            .checked_div(self.examined)
            .unwrap_or(0)
    }
}

/// Minimum non-zero blocks a candidate must have examined.
pub const INFER_MIN_EXAMINED: u32 = 16;
/// Minimum score (‰) for acceptance.
pub const INFER_MIN_SCORE_MILLI: u32 = 500;
/// Minimum lead (‰) over the runner-up.
pub const INFER_MIN_MARGIN_MILLI: u32 = 250;

/// A reader that yields bytes at an offset (the engine supplies a thin
/// adapter over [`crate::source::Evidence`] or a slice).
pub trait ReadAt {
    /// Read up to `buf.len()` bytes at `offset`; return how many were read.
    ///
    /// # Errors
    /// I/O errors from the underlying source.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize>;
    /// Total length in bytes.
    fn len(&self) -> u64;
    /// Whether the source is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ReadAt for &[u8] {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        let Ok(off) = usize::try_from(offset) else {
            return Ok(0);
        };
        let Some(src) = self.get(off..) else {
            return Ok(0);
        };
        let n = src.len().min(buf.len());
        if let (Some(d), Some(s)) = (buf.get_mut(..n), src.get(..n)) {
            d.copy_from_slice(s);
        }
        Ok(n)
    }
    fn len(&self) -> u64 {
        u64::try_from(<[u8]>::len(self)).unwrap_or(u64::MAX)
    }
}

/// Witness 1: `kcvfh` in block 1 at every legal size / order; exactly one
/// must qualify.
fn from_file_header<R: ReadAt + ?Sized>(
    src: &mut R,
) -> std::io::Result<Option<(usize, ByteOrder, u32)>> {
    let mut found = None;
    let mut buf = vec![0u8; 32768];
    for bs in BLOCK_SIZES {
        let off = u64::try_from(bs).unwrap_or(u64::MAX);
        let Some(b) = buf.get_mut(..bs) else { continue };
        let n = src.read_at(off, b)?;
        if n < bs {
            continue;
        }
        for order in [ByteOrder::Little, ByteOrder::Big] {
            if let Ok(h) = FileHeader::parse(b, order)
                && usize::try_from(h.block_size).ok() == Some(bs)
            {
                if found.is_some() {
                    return Ok(None);
                }
                found = Some((bs, order, h.file_size_blocks));
            }
        }
    }
    Ok(found)
}

/// Witness 2: OS block 0. Returns `(block_size, declared_blocks, order)`
/// when the magic is present in some byte order and the size is legal.
fn from_os_block<R: ReadAt + ?Sized>(
    src: &mut R,
) -> std::io::Result<Option<(usize, u32, ByteOrder)>> {
    let mut b = [0u8; 32];
    if src.read_at(0, &mut b)? < 32 {
        return Ok(None);
    }
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let (Ok(magic), Ok(bs), Ok(n)) = (
            read_u32(&b, 28, order),
            read_u32(&b, 20, order),
            read_u32(&b, 24, order),
        ) else {
            continue;
        };
        if magic == BLOCK0_MAGIC
            && let Ok(bsz) = usize::try_from(bs)
            && BLOCK_SIZES.contains(&bsz)
        {
            return Ok(Some((bsz, n, order)));
        }
    }
    Ok(None)
}

/// Does `blk`, read from block number `index` under `(bs, order)`, look
/// like an Oracle block that knows where it lives?
fn self_addressed(blk: &[u8], bs: usize, order: ByteOrder, index: u64) -> bool {
    let Ok(h) = BlockHeader::parse(blk, order) else {
        return false;
    };
    if h.format & 0x0F != 0x02 {
        return false;
    }
    let Ok(tail) = read_u32(blk, bs.saturating_sub(TAIL_LEN), order) else {
        return false;
    };
    if tail != h.expected_tail() {
        return false;
    }
    let Ok(i) = u32::try_from(index) else {
        return false;
    };
    let (_, s) = h.rdba.split(RdbaScheme::Smallfile);
    let (_, b) = h.rdba.split(RdbaScheme::Bigfile);
    s == i || b == i
}

/// Witness 3: sample `windows` windows of `window_len` bytes spread evenly
/// over the file and score every candidate.
///
/// # Errors
/// I/O errors from the source.
pub fn infer<R: ReadAt + ?Sized>(
    src: &mut R,
    windows: u32,
    window_len: usize,
) -> std::io::Result<Inference> {
    let len = src.len();
    // Window length must be a multiple of the largest block size so every
    // candidate sees whole blocks at aligned offsets.
    let max_bs = BLOCK_SIZES.iter().copied().max().unwrap_or(32768);
    let wl = window_len
        .max(max_bs)
        .checked_div(max_bs)
        .unwrap_or(1)
        .saturating_mul(max_bs);
    let wl64 = u64::try_from(wl).unwrap_or(u64::MAX);
    let mut buf = vec![0u8; wl];
    let mut cands: Vec<Candidate> = BLOCK_SIZES
        .iter()
        .flat_map(|&bs| {
            [ByteOrder::Little, ByteOrder::Big].map(|order| Candidate {
                block_size: bs,
                order,
                examined: 0,
                self_addressed: 0,
            })
        })
        .collect();
    let n_windows = u64::from(windows.max(1));
    let stride = len
        .saturating_sub(wl64)
        .checked_div(n_windows.max(1))
        .unwrap_or(0);
    // Window starts aligned to the largest block size.
    let max_bs64 = u64::try_from(max_bs).unwrap_or(u64::MAX);
    let mut starts: Vec<u64> = (0..n_windows)
        .map(|w| {
            w.saturating_mul(stride)
                .checked_div(max_bs64)
                .unwrap_or(0)
                .saturating_mul(max_bs64)
        })
        .collect();
    starts.dedup();
    for start in starts {
        let got = src.read_at(start, &mut buf)?;
        let Some(data) = buf.get(..got) else { continue };
        for c in &mut cands {
            let bs = c.block_size;
            let bs64 = u64::try_from(bs).unwrap_or(u64::MAX);
            let first_index = start.checked_div(bs64).unwrap_or(0);
            for (k, blk) in data.chunks_exact(bs).enumerate() {
                if blk.len() < KCBH_LEN + TAIL_LEN || blk.iter().all(|&x| x == 0) {
                    continue;
                }
                let index = first_index.saturating_add(u64::try_from(k).unwrap_or(u64::MAX));
                if index == 0 {
                    continue; // OS block never self-addresses
                }
                c.examined = c.examined.saturating_add(1);
                if self_addressed(blk, bs, c.order, index) {
                    c.self_addressed = c.self_addressed.saturating_add(1);
                }
            }
        }
    }
    cands.sort_by(|a, b| {
        b.score_milli()
            .cmp(&a.score_milli())
            .then(b.self_addressed.cmp(&a.self_addressed))
            .then(a.block_size.cmp(&b.block_size))
    });
    let accepted = match cands.as_slice() {
        [best, second, ..] => {
            best.examined >= INFER_MIN_EXAMINED
                && best.score_milli() >= INFER_MIN_SCORE_MILLI
                && best.score_milli().saturating_sub(second.score_milli()) >= INFER_MIN_MARGIN_MILLI
        }
        [best] => {
            best.examined >= INFER_MIN_EXAMINED && best.score_milli() >= INFER_MIN_SCORE_MILLI
        }
        [] => false,
    };
    Ok(Inference {
        candidates: cands,
        accepted,
    })
}

/// Decide the layout. `forced` (from the command line) wins over everything
/// when given; otherwise the three witnesses are consulted in order. Returns
/// `Ok(None)` when no witness is conclusive (the caller refuses or asks for
/// `--force`).
///
/// # Errors
/// I/O errors from the source.
pub fn decide<R: ReadAt + ?Sized>(
    src: &mut R,
    forced: Option<(usize, ByteOrder)>,
) -> std::io::Result<Option<Layout>> {
    let os = from_os_block(src)?;
    let hdr = from_file_header(src)?;
    let inference = infer(src, 64, 256 * 1024)?;
    let declared_blocks_os = os.map(|(_, n, _)| n);
    let declared_blocks_header = hdr.map(|(_, _, n)| n);
    let mk = |block_size, order, source| Layout {
        block_size,
        order,
        source,
        declared_blocks_os,
        declared_blocks_header,
        inference: Some(inference.clone()),
    };
    if let Some((bs, order)) = forced {
        return Ok(Some(mk(bs, order, LayoutSource::Forced)));
    }
    if let Some((bs, order, _)) = hdr {
        return Ok(Some(mk(bs, order, LayoutSource::FileHeader)));
    }
    if let Some((bs, _, order)) = os {
        // Block 0 does not fix the byte order of the *blocks* beyond its own
        // magic; corroborate with inference when it is conclusive and agrees
        // on the size, else trust block 0's order.
        let order = inference
            .candidates
            .first()
            .filter(|c| inference.accepted && c.block_size == bs)
            .map_or(order, |c| c.order);
        return Ok(Some(mk(bs, order, LayoutSource::OsBlock)));
    }
    if inference.accepted
        && let Some(best) = inference.candidates.first()
    {
        return Ok(Some(mk(
            best.block_size,
            best.order,
            LayoutSource::Inferred,
        )));
    }
    Ok(None)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn os_block_magic_is_read_in_either_order() {
        let mut b = vec![0u8; 64];
        b[20..24].copy_from_slice(&8192u32.to_le_bytes());
        b[24..28].copy_from_slice(&100u32.to_le_bytes());
        b[28..32].copy_from_slice(&BLOCK0_MAGIC.to_le_bytes());
        let mut s: &[u8] = &b;
        assert_eq!(
            from_os_block(&mut s).unwrap(),
            Some((8192, 100, ByteOrder::Little))
        );
        let mut b = vec![0u8; 64];
        b[20..24].copy_from_slice(&4096u32.to_be_bytes());
        b[24..28].copy_from_slice(&7u32.to_be_bytes());
        b[28..32].copy_from_slice(&BLOCK0_MAGIC.to_be_bytes());
        let mut s: &[u8] = &b;
        assert_eq!(
            from_os_block(&mut s).unwrap(),
            Some((4096, 7, ByteOrder::Big))
        );
        let mut s: &[u8] = &[0u8; 64];
        assert_eq!(from_os_block(&mut s).unwrap(), None);
    }

    #[test]
    fn empty_and_tiny_sources_are_refused() {
        let mut s: &[u8] = &[];
        assert!(decide(&mut s, None).unwrap().is_none());
        let mut s: &[u8] = &[0xAAu8; 5000];
        assert!(decide(&mut s, None).unwrap().is_none());
        let zeros = vec![0u8; 65536];
        let mut s: &[u8] = &zeros;
        assert!(decide(&mut s, None).unwrap().is_none());
        let mut s: &[u8] = &zeros;
        let l = decide(&mut s, Some((2048, ByteOrder::Big)))
            .unwrap()
            .unwrap();
        assert_eq!(
            (l.block_size, l.order, l.source),
            (2048, ByteOrder::Big, LayoutSource::Forced)
        );
    }
}
