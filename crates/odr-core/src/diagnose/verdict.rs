//! Per-block verdict: **what** the block is and **what is wrong** with it.
//!
//! Every judgement is derived from the block's own bytes; nothing is guessed
//! from neighbours. The order of damage checks is fixed so a report is
//! reproducible byte-for-byte across runs and platforms.
//!
//! Evidence: `docs/design/lab-verify.md` LV-075 (`ktbbhtyp = 5` is a LOB
//! block, 3/3 against `dbv`), LV-077 (entropy threshold: max 7.676 bits/byte
//! on any healthy 26ai block vs 7.977 ± 0.01 for random bytes).

use crate::block::{BlockHeader, KCBH_LEN, RdbaScheme, TAIL_LEN, xor_fold};
use crate::bytes::{ByteOrder, read_u32};
use crate::row::compressed::CompressedBlock;
use crate::row::kdb::ASSM_GAP;
use crate::row::kdb::{DataBlock, StructuralFault};
use crate::row::ktb::{Ktbbh, SegmentKind};
use serde::{Deserialize, Serialize};

/// Shannon entropy (bits per byte) at or above which an unparsable block is
/// called *encrypted* rather than *garbage* (LV-077). Healthy Oracle blocks,
/// including BASIC-compressed and index blocks, never exceed 7.68; uniformly
/// random bytes measure 7.97–7.98 on 8 KiB.
pub const ENCRYPTED_ENTROPY_MIN: f64 = 7.85;

/// `seq_kcbh` value Oracle writes when a block is *marked* corrupt
/// (`DBMS_REPAIR.FIX_CORRUPT_BLOCKS`, NOLOGGING recovery). `lab-verify`:
/// documented in Oracle block dumps as `seq: 0xff`; not yet reproduced in the
/// lab.
pub const SEQ_MARKED_CORRUPT: u8 = 0xFF;

/// What a block *is*, from its header (`type_kcbh`, and `ktbbhtyp` for
/// transactional blocks). Mirrors `dbv`'s page classes so the two can be
/// compared exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BlockKind {
    /// Block 0: the OS header (never an Oracle block).
    OsHeader,
    /// `0x0B` — datafile header (`kcvfh`).
    FileHeader,
    /// `0x00` with a well-formed `kcbh` — formatted but never used
    /// (`dbv` "Empty").
    Empty,
    /// `0x06` with `ktbbhtyp = 1` — table/cluster data (`dbv` "Data").
    Data,
    /// `0x06` with `ktbbhtyp = 2` — index (`dbv` "Index").
    Index,
    /// `0x06` with `ktbbhtyp = 5` — LOB (`SecureFile`) data (`dbv` "Lob").
    Lob,
    /// `0x06` with any other `ktbbhtyp`.
    DataOther(u8),
    /// `0x02` — undo block.
    Undo,
    /// `0x0E` — undo segment header.
    UndoHeader,
    /// `0x10` / `0x23` — segment header (`dbv` "Seg").
    SegmentHeader,
    /// `0x1D`, `0x1E`, `0x20`–`0x22` — space management.
    Space,
    /// `0x28` — `BasicFile` LOB chunk.
    LobChunk,
    /// Any other `type_kcbh`.
    Other(u8),
    /// No header could be read (damaged beyond the header).
    Unknown,
}

impl BlockKind {
    /// Classify from the type byte and, for `0x06`, the segment kind.
    pub fn from_header(type_byte: u8, seg: Option<SegmentKind>, raw_typ: u8) -> Self {
        match type_byte {
            0x00 => Self::Empty,
            0x02 => Self::Undo,
            0x06 => match seg {
                Some(SegmentKind::Data) => Self::Data,
                Some(SegmentKind::Index) => Self::Index,
                Some(SegmentKind::Other(5)) => Self::Lob,
                Some(SegmentKind::Other(o)) => Self::DataOther(o),
                None => Self::DataOther(raw_typ),
            },
            0x0B => Self::FileHeader,
            0x0E => Self::UndoHeader,
            0x10 | 0x23 => Self::SegmentHeader,
            0x1D | 0x1E | 0x20..=0x22 => Self::Space,
            0x28 => Self::LobChunk,
            o => Self::Other(o),
        }
    }

    /// `dbv` page class this kind is counted under.
    ///
    /// LV-078 (measured on 4 files / 61,696 blocks of 26ai against `dbv`
    /// 23.0): `Seg` is the **undo segment header** (`0x0E`) only; table /
    /// ASSM segment headers (`0x10`, `0x23`) are counted under `Other`.
    pub const fn dbv_class(self) -> DbvClass {
        match self {
            Self::Empty => DbvClass::Empty,
            Self::Data => DbvClass::Data,
            Self::Index => DbvClass::Index,
            Self::Lob => DbvClass::Lob,
            Self::UndoHeader => DbvClass::Seg,
            Self::OsHeader => DbvClass::NotCounted,
            _ => DbvClass::Other,
        }
    }
}

/// `dbv` "Total Pages Processed (…)" classes (LV-078: `Other` is every
/// formatted non-`0x06` page except `0x0E`; `Seg` is `0x0E`; the OS block
/// is not examined).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DbvClass {
    /// Table / cluster data.
    Data,
    /// Index.
    Index,
    /// LOB.
    Lob,
    /// Segment header.
    Seg,
    /// Everything else that is formatted.
    Other,
    /// Formatted, never used.
    Empty,
    /// Block 0.
    NotCounted,
}

/// Randomness features of a byte region (panel §1.3: entropy alone is a
/// weak discriminator; χ² and serial correlation separate ciphertext from
/// compressed data far better). All integer, deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Randomness {
    /// Shannon entropy × 1000 (bits/byte).
    pub entropy_milli: u16,
    /// χ² statistic against the uniform distribution over 256 bins, × 1
    /// (for 8 KiB: uniform ⇒ ≈ 255 ± 23; compressed data typically ≫ 400).
    pub chi_square: u32,
    /// |serial byte correlation| × 1000 (ciphertext ≤ 30; structured data ≫).
    pub serial_corr_milli: u16,
}

impl Randomness {
    /// Does this look like ciphertext / true randomness? (panel C §3.b
    /// thresholds adapted to 8 KiB: entropy ≥ 7.85, χ² ≤ 330, |r| ≤ 0.05.)
    pub const fn looks_random(self) -> bool {
        self.entropy_milli >= 7850 && self.chi_square <= 330 && self.serial_corr_milli <= 50
    }
}

/// Compute [`Randomness`] for a region.
pub fn randomness(bytes: &[u8]) -> Randomness {
    let n = bytes.len();
    if n < 2 {
        return Randomness {
            entropy_milli: 0,
            chi_square: 0,
            serial_corr_milli: 0,
        };
    }
    let mut hist = [0u32; 256];
    for &b in bytes {
        if let Some(c) = hist.get_mut(usize::from(b)) {
            *c = c.saturating_add(1);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let nf = n as f64;
    let expected = nf / 256.0;
    let mut chi = 0.0f64;
    for &c in &hist {
        let d = f64::from(c) - expected;
        chi += d * d / expected;
    }
    // Serial correlation (lag 1), Pearson on consecutive byte pairs.
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for w in bytes.windows(2) {
        if let [a, b] = *w {
            let (x, y) = (f64::from(a), f64::from(b));
            sx += x;
            sy += y;
            sxx += x * x;
            syy += y * y;
            sxy += x * y;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let m = n.saturating_sub(1) as f64;
    let cov = sxy - sx * sy / m;
    let vx = sxx - sx * sx / m;
    let vy = syy - sy * sy / m;
    let r = if vx > 0.0 && vy > 0.0 {
        (cov / (vx * vy).sqrt()).abs()
    } else {
        0.0
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let out = Randomness {
        entropy_milli: entropy_milli(bytes),
        chi_square: chi.min(f64::from(u32::MAX)) as u32,
        serial_corr_milli: (r * 1000.0).min(1000.0) as u16,
    };
    out
}

/// What is *wrong* with a block. Checked in this order; the first hit wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Damage {
    /// Nothing wrong at the chosen depth.
    None,
    /// Shorter than the block size — the file ends inside this block.
    Truncated {
        /// Bytes present.
        have: usize,
    },
    /// Every byte is zero.
    Zeroed,
    /// The 20-byte `kcbh` is all zero but the rest of the block is not: a
    /// sub-block hole (sparse storage, lost 512 B / 4 KiB sector, partial
    /// wipe) that happened to cover the header.
    HeaderZeroed,
    /// Header does not parse and the bytes look random (ransomware / any
    /// cipher): entropy ≥ [`ENCRYPTED_ENTROPY_MIN`] and χ² / serial
    /// correlation consistent with ciphertext.
    Encrypted {
        /// Shannon entropy × 1000 (bits/byte), integer for exact comparison.
        entropy_milli: u16,
        /// χ² against uniform.
        chi_square: u32,
        /// |serial correlation| × 1000.
        serial_corr_milli: u16,
    },
    /// Header, tail and (if any) checksum are **valid** but the payload is
    /// ciphertext-like and carries no decodable structure for its kind:
    /// header-preserving ransomware (panel §1.1) — or legitimate TDE, which
    /// the file header would declare. Never raised when a row directory
    /// decodes.
    PayloadScrambled {
        /// Payload entropy × 1000.
        entropy_milli: u16,
        /// χ² of the payload.
        chi_square: u32,
    },
    /// Physically valid but its SCN is far below the object's other blocks:
    /// an older image (lost write, mixed restore). Assigned in a post-pass.
    StaleScn {
        /// Block SCN.
        scn: u64,
        /// Object median SCN.
        median: u64,
    },
    /// The same `rdba` was already seen at an earlier position in this
    /// file (duplicated / twice-restored block). Assigned in a post-pass.
    DuplicateRdba {
        /// Earlier position.
        first_at: u64,
    },
    /// Header does not parse and the bytes are *not* random — something
    /// structured but foreign was written here (text, another file…).
    Garbage {
        /// Shannon entropy × 1000.
        entropy_milli: u16,
    },
    /// `seq_kcbh == 0xFF`: Oracle itself marked this block corrupt.
    MarkedCorrupt,
    /// Checksum flag set and the XOR fold is non-zero (bit rot / media).
    ChecksumFail,
    /// Tail check does not match the header (torn / fractured write).
    Torn,
    /// Header, tail and checksum agree but the block says it lives
    /// elsewhere (misdirected write, swapped blocks).
    Misplaced {
        /// Block number claimed by `rdba` (under the scheme that fit, or the
        /// smallfile block field if neither fit).
        header_block: u32,
    },
    /// Passes every physical check; an internal invariant is broken
    /// (`kdbchk`-class logical corruption, LV-062).
    Logical {
        /// The first violated rule.
        fault: StructuralFault,
    },
}

impl Damage {
    /// Whether the block is unusable as-is.
    pub const fn is_damaged(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Short stable code for reports and tests.
    pub const fn code(self) -> &'static str {
        match self {
            Self::None => "ok",
            Self::Truncated { .. } => "truncated",
            Self::Zeroed => "zeroed",
            Self::HeaderZeroed => "header_zeroed",
            Self::Encrypted { .. } => "encrypted",
            Self::PayloadScrambled { .. } => "payload_scrambled",
            Self::StaleScn { .. } => "stale_scn",
            Self::DuplicateRdba { .. } => "duplicate_rdba",
            Self::Garbage { .. } => "garbage",
            Self::MarkedCorrupt => "marked_corrupt",
            Self::ChecksumFail => "checksum_fail",
            Self::Torn => "torn",
            Self::Misplaced { .. } => "misplaced",
            Self::Logical { .. } => "logical",
        }
    }
}

/// How deep to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Depth {
    /// Header, tail, checksum, placement.
    Physical,
    /// Physical plus `structural_check` and live-row counting on data blocks.
    Logical,
}

/// The verdict on one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// What the block is.
    pub kind: BlockKind,
    /// What is wrong.
    pub damage: Damage,
    /// Raw `type_kcbh`, when a header was readable.
    pub type_byte: Option<u8>,
    /// `DATA_OBJECT_ID` from `ktbbh`, for `0x06` blocks whose header is
    /// trustworthy (physically clean). Never reported from a damaged block.
    pub objd: Option<u32>,
    /// Block SCN, when a header was readable.
    pub scn: Option<u64>,
    /// Live (non-deleted, head-piece) rows counted at [`Depth::Logical`] on
    /// clean `Data` blocks.
    pub live_rows: Option<u32>,
    /// Whether the header carried the checksum flag (`flg_kcbh & 0x04`).
    /// `DB_BLOCK_CHECKSUM=OFF` leaves it clear: an *absence of evidence*
    /// the report must disclose (panel §1.2).
    pub checksum_present: bool,
    /// Raw `rdba` for duplicate detection (clean blocks only).
    pub rdba: Option<u32>,
}

/// Shannon entropy in bits per byte, × 1000, rounded down. Exact integer
/// arithmetic except for the final `log2`, which is deterministic for the
/// same histogram on every IEEE-754 platform.
pub fn entropy_milli(bytes: &[u8]) -> u16 {
    if bytes.is_empty() {
        return 0;
    }
    let mut hist = [0u32; 256];
    for &b in bytes {
        if let Some(c) = hist.get_mut(usize::from(b)) {
            *c = c.saturating_add(1);
        }
    }
    #[allow(clippy::cast_precision_loss)] // counts ≤ 32 KiB fit f64 exactly
    let n = bytes.len() as f64;
    let mut h = 0.0f64;
    for &c in &hist {
        if c > 0 {
            #[allow(clippy::cast_precision_loss)]
            let p = f64::from(c) / n;
            h -= p * p.log2();
        }
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // h ∈ [0, 8] ⇒ h·1000 ∈ [0, 8000] fits u16
    let m = (h * 1000.0).floor() as u16;
    m
}

/// Judge one block read from position `index` (0-based block number in the
/// file). `block` may be shorter than `block_size` only for the last block.
///
/// Never panics, for any input.
#[allow(clippy::too_many_lines)] // one fixed-order decision ladder; splitting it would hide the order
pub fn verdict(
    block: &[u8],
    block_size: usize,
    order: ByteOrder,
    index: u64,
    depth: Depth,
) -> Verdict {
    let unknown = |damage: Damage| Verdict {
        kind: BlockKind::Unknown,
        damage,
        type_byte: None,
        objd: None,
        scn: None,
        live_rows: None,
        checksum_present: false,
        rdba: None,
    };
    if block.len() < block_size || block.len() < KCBH_LEN + TAIL_LEN {
        return unknown(Damage::Truncated { have: block.len() });
    }
    if index == 0 {
        return Verdict {
            kind: BlockKind::OsHeader,
            damage: Damage::None,
            type_byte: None,
            objd: None,
            scn: None,
            live_rows: None,
            checksum_present: false,
            rdba: None,
        };
    }
    if block.iter().all(|&b| b == 0) {
        return unknown(Damage::Zeroed);
    }
    let Some(body) = block.get(..block_size) else {
        return unknown(Damage::Truncated { have: block.len() });
    };
    if body
        .get(..KCBH_LEN)
        .is_some_and(|h| h.iter().all(|&b| b == 0))
    {
        return unknown(Damage::HeaderZeroed);
    }
    let Ok(h) = BlockHeader::parse(body, order) else {
        return unknown(Damage::Truncated { have: block.len() });
    };
    // A header is "readable" when the format nibble is Oracle's; otherwise
    // the bytes are not an Oracle block at all and entropy decides what they
    // are.
    if h.format & 0x0F != 0x02 {
        let rnd = randomness(body);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let min = (ENCRYPTED_ENTROPY_MIN * 1000.0) as u16;
        return unknown(if rnd.entropy_milli >= min {
            Damage::Encrypted {
                entropy_milli: rnd.entropy_milli,
                chi_square: rnd.chi_square,
                serial_corr_milli: rnd.serial_corr_milli,
            }
        } else {
            Damage::Garbage {
                entropy_milli: rnd.entropy_milli,
            }
        });
    }
    let tail_ok = matches!(read_u32(body, block_size.saturating_sub(TAIL_LEN), order), Ok(t) if t == h.expected_tail());
    let ktb = if h.type_byte == 0x06 {
        Ktbbh::parse(body, order).ok()
    } else {
        None
    };
    let raw_typ = body.get(0x14).copied().unwrap_or(0);
    let kind = BlockKind::from_header(h.type_byte, ktb.map(|k| k.kind), raw_typ);
    let mut v = Verdict {
        kind,
        damage: Damage::None,
        type_byte: Some(h.type_byte),
        objd: None,
        scn: Some(h.scn.0),
        live_rows: None,
        checksum_present: h.has_checksum(),
        rdba: None,
    };
    if h.seq == SEQ_MARKED_CORRUPT {
        v.damage = Damage::MarkedCorrupt;
        return v;
    }
    // Tail before checksum, as Oracle's `kcbzib` reports it ("Fractured
    // block found" precedes "Bad check value found") and as
    // [`crate::block::classify`] orders it: a torn write almost always
    // fails the checksum too, and *torn* is the more specific diagnosis.
    if !tail_ok {
        v.damage = Damage::Torn;
        return v;
    }
    if h.has_checksum() && xor_fold(body) != 0 {
        v.damage = Damage::ChecksumFail;
        return v;
    }
    // Physically clean from here: the header is trustworthy.
    v.objd = ktb.map(|k| k.objd);
    // Placement: the block must name its own position under *some* scheme.
    // Block 0 is never an Oracle block, so `index ≥ 1` here; `rdba` of an
    // empty formatted block is its position too (LV-076 samples).
    if let Ok(idx32) = u32::try_from(index) {
        let (_, small_blk) = h.rdba.split(RdbaScheme::Smallfile);
        let (_, big_blk) = h.rdba.split(RdbaScheme::Bigfile);
        if small_blk != idx32 && big_blk != idx32 {
            v.damage = Damage::Misplaced {
                header_block: small_blk,
            };
            return v;
        }
    }
    v.rdba = Some(h.rdba.0);
    // Header-preserving encryption (panel §1.1): the transactional header
    // region is plausible, but the payload after the ITL area is ciphertext
    // and *no structure decodes*. Only table data blocks are judged: they
    // are the one kind we can decode (`kdbh` + row directory), so
    // "structure score 0" is a measurement, not a guess. A compressed
    // block (`Unsupported`) *is* recognised structure — its payload is
    // legitimately high-entropy. Index and LOB payloads are deferred until
    // a `kdx`/LOB decoder exists (panel §2); Empty/Space/Undo may hold
    // anything.
    if kind == BlockKind::Data {
        let payload_start = ktb
            .map_or(0x2C, |k| k.itl_end())
            .min(block_size.saturating_sub(TAIL_LEN));
        if let Some(payload) = body.get(payload_start..block_size.saturating_sub(TAIL_LEN)) {
            let rnd = randomness(payload);
            if rnd.looks_random() {
                // `Unsupported` alone is not structure: a random flag byte
                // carries the COMPRESSED bit half the time. Only a parsed
                // compressed block (symbol table + rows) counts.
                let kdbh_off = ktb.map_or(0x2C, |k| k.itl_end());
                let structured = DataBlock::parse(body, order).is_ok()
                    || CompressedBlock::parse(body, order, kdbh_off).is_ok()
                    || CompressedBlock::parse(body, order, kdbh_off.saturating_add(ASSM_GAP))
                        .is_ok();
                if !structured {
                    v.damage = Damage::PayloadScrambled {
                        entropy_milli: rnd.entropy_milli,
                        chi_square: rnd.chi_square,
                    };
                    return v;
                }
            }
        }
    }
    if depth == Depth::Logical && kind == BlockKind::Data {
        match DataBlock::parse(body, order) {
            Ok(db) => {
                if let Some(fault) = db.structural_check() {
                    v.damage = Damage::Logical { fault };
                    return v;
                }
                let mut live: u32 = 0;
                for (_, r) in db.rows() {
                    if let Ok(p) = r
                        && !p.is_deleted()
                        && p.flag & crate::row::flag::H != 0
                    {
                        live = live.saturating_add(1);
                    }
                }
                v.live_rows = Some(live);
            }
            // Compressed (OLTP/HCC) or otherwise unsupported layouts are not
            // corruption; a `Malformed` data block that passed its checksum
            // is a logical fault we cannot name more precisely.
            Err(crate::error::DecodeError::Unsupported(_)) => {}
            Err(_) => {
                v.damage = Damage::Logical {
                    fault: StructuralFault::UndecodableLiveRow { slot: 0, flag: 0 },
                };
                return v;
            }
        }
    }
    v
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;

    #[test]
    fn entropy_extremes() {
        assert_eq!(entropy_milli(&[]), 0);
        assert_eq!(entropy_milli(&[7u8; 8192]), 0);
        let uniform: Vec<u8> = (0..8192u32).map(|i| (i % 256) as u8).collect();
        assert_eq!(entropy_milli(&uniform), 8000);
        let two: Vec<u8> = (0..8192u32).map(|i| (i % 2) as u8).collect();
        assert_eq!(entropy_milli(&two), 1000);
    }

    #[test]
    fn randomness_separates_uniform_from_structured() {
        // SplitMix64: the multiplicative-hash sequence used before is a
        // Weyl sequence with |serial r| ≈ 0.17 — the detector was right
        // to refuse it, so the fixture must be a real PRNG.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let uniform: Vec<u8> = (0..8192u32)
            .map(|_| {
                state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = state;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                (z ^ (z >> 31)) as u8
            })
            .collect();
        let r = randomness(&uniform);
        assert!(r.entropy_milli >= 7900, "{r:?}");
        assert!(r.chi_square <= 330, "{r:?}");
        assert!(r.serial_corr_milli <= 50, "{r:?}");
        assert!(r.looks_random());
        let text: Vec<u8> = b"SELECT * FROM T WHERE ID = 1; "
            .iter()
            .cycle()
            .take(8192)
            .copied()
            .collect();
        let t = randomness(&text);
        assert!(!t.looks_random(), "{t:?}");
        assert!(t.chi_square > 1000, "{t:?}");
        assert_eq!(randomness(&[]).entropy_milli, 0);
    }

    #[test]
    fn verdict_never_panics_on_degenerate_input() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            for depth in [Depth::Physical, Depth::Logical] {
                assert!(matches!(
                    verdict(&[], 8192, order, 5, depth).damage,
                    Damage::Truncated { have: 0 }
                ));
                assert_eq!(
                    verdict(&[0u8; 8192], 8192, order, 5, depth).damage,
                    Damage::Zeroed
                );
                assert_eq!(
                    verdict(&[0u8; 8192], 8192, order, 0, depth).kind,
                    BlockKind::OsHeader
                );
                let v = verdict(&[0xFFu8; 8192], 8192, order, 5, depth);
                assert!(matches!(v.damage, Damage::Garbage { entropy_milli: 0 }));
                assert_eq!(v.kind, BlockKind::Unknown);
                let mut hz = vec![0u8; 8192];
                hz[100] = 1;
                assert_eq!(
                    verdict(&hz, 8192, order, 5, depth).damage,
                    Damage::HeaderZeroed
                );
            }
        }
    }

    #[test]
    fn block_kind_mirrors_dbv_classes() {
        assert_eq!(
            BlockKind::from_header(0x06, Some(SegmentKind::Data), 1),
            BlockKind::Data
        );
        assert_eq!(
            BlockKind::from_header(0x06, Some(SegmentKind::Index), 2),
            BlockKind::Index
        );
        assert_eq!(
            BlockKind::from_header(0x06, Some(SegmentKind::Other(5)), 5),
            BlockKind::Lob
        );
        assert_eq!(BlockKind::from_header(0x00, None, 0), BlockKind::Empty);
        assert_eq!(
            BlockKind::from_header(0x0E, None, 0).dbv_class(),
            DbvClass::Seg
        );
        assert_eq!(
            BlockKind::from_header(0x10, None, 0).dbv_class(),
            DbvClass::Other
        );
        assert_eq!(
            BlockKind::from_header(0x23, None, 0).dbv_class(),
            DbvClass::Other
        );
        assert_eq!(
            BlockKind::from_header(0x1E, None, 0).dbv_class(),
            DbvClass::Other
        );
        assert_eq!(
            BlockKind::from_header(0x28, None, 0).dbv_class(),
            DbvClass::Other
        );
        assert_eq!(
            BlockKind::from_header(0x0B, None, 0).dbv_class(),
            DbvClass::Other
        );
    }
}
