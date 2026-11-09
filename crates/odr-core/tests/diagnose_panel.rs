//! Agent-panel consensus verdicts (`docs/research/06_agent_panel_synthesis_ar.md` §1):
//! header-preserving encryption, stale SCN, duplicate `rdba`, checksum
//! coverage, tail footer, ransomware parameters — on synthetic images.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::arithmetic_side_effects,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::many_single_char_names,
    clippy::items_after_statements
)]

use odr_core::ByteOrder;
use odr_core::block::{KCBH_LEN, TAIL_LEN, xor_fold};
use odr_core::diagnose::layout::BLOCK0_MAGIC;
use odr_core::diagnose::{
    BlockKind, ConsistencyPass, Damage, Depth, FindingCode, Options, diagnose, verdict,
};
use odr_testkit::{SplitMix64, synth_block};

const BS: usize = 8192;
const LE: ByteOrder = ByteOrder::Little;

fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

/// Re-seal a block after editing: recompute the checksum so the fold is 0.
fn seal(b: &mut [u8]) {
    b[16..18].fill(0);
    let c = xor_fold(b);
    b[16..18].copy_from_slice(&c.to_le_bytes());
    debug_assert_eq!(xor_fold(b), 0);
}

/// A *real* table data block: `ktbbh` (typ 1, objd, 2 ITLs) + MSSM `kdbh`
/// with `nrow` one-byte dummy rows, the rest of the body filled by `fill`.
fn data_block(
    index: u32,
    objd: u32,
    scn: u32,
    nrow: u16,
    mut fill: impl FnMut(&mut [u8]),
) -> Vec<u8> {
    let mut b = synth_block(BS, LE, 0x06, (1 << 22) | index, scn, 1, u64::from(index));
    // ktbbh @0x14
    b[0x14] = 1; // typ = data
    put32(&mut b, 0x18, objd);
    put32(&mut b, 0x1C, scn); // cleanout scn base
    put16(&mut b, 0x20, 0);
    b[0x24] = 2; // ITL count
    b[0x26] = 0;
    put32(&mut b, 0x28, 0);
    // ITLs are 24 bytes each from 0x2C: zero them.
    let itl_end = 0x2C + 24 * 2;
    b[0x2C..itl_end].fill(0);
    // kdbh @ itl_end (MSSM): flag, ntab=1, nrow, frre=0xFFFF, fsbo, fseo, avsp, tosp
    let kdbh = itl_end;
    let dir_end = 14 + 4 + 2 * usize::from(nrow);
    let space = BS - TAIL_LEN - kdbh;
    b[kdbh] = 0;
    b[kdbh + 1] = 1;
    put16(&mut b, kdbh + 2, nrow);
    put16(&mut b, kdbh + 4, 0xFFFF);
    put16(&mut b, kdbh + 6, dir_end as u16);
    // rows live at the very end of the body: one 3-byte row each
    // (flag, lock, cc) — enough to decode as a head piece with 0 columns.
    let row_len = 3usize;
    let rows_start = space - row_len * usize::from(nrow);
    put16(&mut b, kdbh + 8, rows_start as u16); // fseo
    // kdbchk 6110 counts every row at >= 9 bytes when bounding `avsp`.
    let avsp = (space - dir_end - 9 * usize::from(nrow)) as u16;
    put16(&mut b, kdbh + 10, avsp);
    put16(&mut b, kdbh + 12, avsp); // tosp
    // table directory
    put16(&mut b, kdbh + 14, 0);
    put16(&mut b, kdbh + 16, nrow);
    // row directory + rows
    for r in 0..usize::from(nrow) {
        let off = rows_start + r * row_len;
        put16(&mut b, kdbh + 18 + 2 * r, off as u16);
        b[kdbh + off] = 0x2C; // H|F|L head piece
        b[kdbh + off + 1] = 0; // lock
        b[kdbh + off + 2] = 0; // cc
    }
    // fill the free gap
    let gap = kdbh + dir_end..kdbh + rows_start;
    fill(&mut b[gap]);
    seal(&mut b);
    b
}

/// OS block 0 + `kcvfh` block 1 + `n` data blocks of `objd` with SCNs from
/// `scn_of`, bodies filled by `fill`.
fn image(
    n: u32,
    objd: u32,
    scn_of: impl Fn(u32) -> u32,
    fill: impl Fn(&mut [u8]) + Copy,
) -> Vec<u8> {
    let mut img = vec![0u8; BS];
    put32(&mut img, 20, BS as u32);
    put32(&mut img, 24, n + 1);
    put32(&mut img, 28, BLOCK0_MAGIC);
    let mut h = synth_block(BS, LE, 0x0B, (1 << 22) | 1, 1001, 1, 9);
    h[KCBH_LEN..BS - TAIL_LEN].fill(0);
    h[32..40].copy_from_slice(b"SYNTH   ");
    put32(&mut h, 44, n + 1);
    put32(&mut h, 48, BS as u32);
    put16(&mut h, 52, 7);
    put16(&mut h, 54, 3);
    seal(&mut h);
    img.extend(h);
    for i in 2..=n + 1 {
        img.extend(data_block(i, objd, scn_of(i), 4, fill));
    }
    img
}

fn zero_fill(b: &mut [u8]) {
    b.fill(0);
}

fn run(img: &[u8]) -> odr_core::diagnose::Report {
    let mut s: &[u8] = img;
    diagnose(&mut s, None, Options::default())
        .unwrap()
        .unwrap()
        .with_triage("t.dbf")
}

#[test]
fn structured_data_block_is_clean_at_logical_depth() {
    let b = data_block(5, 70000, 500, 4, zero_fill);
    let v = verdict(&b, BS, LE, 5, Depth::Logical);
    assert_eq!(v.damage, Damage::None, "{v:?}");
    assert_eq!(v.kind, BlockKind::Data);
    assert_eq!(v.objd, Some(70000));
    assert_eq!(v.live_rows, Some(4));
    assert!(v.checksum_present);
    assert_eq!(v.rdba, Some((1 << 22) | 5));
}

#[test]
fn payload_scrambled_needs_random_payload_and_no_structure() {
    // 1. Random payload but a *decodable* row directory: not scrambled.
    //    (Compressed / binary payloads look like this.)
    let mut rng = SplitMix64(77);
    let b = data_block(5, 70000, 500, 4, |g| {
        for x in g.iter_mut() {
            *x = rng.next_u8();
        }
    });
    let v = verdict(&b, BS, LE, 5, Depth::Logical);
    assert_eq!(
        v.damage,
        Damage::None,
        "decodable structure must win: {v:?}"
    );

    // 2. Header-preserving ransomware: kcbh + ktbbh intact, everything from
    //    the ITL end to the tail replaced by ciphertext, checksum recomputed
    //    by the attacker so dbv would pass it.
    let mut b = data_block(5, 70000, 500, 4, zero_fill);
    let mut rng = SplitMix64(78);
    for x in &mut b[0x2C + 48..BS - TAIL_LEN] {
        *x = rng.next_u8();
    }
    seal(&mut b);
    let v = verdict(&b, BS, LE, 5, Depth::Physical);
    assert!(
        matches!(v.damage, Damage::PayloadScrambled { entropy_milli, chi_square } if entropy_milli >= 7850 && chi_square <= 330),
        "{v:?}"
    );
    assert_eq!(v.damage.code(), "payload_scrambled");

    // 3. Same block without the checksum flag: still caught (coverage gap
    //    must not hide it).
    b[15] = 0;
    let v = verdict(&b, BS, LE, 5, Depth::Physical);
    assert!(matches!(v.damage, Damage::PayloadScrambled { .. }), "{v:?}");
    assert!(!v.checksum_present);
}

#[test]
fn payload_scrambled_surfaces_as_finding_and_deduction() {
    let img = image(64, 70000, |_| 5000, zero_fill);
    let mut c = img.clone();
    let mut rng = SplitMix64(5);
    for i in 10..30u32 {
        let off = i as usize * BS;
        for x in &mut c[off + 0x2C + 48..off + BS - TAIL_LEN] {
            *x = rng.next_u8();
        }
        seal(&mut c[off..off + BS]);
    }
    let r = run(&c);
    assert_eq!(r.counters.damaged.get("payload_scrambled"), Some(&20));
    let f = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::PayloadScrambled)
        .expect("PAYLOAD_SCRAMBLED finding");
    assert_eq!(f.blocks, 20);
    assert!(f.evidence.iter().any(|e| e.contains("χ²")));
    let t = r.triage.as_ref().unwrap();
    assert!(
        t.deductions
            .iter()
            .any(|d| d.reason_en.contains("encrypted payloads"))
    );
    assert!(
        t.next_steps
            .iter()
            .any(|s| s.purpose_en.starts_with("Ransomware"))
    );
    // Loss is quantified: 20 of 64 used blocks are data → ≈ 312‰.
    assert!(
        (300..=330).contains(&t.expected_loss_milli),
        "{}",
        t.expected_loss_milli
    );
    assert_eq!(t.bytes_affected, 20 * BS as u64);
}

#[test]
fn stale_scn_is_a_post_pass_outlier_per_object() {
    // 100 blocks at SCN ≈ 50_000_000 ± small jitter; block 40 at SCN 1_000.
    let img = image(
        100,
        70000,
        |i| {
            if i == 40 {
                1_000
            } else {
                50_000_000 + (i % 7) * 10
            }
        },
        zero_fill,
    );
    let r = run(&img);
    assert_eq!(r.consistency_pass, ConsistencyPass::Complete);
    assert_eq!(
        r.counters.damaged.get("stale_scn"),
        Some(&1),
        "{:?}",
        r.counters.damaged
    );
    let d = r.damaged_blocks.iter().find(|b| b.index == 40).unwrap();
    assert!(matches!(d.damage, Damage::StaleScn { scn: 1_000, median } if median >= 50_000_000));
    // Reclassified: counters stay consistent.
    assert_eq!(r.counters.examined, 101);
    assert_eq!(r.counters.used_clean() + r.counters.damaged_total(), 101);
    let o = &r.objects[&70000];
    assert_eq!(o.blocks, 99);
    assert_eq!(o.logical_faults, 1);
    assert_eq!(o.live_rows, Some(99 * 4));
    let codes: Vec<_> = r.findings.iter().map(|f| f.code).collect();
    assert!(codes.contains(&FindingCode::StaleBlocks), "{codes:?}");
    assert!(
        r.regions
            .iter()
            .any(|g| g.code == "stale_scn" && g.start == 40 && g.len == 1)
    );

    // A legitimately wide but *continuous* spread is not stale: SCN grows
    // 2M per block; MAD is large so the gap rule scales with it.
    let img = image(100, 70000, |i| i * 2_000_000, zero_fill);
    let r = run(&img);
    assert_eq!(
        r.counters.damaged.get("stale_scn"),
        None,
        "{:?}",
        r.counters.damaged
    );

    // Too few blocks for a median (< 8): never judged.
    let img = image(6, 70000, |i| if i == 3 { 1 } else { 90_000_000 }, zero_fill);
    let r = run(&img);
    assert_eq!(r.counters.damaged.get("stale_scn"), None);
}

#[test]
fn duplicated_block_content_is_caught_by_placement_first() {
    // A block copied to another position keeps its rdba, so the per-block
    // placement check calls it `misplaced` before the post-pass ever sees
    // it. `duplicate_rdba` is the safety net for the one case placement
    // cannot see: the same 32-bit rdba fitting two positions under the
    // smallfile *and* bigfile splits, which needs a file ≥ 2^22 blocks —
    // covered by the unit test on the post-pass in `diagnose::mod`.
    let img = image(50, 70000, |_| 7_000, zero_fill);
    let mut c = img.clone();
    let src = c[12 * BS..13 * BS].to_vec();
    c[30 * BS..31 * BS].copy_from_slice(&src);
    let r = run(&c);
    assert_eq!(r.counters.damaged.get("misplaced"), Some(&1));
    assert_eq!(r.counters.damaged.get("duplicate_rdba"), None);
    assert_eq!(r.consistency_pass, ConsistencyPass::Complete);
    let d = r.damaged_blocks.iter().find(|b| b.index == 30).unwrap();
    assert_eq!(d.damage, Damage::Misplaced { header_block: 12 });
}

#[test]
fn checksum_coverage_is_disclosed() {
    let img = image(40, 70000, |_| 7_000, zero_fill);
    let r = run(&img);
    assert_eq!(r.counters.checksum_coverage_milli(), Some(1000));
    assert!(
        !r.findings
            .iter()
            .any(|f| f.code == FindingCode::ChecksumCoverageLow)
    );

    // Clear the checksum flag on half the data blocks (DB_BLOCK_CHECKSUM=OFF era).
    let mut c = img.clone();
    for i in 2..22usize {
        let off = i * BS;
        c[off + 15] = 0;
        // chkval is ignored when the flag is clear; leave it.
    }
    let r = run(&c);
    assert_eq!(r.counters.damaged_total(), 0);
    assert_eq!(r.counters.used_clean(), 41); // 40 data + the kcvfh block
    assert_eq!(r.counters.checksum_flagged, 21);
    assert_eq!(r.counters.checksum_coverage_milli(), Some(512));
    let f = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::ChecksumCoverageLow)
        .expect("CHECKSUM_COVERAGE_LOW");
    assert_eq!(f.blocks, 20);
    assert_eq!(f.parameters, vec![("coverage_milli".to_owned(), 512)]);
    let t = r.triage.as_ref().unwrap();
    assert_eq!(t.checksum_coverage_milli, Some(512));
    assert!(t.score < 100);
}

#[test]
fn tail_footer_is_excerpted() {
    let img = image(40, 70000, |_| 7_000, zero_fill);
    let mut c = img.clone();
    // Ransomware footer: 2 blocks of ciphertext + a printable marker.
    let mut rng = SplitMix64(3);
    let n = c.len();
    for x in &mut c[n - 2 * BS..] {
        *x = rng.next_u8();
    }
    c[n - 2 * BS..n - 2 * BS + 16].copy_from_slice(b"LOCKED-BY-X 0001");
    c[n - 2 * BS + 1] = 0x00; // make sure the format nibble is not 2
    let r = run(&c);
    let t = r.tail_excerpt.as_ref().expect("tail excerpt");
    assert_eq!(t.start_block, 40);
    assert_eq!(t.blocks, 2);
    assert!(t.ascii.starts_with('L'), "{}", t.ascii);
    assert!(t.hex.starts_with("4c "), "{}", t.hex);
    let f = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::TailFooter)
        .expect("TAIL_FOOTER");
    assert_eq!(f.blocks, 2);
    assert!(f.evidence.iter().any(|e| e.contains("CKED-BY-X 0001")));
    assert_eq!(f.parameters[0], ("tail_blocks".to_owned(), 2));

    // A damaged region *followed by clean blocks* is not a tail.
    let mut c = img.clone();
    for x in &mut c[10 * BS..12 * BS] {
        *x = rng.next_u8();
    }
    let r = run(&c);
    assert!(r.tail_excerpt.is_none());
    // A merely truncated last block is not a footer either.
    let r = run(&img[..img.len() - 100]);
    assert!(r.tail_excerpt.is_none());
}

#[test]
fn ransomware_findings_carry_parameters_not_family_names() {
    let img = image(400, 70000, |_| 7_000, zero_fill);
    let mut c = img.clone();
    let mut rng = SplitMix64(5);
    for x in &mut c[..33 * BS] {
        *x = rng.next_u8();
    }
    let r = run(&c);
    let f = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::RansomwareHead)
        .expect("RANSOMWARE_HEAD");
    let p = |k: &str| f.parameters.iter().find(|(n, _)| n == k).map(|(_, v)| *v);
    assert_eq!(p("head_blocks"), Some(33));
    assert_eq!(p("head_bytes"), Some(33 * BS as u64));
    assert!(!f.cause_en.to_lowercase().contains("lockbit"));
    assert!(!f.cause_en.to_lowercase().contains("conti"));

    // Periodic: 8 blocks every 50, from block 60.
    let mut c = img.clone();
    for k in 0..6u32 {
        let start = (60 + 50 * k) as usize;
        for x in &mut c[start * BS..(start + 8) * BS] {
            *x = rng.next_u8();
        }
    }
    let r = run(&c);
    let f = r
        .findings
        .iter()
        .find(|f| f.code == FindingCode::RansomwarePeriodic)
        .expect("RANSOMWARE_PERIODIC");
    let p = |k: &str| f.parameters.iter().find(|(n, _)| n == k).map(|(_, v)| *v);
    assert_eq!(p("stride_blocks"), Some(50));
    assert_eq!(p("span_blocks"), Some(8));
    assert_eq!(p("spans_in_phase"), Some(6));
}

#[test]
fn report_json_round_trips_new_fields() {
    let img = image(20, 70000, |_| 7_000, zero_fill);
    let r = run(&img);
    let s = serde_json::to_string(&r).unwrap();
    assert!(s.contains("\"consistency_pass\":\"complete\""));
    assert_eq!(r.counters.checksum_flagged, r.counters.used_clean());
    assert!(s.contains("\"checksum_flagged\":"));
    assert!(s.contains("\"expected_loss_milli\":0"));
    let back: odr_core::diagnose::Report = serde_json::from_str(&s).unwrap();
    assert_eq!(back, r);
}
