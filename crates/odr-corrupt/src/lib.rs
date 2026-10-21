//! `odr-corrupt` — deterministic, manifest-backed damage for **copies** of
//! Oracle datafiles (ADR-0004 golden corpus; REQ-004 tests).
//!
//! Guarantees (all tested):
//! * The source is opened read-only and never written. Output must be a
//!   different path; refusing to overwrite an existing file prevents mistakes.
//! * Same (input bytes, spec, seed) ⇒ same output bytes and same manifest, on
//!   every platform (`SplitMix64`, no OS randomness, no floats).
//! * The manifest lists **every** byte range that differs from the input,
//!   exactly: `diff(input, output) == ⋃ manifest ranges` (property test).
//! * SHA-256 of input and output are recorded.
//!
//! Patterns model real failures: random bit rot, torn writes (stale second
//! half), zeroed blocks, ransomware (leading region + every k-th chunk), wiped
//! file header, truncation, block swaps (misplaced writes), scattered missing
//! regions, and **forge** — change a byte then recompute the checksum so the
//! block still passes `kcbh` validation (tests that a valid checksum is not
//! treated as proof of correct content).

// Offsets are bounded by the input length (checked before use) and the PRNG
// uses wrapping ops; overflow-checked arithmetic would only add noise here.
#![allow(clippy::arithmetic_side_effects)]
use odr_core::block::{FLG_CHECKSUM, xor_fold};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One damage operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    /// Flip one random bit in each of `count` random blocks (body only).
    Bitrot {
        /// Number of blocks to hit.
        count: u64,
    },
    /// Torn write in `count` random blocks: bytes from `B/2` replaced by
    /// random data (half old, half new) — tail check no longer matches.
    Torn {
        /// Number of blocks.
        count: u64,
    },
    /// Zero `count` random whole blocks.
    Zero {
        /// Number of blocks.
        count: u64,
    },
    /// Ransomware: overwrite the first `lead_bytes` with random bytes, then
    /// every `every`-th chunk of `chunk` bytes after it (0 = none).
    Ransom {
        /// Bytes encrypted from offset 0.
        lead_bytes: u64,
        /// Chunk size for the intermittent pattern.
        chunk: u64,
        /// Encrypt one chunk out of every `every`.
        every: u64,
    },
    /// Zero blocks `0..=1` (OS header + file header `kcvfh`).
    WipeHeader,
    /// Truncate the file to `keep_bytes`.
    Truncate {
        /// Bytes kept.
        keep_bytes: u64,
    },
    /// Swap `count` random pairs of blocks (misdirected writes).
    Swap {
        /// Number of pairs.
        count: u64,
    },
    /// Zero `count` random regions of `len` bytes (not block aligned).
    Holes {
        /// Number of regions.
        count: u64,
        /// Region length in bytes.
        len: u64,
    },
    /// In `count` random blocks with a checksum, change one body byte and
    /// recompute `chkval` so the block still validates.
    Forge {
        /// Number of blocks.
        count: u64,
    },
}

/// What a spec does to a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    /// Block size of the datafile.
    pub block_size: u64,
    /// Seed for every random choice.
    pub seed: u64,
    /// Operations, applied in order.
    pub ops: Vec<Op>,
    /// Restrict block-granular ops (`bitrot`, `torn`, `zero`, `swap`,
    /// `forge`) to blocks `[first, last]`; `None` = whole file. Lets a test
    /// aim damage at one segment so the sample is meaningful. Byte-granular
    /// ops (`ransom`, `holes`, `truncate`, `wipe-header`) ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<(u64, u64)>,
}

/// One damaged byte range in the output, `[offset, offset+len)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Index of the op in [`Spec::ops`].
    pub op: usize,
    /// What was done.
    pub what: String,
    /// Byte offset.
    pub offset: u64,
    /// Length in bytes.
    pub len: u64,
    /// Block index (`offset / block_size`).
    pub block: u64,
}

/// Machine-readable record of the damage (`*.manifest.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Manifest format version.
    pub version: u32,
    /// The spec that produced it.
    pub spec: Spec,
    /// Input length.
    pub input_len: u64,
    /// SHA-256 of the input (hex).
    pub input_sha256: String,
    /// Output length.
    pub output_len: u64,
    /// SHA-256 of the output (hex).
    pub output_sha256: String,
    /// Every changed range, in application order.
    pub changes: Vec<Change>,
}

/// Errors that make a spec impossible to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    /// Block size is not 2K/4K/8K/16K/32K.
    BlockSize(u64),
    /// The image has no complete block.
    TooSmall,
    /// An op parameter is out of range.
    Param(&'static str),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlockSize(b) => {
                write!(f, "block size {b} is not one of 2048/4096/8192/16384/32768")
            }
            Self::TooSmall => f.write_str("image is smaller than one block"),
            Self::Param(p) => write!(f, "invalid parameter: {p}"),
        }
    }
}
impl std::error::Error for SpecError {}

/// Deterministic PRNG (same as `odr-testkit`, duplicated to keep this tool
/// dependency-free at runtime).
#[derive(Debug, Clone)]
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (`n > 0`); rejection sampling avoids modulo bias.
    fn below(&mut self, n: u64) -> u64 {
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next();
            if v < zone {
                return v % n;
            }
        }
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next().to_le_bytes();
            chunk.copy_from_slice(v.get(..chunk.len()).unwrap_or_default());
        }
    }
}

fn hex(d: &[u8]) -> String {
    use std::fmt::Write;
    d.iter()
        .fold(String::with_capacity(d.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// SHA-256 as lower-case hex.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn usize_of(v: u64) -> Result<usize, SpecError> {
    usize::try_from(v).map_err(|_| SpecError::Param("value does not fit in memory"))
}

/// Apply `spec` to a copy of `input`. Returns the damaged bytes and manifest.
///
/// # Errors
/// [`SpecError`] if the spec cannot be applied to this image.
pub fn apply(input: &[u8], spec: &Spec) -> Result<(Vec<u8>, Manifest), SpecError> {
    if !matches!(spec.block_size, 2048 | 4096 | 8192 | 16384 | 32768) {
        return Err(SpecError::BlockSize(spec.block_size));
    }
    let bs = usize_of(spec.block_size)?;
    let mut out = input.to_vec();
    let mut changes = Vec::new();
    let mut rng = Rng(spec.seed);
    let range = match spec.blocks {
        None => None,
        Some((first, last)) => {
            if last < first {
                return Err(SpecError::Param("blocks: last < first"));
            }
            Some((usize_of(first)?, usize_of(last)?))
        }
    };
    for (i, op) in spec.ops.iter().enumerate() {
        apply_op(&mut out, bs, i, op, &mut rng, &mut changes, range)?;
    }
    let manifest = Manifest {
        version: 1,
        spec: spec.clone(),
        input_len: u64::try_from(input.len()).unwrap_or(u64::MAX),
        input_sha256: sha256_hex(input),
        output_len: u64::try_from(out.len()).unwrap_or(u64::MAX),
        output_sha256: sha256_hex(&out),
        changes,
    };
    Ok((out, manifest))
}

/// Byte range of block `b` (`b < nblocks` guaranteed by callers).
fn block_range(b: usize, bs: usize) -> std::ops::Range<usize> {
    b.saturating_mul(bs)..b.saturating_add(1).saturating_mul(bs)
}

fn record(changes: &mut Vec<Change>, op: usize, what: &str, offset: usize, len: usize, bs: usize) {
    let to64 = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
    changes.push(Change {
        op,
        what: what.to_owned(),
        offset: to64(offset),
        len: to64(len),
        block: to64(offset.checked_div(bs).unwrap_or(0)),
    });
}

fn nblocks(out: &[u8], bs: usize) -> Result<usize, SpecError> {
    let n = out.len().checked_div(bs).unwrap_or(0);
    if n == 0 {
        Err(SpecError::TooSmall)
    } else {
        Ok(n)
    }
}

fn pick(rng: &mut Rng, n: usize) -> usize {
    usize::try_from(rng.below(u64::try_from(n).unwrap_or(u64::MAX))).unwrap_or(0)
}

/// Pick a block index inside the optional range (clamped to the image).
fn pick_block(rng: &mut Rng, n: usize, range: Option<(usize, usize)>) -> Result<usize, SpecError> {
    match range {
        None => Ok(pick(rng, n)),
        Some((first, last)) => {
            let last = last.min(n.saturating_sub(1));
            if first > last {
                return Err(SpecError::Param("blocks range outside the image"));
            }
            Ok(first + pick(rng, last - first + 1))
        }
    }
}

#[allow(clippy::too_many_lines)]
fn apply_op(
    out: &mut Vec<u8>,
    bs: usize,
    i: usize,
    op: &Op,
    rng: &mut Rng,
    changes: &mut Vec<Change>,
    range: Option<(usize, usize)>,
) -> Result<(), SpecError> {
    // Block body = bytes 20 .. B-4 (kcbh and tail check excluded).
    let body = 20..bs.saturating_sub(4);
    match *op {
        Op::Bitrot { count } => {
            let n = nblocks(out, bs)?;
            for _ in 0..count {
                let b = pick_block(rng, n, range)?;
                let pos = block_range(b, bs).start + body.start + pick(rng, body.len());
                let bit = 1u8 << rng.below(8);
                if let Some(x) = out.get_mut(pos) {
                    *x ^= bit;
                }
                record(changes, i, "bitrot", pos, 1, bs);
            }
        }
        Op::Torn { count } => {
            let n = nblocks(out, bs)?;
            for _ in 0..count {
                let r = block_range(pick_block(rng, n, range)?, bs);
                let half = r.start + bs / 2;
                if let Some(s) = out.get_mut(half..r.end) {
                    rng.fill(s);
                }
                record(changes, i, "torn", half, bs / 2, bs);
            }
        }
        Op::Zero { count } => {
            let n = nblocks(out, bs)?;
            for _ in 0..count {
                let r = block_range(pick_block(rng, n, range)?, bs);
                if let Some(s) = out.get_mut(r.clone()) {
                    s.fill(0);
                }
                record(changes, i, "zero", r.start, bs, bs);
            }
        }
        Op::Ransom {
            lead_bytes,
            chunk,
            every,
        } => {
            let lead = usize_of(lead_bytes)?.min(out.len());
            if let Some(s) = out.get_mut(..lead) {
                rng.fill(s);
            }
            if lead > 0 {
                record(changes, i, "ransom_lead", 0, lead, bs);
            }
            if chunk > 0 && every > 0 {
                let c = usize_of(chunk)?;
                let k = usize_of(every)?;
                let mut off = lead;
                let mut idx = 0usize;
                while off < out.len() {
                    let end = off.saturating_add(c).min(out.len());
                    if idx.is_multiple_of(k) {
                        if let Some(s) = out.get_mut(off..end) {
                            rng.fill(s);
                        }
                        record(changes, i, "ransom_chunk", off, end - off, bs);
                    }
                    off = end;
                    idx = idx.wrapping_add(1);
                }
            }
        }
        Op::WipeHeader => {
            let end = bs.saturating_mul(2).min(out.len());
            if let Some(s) = out.get_mut(..end) {
                s.fill(0);
            }
            record(changes, i, "wipe_header", 0, end, bs);
        }
        Op::Truncate { keep_bytes } => {
            let keep = usize_of(keep_bytes)?;
            if keep < out.len() {
                let cut = out.len() - keep;
                record(changes, i, "truncate", keep, cut, bs);
                out.truncate(keep);
            }
        }
        Op::Swap { count } => {
            let n = nblocks(out, bs)?;
            if n < 2 {
                return Err(SpecError::Param("swap needs at least two blocks"));
            }
            for _ in 0..count {
                let a = pick_block(rng, n, range)?;
                let mut b = pick_block(
                    rng,
                    n.saturating_sub(1),
                    range.map(|(f, l)| (f, l.saturating_sub(1))),
                )?;
                if b >= a {
                    b += 1;
                }
                let (lo, hi) = (a.min(b), a.max(b));
                let (head, tail) = out.split_at_mut(hi * bs);
                if let (Some(x), Some(y)) =
                    (head.get_mut(lo * bs..(lo + 1) * bs), tail.get_mut(..bs))
                {
                    x.swap_with_slice(y);
                }
                record(changes, i, "swap", lo * bs, bs, bs);
                record(changes, i, "swap", hi * bs, bs, bs);
            }
        }
        Op::Holes { count, len } => {
            let l = usize_of(len)?;
            if l == 0 || l > out.len() {
                return Err(SpecError::Param("holes len must be 1..=file length"));
            }
            for _ in 0..count {
                let off = pick(rng, out.len() - l + 1);
                if let Some(s) = out.get_mut(off..off + l) {
                    s.fill(0);
                }
                record(changes, i, "hole", off, l, bs);
            }
        }
        Op::Forge { count } => {
            let n = nblocks(out, bs)?;
            for _ in 0..count {
                let r = block_range(pick_block(rng, n, range)?, bs);
                let Some(blk) = out.get_mut(r.clone()) else {
                    continue;
                };
                if blk.get(15).is_none_or(|f| f & FLG_CHECKSUM == 0) {
                    continue; // no checksum: nothing to forge, nothing changed
                }
                let pos = body.start + pick(rng, body.len());
                let old = blk.get(16..18).map(<[u8]>::to_vec).unwrap_or_default();
                if let Some(x) = blk.get_mut(pos) {
                    *x = x.wrapping_add(1);
                }
                if let Some(c) = blk.get_mut(16..18) {
                    c.fill(0);
                }
                // chkval is the fold of everything else (same rule as odr-testkit).
                let c = xor_fold(blk).to_le_bytes();
                if let Some(dst) = blk.get_mut(16..18) {
                    dst.copy_from_slice(&c);
                }
                record(changes, i, "forge_value", r.start + pos, 1, bs);
                if blk.get(16..18) != Some(old.as_slice()) {
                    record(changes, i, "forge_checksum", r.start + 16, 2, bs);
                }
            }
        }
    }
    Ok(())
}
