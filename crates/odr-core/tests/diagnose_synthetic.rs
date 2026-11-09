//! `odr_core::diagnose` on synthetic images: every verdict, the layout
//! witnesses, region merging and the finding codes — no Oracle needed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::arithmetic_side_effects,
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::format_collect
)]

use odr_core::ByteOrder;
use odr_core::block::{KCBH_LEN, TAIL_LEN};
use odr_core::diagnose::layout::{BLOCK0_MAGIC, decide, infer};
use odr_core::diagnose::{
    BlockKind, Damage, Depth, FindingCode, LayoutSource, Options, Severity, diagnose, run, verdict,
};
use odr_testkit::{Corruption, SplitMix64, corrupt, synth_block, synth_image};

const BS: usize = 8192;

/// A synthetic datafile: OS block 0 with the Oracle magic, then `n` valid
/// data blocks whose `rdba` names their own position (smallfile, rfile 1).
fn datafile(n: usize, order: ByteOrder, seed: u64) -> Vec<u8> {
    let mut img = vec![0u8; BS];
    let put = |b: &mut [u8], off: usize, v: u32| {
        let a = match order {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        };
        b[off..off + 4].copy_from_slice(&a);
    };
    put(&mut img, 20, BS as u32);
    put(&mut img, 24, n as u32);
    put(&mut img, 28, BLOCK0_MAGIC);
    // Block 1: a minimal `kcvfh` — type 0x0B, declares its own block size,
    // a non-zero file number and an ASCII db name; tail + checksum valid.
    let mut h = synth_block(BS, order, 0x0B, (1 << 22) | 1, 1001, 1, seed);
    h[KCBH_LEN..BS - TAIL_LEN].fill(0);
    h[32..40].copy_from_slice(b"SYNTH   ");
    put(&mut h, 44, n as u32);
    put(&mut h, 48, BS as u32);
    h[52..54].copy_from_slice(&match order {
        ByteOrder::Little => 7u16.to_le_bytes(),
        ByteOrder::Big => 7u16.to_be_bytes(),
    });
    h[54..56].copy_from_slice(&match order {
        ByteOrder::Little => 3u16.to_le_bytes(),
        ByteOrder::Big => 3u16.to_be_bytes(),
    });
    h[16..18].fill(0); // chkval = fold of everything else, so clear the old one first
    let c = odr_core::block::xor_fold(&h);
    h[16..18].copy_from_slice(&c.to_le_bytes());
    debug_assert_eq!(odr_core::block::xor_fold(&h), 0);
    img.extend(h);
    for i in 2..=n {
        let mut b = synth_block(
            BS,
            order,
            0x06,
            (1 << 22) | i as u32,
            1000 + i as u32,
            1,
            seed ^ i as u64,
        );
        // Random bodies carry no row directory: a `ktbbhtyp` of 1 (by
        // chance, 1/256) would now be judged header-preserving encryption
        // (panel §1.1), which is correct — so make these blocks explicitly
        // "other data" (typ 3) and re-seal the checksum.
        b[0x14] = 3;
        b[16..18].fill(0);
        let c = odr_core::block::xor_fold(&b);
        b[16..18].copy_from_slice(&c.to_le_bytes());
        img.extend(b);
    }
    img
}

#[test]
fn verdict_covers_every_physical_damage() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut img = synth_image(16, BS, order, 7);
        // Re-address so block i claims position i.
        for i in 0..16 {
            let b = synth_block(BS, order, 0x06, (1 << 22) | i as u32, 500, 1, 7 ^ i as u64);
            img[i * BS..(i + 1) * BS].copy_from_slice(&b);
        }
        let blk = |i: usize| &img[i * BS..(i + 1) * BS];
        assert_eq!(
            verdict(blk(3), BS, order, 3, Depth::Physical).damage,
            Damage::None
        );
        assert_eq!(
            verdict(blk(3), BS, order, 0, Depth::Physical).kind,
            BlockKind::OsHeader
        );
        // misplaced: block 3 read at position 9
        assert_eq!(
            verdict(blk(3), BS, order, 9, Depth::Physical).damage,
            Damage::Misplaced { header_block: 3 }
        );
        let mut c = img.clone();
        corrupt(&mut c, BS, 4, Corruption::BitRot, 1);
        assert_eq!(
            verdict(&c[4 * BS..5 * BS], BS, order, 4, Depth::Physical).damage,
            Damage::ChecksumFail
        );
        let mut c = img.clone();
        corrupt(&mut c, BS, 5, Corruption::TornWrite, 1);
        assert_eq!(
            verdict(&c[5 * BS..6 * BS], BS, order, 5, Depth::Physical).damage,
            Damage::Torn
        );
        let mut c = img.clone();
        corrupt(&mut c, BS, 6, Corruption::Wipe, 1);
        assert_eq!(
            verdict(&c[6 * BS..7 * BS], BS, order, 6, Depth::Physical).damage,
            Damage::Zeroed
        );
        let mut c = img.clone();
        corrupt(&mut c, BS, 7, Corruption::Encrypt, 1);
        let v = verdict(&c[7 * BS..8 * BS], BS, order, 7, Depth::Physical);
        // Random bytes: encrypted, or (1/16) a header that happens to parse
        // and then fails tail/checksum — never clean.
        assert!(
            matches!(v.damage, Damage::Encrypted { entropy_milli, .. } if entropy_milli >= 7850)
                || matches!(v.damage, Damage::Torn | Damage::ChecksumFail),
            "{:?}",
            v.damage
        );
        let mut c = img.clone();
        c[8 * BS + 14] = 0xFF; // seq
        assert_eq!(
            verdict(&c[8 * BS..9 * BS], BS, order, 8, Depth::Physical).damage,
            Damage::MarkedCorrupt
        );
        let mut c = img.clone();
        c[9 * BS..9 * BS + KCBH_LEN].fill(0);
        assert_eq!(
            verdict(&c[9 * BS..10 * BS], BS, order, 9, Depth::Physical).damage,
            Damage::HeaderZeroed
        );
        // text over a block: garbage (low entropy, bad format nibble).
        // Byte 1 must not have nibble 2 ("ORACLE" would: 'R' = 0x52).
        let mut c = img.clone();
        for (k, x) in c[10 * BS..11 * BS].iter_mut().enumerate() {
            *x = b"The quick brown fox "[k % 20];
        }
        assert_ne!(c[10 * BS + 1] & 0x0F, 2);
        assert!(matches!(
            verdict(&c[10 * BS..11 * BS], BS, order, 10, Depth::Physical).damage,
            Damage::Garbage { .. }
        ));
        assert!(matches!(
            verdict(&img[..BS - 100], BS, order, 0, Depth::Physical).damage,
            Damage::Truncated { have } if have == BS - 100
        ));
    }
}

#[test]
fn layout_from_header_then_os_block_then_inference() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let img = datafile(200, order, 3);
        // Witness 1: the file header.
        let mut s: &[u8] = &img;
        let l = decide(&mut s, None).unwrap().unwrap();
        assert_eq!(
            (l.block_size, l.order, l.source),
            (BS, order, LayoutSource::FileHeader)
        );
        assert_eq!(l.declared_blocks_os, Some(200));
        assert_eq!(l.declared_blocks_header, Some(200));
        // Witness 2: wipe block 1 → block 0 decides.
        let mut w1 = img.clone();
        w1[BS..2 * BS].fill(0);
        let mut s: &[u8] = &w1;
        let l = decide(&mut s, None).unwrap().unwrap();
        assert_eq!(
            (l.block_size, l.order, l.source),
            (BS, order, LayoutSource::OsBlock)
        );
        // Witness 3: wipe blocks 0 and 1 → inference must still find 8192 / order.
        let mut wiped = img.clone();
        wiped[..2 * BS].fill(0);
        let mut s: &[u8] = &wiped;
        let l = decide(&mut s, None).unwrap().unwrap();
        assert_eq!(
            (l.block_size, l.order, l.source),
            (BS, order, LayoutSource::Inferred)
        );
        let inf = l.inference.unwrap();
        assert!(inf.accepted);
        let best = inf.candidates[0];
        assert_eq!((best.block_size, best.order), (BS, order));
        assert!(best.score_milli() >= 900, "{best:?}");
        // Every other candidate scores far lower.
        for c in &inf.candidates[1..] {
            assert!(
                c.score_milli() + 250 <= best.score_milli(),
                "{c:?} vs {best:?}"
            );
        }
    }
}

#[test]
fn inference_refuses_non_oracle_bytes() {
    let mut rng = SplitMix64(99);
    let random: Vec<u8> = (0..1 << 20).map(|_| rng.next_u8()).collect();
    let mut s: &[u8] = &random;
    let inf = infer(&mut s, 64, 256 * 1024).unwrap();
    assert!(!inf.accepted, "{:?}", inf.candidates[0]);
    let text: Vec<u8> = b"The quick brown fox jumps over the lazy dog.\n"
        .iter()
        .cycle()
        .take(1 << 20)
        .copied()
        .collect();
    let mut s: &[u8] = &text;
    assert!(decide(&mut s, None).unwrap().is_none());
}

#[test]
fn regions_merge_and_findings_name_the_pattern() {
    let order = ByteOrder::Little;
    let img = datafile(400, order, 11);
    let opts = Options {
        depth: Depth::Physical,
        ..Options::default()
    };
    // Healthy.
    let mut s: &[u8] = &img;
    let r = diagnose(&mut s, None, opts).unwrap().unwrap();
    assert_eq!(r.counters.examined, 400);
    // synth_block bodies are random: `ktbbhtyp` is 1 only by chance, so the
    // blocks split between Data and DataOther — all clean and in use.
    assert_eq!(r.counters.used_clean(), 400);
    assert_eq!(r.layout.source, LayoutSource::FileHeader);
    assert_eq!(r.counters.empty, 0);
    assert_eq!(r.counters.damaged_total(), 0);
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].code, FindingCode::Healthy);
    assert!(!r.has_damage());
    // Ransomware head: blocks 0..=40 random.
    let mut c = img.clone();
    let mut rng = SplitMix64(5);
    for x in &mut c[..41 * BS] {
        *x = rng.next_u8();
    }
    let mut s: &[u8] = &c;
    let r = diagnose(&mut s, None, opts).unwrap().unwrap();
    assert_eq!(r.layout.source, LayoutSource::Inferred);
    let codes: Vec<_> = r.findings.iter().map(|f| f.code).collect();
    assert!(codes.contains(&FindingCode::RansomwareHead), "{codes:?}");
    assert!(codes.contains(&FindingCode::HeaderDestroyed), "{codes:?}");
    assert!(
        !codes.contains(&FindingCode::BitRot),
        "interleaved verdicts must be absorbed: {codes:?}"
    );
    let head = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::RansomwareHead)
        .unwrap();
    assert_eq!(head.blocks, 40, "blocks 1..=40: {:?}", head.evidence);
    assert_eq!(head.severity, Severity::Critical);
    // Periodic: 8 blocks random every 50, starting at 60.
    let mut c = img.clone();
    for k in 0..6 {
        let start = (60 + 50 * k) * BS;
        for x in &mut c[start..start + 8 * BS] {
            *x = rng.next_u8();
        }
    }
    let mut s: &[u8] = &c;
    let r = diagnose(&mut s, None, opts).unwrap().unwrap();
    let per = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::RansomwarePeriodic)
        .expect("periodic");
    assert_eq!(per.blocks, 48);
    assert!(
        per.evidence[0].contains("every 50 blocks"),
        "{:?}",
        per.evidence
    );
    // Zeroed region + bit rot + torn + misplaced all at once, each named.
    let mut c = img.clone();
    c[100 * BS..110 * BS].fill(0);
    corrupt(&mut c, BS, 150, Corruption::BitRot, 1);
    corrupt(&mut c, BS, 151, Corruption::BitRot, 2);
    corrupt(&mut c, BS, 200, Corruption::TornWrite, 1);
    let (a, b) = (
        c[300 * BS..301 * BS].to_vec(),
        c[301 * BS..302 * BS].to_vec(),
    );
    c[300 * BS..301 * BS].copy_from_slice(&b);
    c[301 * BS..302 * BS].copy_from_slice(&a);
    let mut s: &[u8] = &c;
    let r = diagnose(&mut s, None, opts).unwrap().unwrap();
    assert_eq!(r.regions.len(), 4, "{:?}", r.regions);
    assert_eq!(
        (
            r.regions[0].start,
            r.regions[0].len,
            r.regions[0].code.as_str()
        ),
        (100, 10, "zeroed")
    );
    assert_eq!(
        (
            r.regions[1].start,
            r.regions[1].len,
            r.regions[1].code.as_str()
        ),
        (150, 2, "checksum_fail")
    );
    assert_eq!(
        (
            r.regions[2].start,
            r.regions[2].len,
            r.regions[2].code.as_str()
        ),
        (200, 1, "torn")
    );
    assert_eq!(
        (
            r.regions[3].start,
            r.regions[3].len,
            r.regions[3].code.as_str()
        ),
        (300, 2, "misplaced")
    );
    let codes: Vec<_> = r.findings.iter().map(|f| f.code).collect();
    for want in [
        FindingCode::ZeroedRegions,
        FindingCode::BitRot,
        FindingCode::TornWrites,
        FindingCode::MisdirectedWrites,
    ] {
        assert!(codes.contains(&want), "{codes:?}");
    }
    assert_eq!(r.counters.damaged_total(), 15);
    assert_eq!(r.damaged_blocks.len(), 15);
    assert!(r.has_damage());
    // Truncation: cut inside block 399.
    let mut c = img.clone();
    c.truncate(400 * BS + 100);
    let mut s: &[u8] = &c;
    let r = diagnose(&mut s, None, opts).unwrap().unwrap();
    assert!(r.last_block_partial);
    let t = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::Truncated)
        .unwrap();
    assert_eq!(t.blocks, 1);
    // Shorter than declared.
    let mut c = img.clone();
    c.truncate(300 * BS);
    let mut s: &[u8] = &c;
    let r = diagnose(&mut s, None, opts).unwrap().unwrap();
    let t = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::Truncated)
        .unwrap();
    assert_eq!(t.blocks, 101, "declared 400, 299 present after block 0");
}

#[test]
fn hash_is_of_the_scanned_bytes_and_records_are_capped() {
    let img = datafile(64, ByteOrder::Little, 1);
    let mut c = img.clone();
    for i in 1..=20 {
        corrupt(&mut c, BS, i, Corruption::Wipe, 1);
    }
    let mut s: &[u8] = &c;
    let l = decide(&mut s, None).unwrap().unwrap();
    let r = run(
        &mut s,
        l,
        Options {
            depth: Depth::Physical,
            max_block_records: 5,
            ..Options::default()
        },
    )
    .unwrap();
    use sha2::Digest as _;
    let want = sha2::Sha256::digest(&c);
    let want_hex: String = want.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(r.sha256_scanned, want_hex);
    assert_eq!(r.counters.damaged_total(), 20);
    assert_eq!(r.damaged_blocks.len(), 5);
    assert!(r.damaged_blocks_truncated);
    assert_eq!(r.regions.len(), 1);
    assert_eq!(r.regions[0].len, 20);
}

#[test]
fn tail_bytes_never_panic_at_any_length() {
    let img = datafile(8, ByteOrder::Little, 2);
    for cut in [
        0usize,
        1,
        19,
        20,
        23,
        24,
        100,
        BS - 1,
        BS,
        BS + 1,
        3 * BS + 7,
    ] {
        let mut s: &[u8] = &img[..cut.min(img.len())];
        let _ = diagnose(&mut s, Some((BS, ByteOrder::Little)), Options::default()).unwrap();
        let _ = diagnose(&mut s, None, Options::default()).unwrap();
    }
    let _ = TAIL_LEN;
}
