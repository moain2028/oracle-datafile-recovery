//! Golden tests against **real Oracle datafiles** produced by `lab/` (ADR-0004).
//!
//! They run only when `ODR_GOLDEN` points at a `lab/out/golden/<run>/`
//! directory (the files are never committed); otherwise every test is a
//! no-op so CI stays green. Each assertion compares ODR's decoding with
//! what Oracle itself reported (`meta/*.txt`, `rows.psv`, `x$kcvfh`).
//!
//! Lab run used while writing these: Oracle AI Database 26ai Free 23.26,
//! AL32UTF8, PDB FREEPDB1; SYSTEM is a **bigfile** tablespace (file 12,
//! rfile 1024), `ODR_ASSM`/`ODR_MSSM` smallfile (files 16/17), `ODR_BIG` bigfile.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::panic,
    clippy::too_many_lines,
    clippy::items_after_statements,
    clippy::assert_is_empty,
    clippy::type_complexity,
    clippy::semicolon_if_nothing_returned,
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::cognitive_complexity
)]

use odr_core::ByteOrder;
use odr_core::block::{BlockStatus, Rdba, RdbaScheme, classify};
use odr_core::charset::Charset;
use odr_core::dict::bootstrap::{Catalog, ObjectKind, Storage};
use odr_core::dict::kcvfh::FileHeader;
use odr_core::dict::segment::{ExtentMap, MapKind, walk_segment};
use odr_core::row::DataBlock;
use odr_core::types::OraNumber;
use std::path::PathBuf;

const BS: usize = 8192;

fn golden() -> Option<PathBuf> {
    std::env::var_os("ODR_GOLDEN").map(PathBuf::from)
}

fn read_file(p: &std::path::Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn block(img: &[u8], n: u32) -> &[u8] {
    &img[n as usize * BS..(n as usize + 1) * BS]
}

#[test]
fn lab_kcvfh_matches_x_kcvfh() {
    let Some(g) = golden() else { return };
    let img = read_file(&g.join("datafiles/FREE_FREEPDB1_system01.dbf"));
    let h = FileHeader::detect(block(&img, 1)).unwrap();
    assert_eq!(h.order, ByteOrder::Little);
    assert_eq!(h.block_size, 8192);
    assert_eq!(h.file_number, 12, "dba_data_files.file_id");
    assert_eq!(h.file_type, 3);
    assert_eq!(h.db_name, "FREE");
    assert_eq!(h.file_size_blocks, 37120, "dba_data_files.blocks");
    // x$kcvfh: fhrdb = 520, fhrfn = 1024 (bigfile), fhtnm = SYSTEM
    assert_eq!(h.root_dba, Rdba(520));
    assert_eq!(h.root_dba.split(RdbaScheme::Bigfile), (None, 520));
    assert_eq!(
        h.relative_file, None,
        "1024 is outside 1..=1023 => bigfile marker, not a file"
    );
    assert_eq!(h.tablespace, Some(0));
    assert_eq!(h.tablespace_name.as_deref(), Some("SYSTEM"));
    // Every ODR datafile header decodes and names its tablespace.
    for (f, fno, ts) in [
        ("odr_odr_assm01", 16u16, "ODR_ASSM"),
        ("odr_odr_mssm01", 17, "ODR_MSSM"),
        ("odr_odr_big01", 18, "ODR_BIG"),
    ] {
        let img = read_file(&g.join(format!("datafiles/{f}.dbf")));
        let h = FileHeader::detect(block(&img, 1)).unwrap();
        assert_eq!(
            (h.file_number, h.tablespace_name.as_deref()),
            (fno, Some(ts))
        );
        assert_eq!(
            h.root_dba,
            Rdba(520),
            "x$kcvfh.fhrdb is 520 in every file of the PDB"
        );
    }
}

#[test]
fn lab_every_block_classifies_valid() {
    let Some(g) = golden() else { return };
    for f in [
        "FREE_FREEPDB1_system01",
        "odr_odr_assm01",
        "odr_odr_mssm01",
        "odr_odr_big01",
    ] {
        let img = read_file(&g.join(format!("datafiles/{f}.dbf")));
        let n = img.len() / BS;
        let bad: Vec<usize> = (1..n)
            .filter(|&i| {
                classify(block(&img, i as u32), BS, ByteOrder::Little) != BlockStatus::Valid
            })
            .filter(|&i| !block(&img, i as u32).iter().all(|&b| b == 0))
            .collect();
        assert!(bad.is_empty(), "{f}: non-valid non-zero blocks {bad:?}");
    }
}

/// LV-062 — *the structural check has zero false positives on real data.*
///
/// Every `Valid` table data block of the four golden files (26ai: 17,683 in
/// SYSTEM incl. 2,254 reused deleted stubs and 722 free-slot chains, 394
/// ASSM, 6 MSSM, 1 bigfile) must pass `DataBlock::structural_check`. If a
/// future rule fails here it is wrong, not the file.
#[test]
fn lab_structural_check_passes_every_healthy_block() {
    let Some(g) = golden() else { return };
    let mut checked = 0usize;
    for f in [
        "FREE_FREEPDB1_system01",
        "odr_odr_assm01",
        "odr_odr_mssm01",
        "odr_odr_big01",
    ] {
        let img = read_file(&g.join(format!("datafiles/{f}.dbf")));
        let mut faults = Vec::new();
        for i in 1..img.len() / BS {
            let b = block(&img, i as u32);
            if classify(b, BS, ByteOrder::Little) != BlockStatus::Valid {
                continue;
            }
            let Ok(h) = odr_core::block::BlockHeader::parse(b, ByteOrder::Little) else {
                continue;
            };
            if h.type_byte != 0x06 {
                continue;
            }
            let Ok(db) = DataBlock::parse(b, ByteOrder::Little) else {
                continue;
            };
            checked += 1;
            if let Some(fault) = db.structural_check() {
                faults.push((i, fault));
            }
        }
        assert!(faults.is_empty(), "{f}: {faults:?}");
    }
    assert!(checked >= 18_000, "checked {checked} data blocks");
}

#[test]
fn lab_bootstrap_segment_and_catalog() {
    let Some(g) = golden() else { return };
    let img = read_file(&g.join("datafiles/FREE_FREEPDB1_system01.dbf"));
    let h = FileHeader::detect(block(&img, 1)).unwrap();
    let (_, root) = h.root_dba.split(RdbaScheme::Bigfile);
    // Segment header of bootstrap$ (dba_segments: 12/520; obj$: obj# 59).
    let m = ExtentMap::parse(block(&img, root), ByteOrder::Little).unwrap();
    assert_eq!(
        m.kind,
        MapKind::FreelistHeader,
        "SYSTEM is MSSM even in 26ai"
    );
    assert_eq!(m.objd, 59);
    assert_eq!((m.total_extents, m.total_blocks), (Some(1), Some(7)));
    let seg = walk_segment(block(&img, root), ByteOrder::Little, h.root_dba, |r| {
        let (_, b) = r.split(RdbaScheme::Bigfile);
        img.get(b as usize * BS..(b as usize + 1) * BS)
            .map(<[u8]>::to_vec)
    })
    .unwrap();
    assert!(seg.issues.is_empty(), "{:?}", seg.issues);
    let blocks: Vec<u32> = seg.blocks(RdbaScheme::Bigfile).map(|(_, b)| b).collect();
    assert_eq!(blocks, (521..528).collect::<Vec<_>>());

    // Rows of bootstrap$ across its data blocks -> catalog. Oracle: 60 rows.
    let mut rows = Vec::new();
    let mut data_blocks = 0;
    for b in &blocks {
        let blk = block(&img, *b);
        if blk[0] != 0x06 {
            continue;
        }
        data_blocks += 1;
        let db =
            DataBlock::parse(blk, ByteOrder::Little).unwrap_or_else(|e| panic!("block {b}: {e}"));
        assert_eq!(db.ktb.objd, 59);
        for (slot, r) in db.rows() {
            let r = r.unwrap_or_else(|e| panic!("block {b} slot {slot}: {e}"));
            assert!(r.is_complete(), "bootstrap$ rows are never chained");
            rows.push(r);
        }
    }
    assert!(data_blocks >= 1);
    assert_eq!(rows.len(), 60, "SELECT COUNT(*) FROM bootstrap$");
    // LINE# of the first row in the first block is a small integer.
    assert!(
        OraNumber::decode(rows[0].columns[0].unwrap())
            .unwrap()
            .to_i64()
            .is_some()
    );

    let cat = Catalog::from_rows(rows, Charset::Al32Utf8);
    assert!(cat.issues.is_empty(), "{:?}", cat.issues);
    assert!(cat.conflicts.is_empty());
    assert_eq!(cat.version.as_deref(), Some("8.0.0.0.0"));
    assert_eq!(cat.objects.len(), 59, "60 rows minus the version marker");
    // Fixed object numbers (LV-027) and the lab's dba_segments header blocks.
    // LV-042: in a bigfile SYSTEM the bootstrap EXTENTS clause says `FILE 1024`
    // (the bigfile relative-file marker), not `FILE 1`.
    const F: u32 = 1024;
    let obj = cat.by_name("OBJ$").unwrap();
    assert_eq!((obj.obj, obj.kind), (18, ObjectKind::Table));
    assert_eq!(
        obj.storage,
        Storage::Segment {
            file: F,
            block: 240
        }
    );
    let cobj = cat.by_name("C_OBJ#").unwrap();
    assert_eq!(
        (cobj.obj, cobj.storage.clone()),
        (
            2,
            Storage::Segment {
                file: F,
                block: 144
            }
        )
    );
    assert_eq!(
        cat.by_name("C_USER#").unwrap().storage,
        Storage::Segment {
            file: F,
            block: 208
        }
    );
    assert_eq!(
        cat.by_name("C_TS#").unwrap().storage,
        Storage::Segment {
            file: F,
            block: 176
        }
    );
    let tab = cat.by_name("TAB$").unwrap();
    assert!(
        matches!(&tab.storage, Storage::Clustered { cluster, tabno: 1, .. } if cluster == "C_OBJ#")
    );
    let col = cat.by_name("COL$").unwrap();
    assert!(
        matches!(&col.storage, Storage::Clustered { cluster, tabno: 5, .. } if cluster == "C_OBJ#")
    );
    let boot = cat.by_name("BOOTSTRAP$").unwrap();
    assert_eq!(
        (boot.obj, boot.storage.clone()),
        (
            59,
            Storage::Segment {
                file: F,
                block: 520
            }
        )
    );
    // Column lists come from the file (LV-038): sanity on the ones the resolver needs.
    let names = |n: &str| {
        cat.by_name(n)
            .unwrap()
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(&names("OBJ$")[..4], &["OBJ#", "DATAOBJ#", "OWNER#", "NAME"]);
    assert!(names("TAB$").contains(&"CLUCOLS".to_owned()));
    assert!(names("COL$").contains(&"SEGCOL#".to_owned()));
    assert!(names("COL$").contains(&"PROPERTY".to_owned()));
    assert_eq!(names("USER$")[1], "NAME");
    assert_eq!(names("TS$")[1], "NAME");
    eprintln!(
        "26ai bootstrap$: OBJ$ {} cols, TAB$ {} cols, COL$ {} cols, USER$ {} cols",
        names("OBJ$").len(),
        names("TAB$").len(),
        names("COL$").len(),
        names("USER$").len()
    );
}

#[test]
fn lab_corpus_segment_headers_match_dba_extents() {
    let Some(g) = golden() else { return };
    // segments.txt: NAME|obj#|dataobj#|TYPE|TS|header_file|header_block ; extents.txt: NAME|ext#|file|block|blocks
    let segs = std::fs::read_to_string(g.join("meta/segments.txt")).unwrap();
    let exts = std::fs::read_to_string(g.join("meta/extents.txt")).unwrap();
    let files: std::collections::HashMap<u32, Vec<u8>> = [
        (16u32, "odr_odr_assm01"),
        (17, "odr_odr_mssm01"),
        (18, "odr_odr_big01"),
    ]
    .into_iter()
    .map(|(n, f)| (n, read_file(&g.join(format!("datafiles/{f}.dbf")))))
    .collect();
    let mut checked = 0;
    for line in segs.lines() {
        let p: Vec<&str> = line.split('|').collect();
        if p.len() < 7 {
            continue;
        }
        let (name, dataobj, file, hb): (&str, u32, u32, u32) = (
            p[0],
            p[2].parse().unwrap(),
            p[5].parse().unwrap(),
            p[6].parse().unwrap(),
        );
        let img = &files[&file];
        let scheme = if file == 18 {
            RdbaScheme::Bigfile
        } else {
            RdbaScheme::Smallfile
        };
        let m = ExtentMap::parse(block(img, hb), ByteOrder::Little)
            .unwrap_or_else(|e| panic!("{name} header {file}/{hb}: {e}"));
        let expected_kind = if file == 17 {
            MapKind::FreelistHeader
        } else {
            MapKind::PagetableHeader
        };
        assert_eq!(m.kind, expected_kind, "{name}");
        assert_eq!(
            m.objd, dataobj,
            "{name}: obj# in map header = DATA_OBJECT_ID"
        );
        let truth: Vec<(u32, u32)> = exts
            .lines()
            .filter_map(|l| {
                let q: Vec<&str> = l.split('|').collect();
                (q.len() == 5 && q[0] == name)
                    .then(|| (q[3].parse().unwrap(), q[4].parse().unwrap()))
            })
            .collect();
        let ours: Vec<(u32, u32)> = m
            .extents
            .iter()
            .map(|e| (e.dba.split(scheme).1, e.blocks))
            .collect();
        // LV-043: a freelist (MSSM) header lists its first extent *after* the
        // header block itself (dba_extents 128x128 -> ktetb 129x127); ASSM
        // headers list the whole extent (the header sits inside it).
        let truth: Vec<(u32, u32)> = truth
            .into_iter()
            .enumerate()
            .map(|(i, (b, n))| {
                if i == 0 && expected_kind == MapKind::FreelistHeader {
                    (hb + 1, n - (hb - b) - 1)
                } else {
                    (b, n)
                }
            })
            .collect();
        assert_eq!(ours, truth, "{name}: extent map vs dba_extents");
        if scheme == RdbaScheme::Smallfile {
            for e in &m.extents {
                assert_eq!(e.dba.file(), file, "{name}: relative file in rdba");
            }
        }
        checked += 1;
    }
    assert!(checked >= 14, "checked {checked} segments");
}

/// Row readers over the real corpus: every table's row count and every
/// head-piece rowid must equal what Oracle reported (`meta/rowcounts.txt`,
/// `meta/rowids.psv`). This exercises heap (ASSM/MSSM/bigfile), chained,
/// migrated, ROWDEPENDENCIES, deleted-row skipping and the cluster reader.
#[test]
fn lab_row_readers_match_oracle_rowids() {
    use odr_core::dict::rows::{read_cluster, read_heap};
    use odr_core::dict::source::{BlockSource, FileInfo, ImageSet};
    use std::collections::{BTreeMap, BTreeSet};
    let Some(g) = golden() else { return };
    let mut src = ImageSet::new(ByteOrder::Little);
    for (file, rel, name, scheme) in [
        (16u32, Some(16u32), "odr_odr_assm01", RdbaScheme::Smallfile),
        (17, Some(17), "odr_odr_mssm01", RdbaScheme::Smallfile),
        (18, None, "odr_odr_big01", RdbaScheme::Bigfile),
    ] {
        src.add(
            FileInfo {
                file,
                relative: rel,
                block_size: BS,
                scheme,
                tablespace: None,
            },
            read_file(&g.join(format!("datafiles/{name}.dbf"))),
        );
    }
    // Oracle truth: table -> set of (rfile, block, slot) and counts.
    let mut truth: BTreeMap<String, BTreeSet<(u32, u32, u16)>> = BTreeMap::new();
    for l in std::fs::read_to_string(g.join("meta/rowids.psv"))
        .unwrap()
        .lines()
    {
        let p: Vec<&str> = l.split('|').collect();
        if p.len() == 6 {
            // DBMS_ROWID.ROWID_RELATIVE_FNO returns 0 for a bigfile
            // tablespace (no file bits in the rdba); ODR reports the bigfile
            // marker 1024 (`kcvfhrfn`), so normalise here.
            let rel: u32 = p[3].parse().unwrap();
            truth.entry(p[0].to_owned()).or_default().insert((
                if rel == 0 { 1024 } else { rel },
                p[4].parse().unwrap(),
                p[5].parse().unwrap(),
            ));
        }
    }
    let counts: BTreeMap<String, usize> = std::fs::read_to_string(g.join("meta/rowcounts.txt"))
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('|'))
        .map(|(t, n)| (t.to_owned(), n.trim().parse().unwrap()))
        .collect();
    let segs = std::fs::read_to_string(g.join("meta/segments.txt")).unwrap();
    // Cluster members: T_CL_* live in C_ODR. Find C_ODR's header + each table's tab#.
    // tab# is not in meta; derive it from the segment's kdbt order by trying 1..=3 and
    // matching rowid sets (there are only two member tables).
    let mut checked = 0;
    for line in segs.lines() {
        let p: Vec<&str> = line.split('|').collect();
        if p.len() < 7 {
            continue;
        }
        let (name, dataobj, kind, file, hb): (&str, u32, &str, u32, u32) = (
            p[0],
            p[2].parse().unwrap(),
            p[3],
            p[5].parse().unwrap(),
            p[6].parse().unwrap(),
        );
        let scheme = src.scheme(file).unwrap();
        let hdr = src
            .block(odr_core::dict::source::BlockAddr { file, block: hb })
            .unwrap();
        let seg = walk_segment(&hdr, ByteOrder::Little, Rdba(0), |r| {
            let a = src.resolve(r, file)?;
            src.block(a)
        })
        .unwrap();
        assert!(seg.issues.is_empty(), "{name}: {:?}", seg.issues);
        let _ = scheme;
        if kind == "CLUSTER" {
            // Member tables of C_ODR: T_CL_DEPT (tab 1) and T_CL_EMP (tab 2) in creation order.
            for (tab, tname, clucols) in [(1usize, "T_CL_DEPT", 2usize), (2, "T_CL_EMP", 2)] {
                let Some(want) = truth.get(tname) else {
                    continue;
                };
                let got = read_cluster(&src, &seg, file, dataobj, tab, clucols);
                let ok: BTreeSet<(u32, u32, u16)> = got
                    .rows
                    .iter()
                    .filter_map(|r| r.as_ref().ok())
                    .map(|r| {
                        (
                            Rdba(r.provenance.rdba).file(),
                            Rdba(r.provenance.rdba).block(),
                            r.rowid_row,
                        )
                    })
                    .collect();
                let errs: Vec<_> = got.rows.iter().filter_map(|r| r.as_ref().err()).collect();
                assert!(errs.is_empty(), "{tname}: {errs:?}");
                assert_eq!(ok.len(), counts[tname], "{tname}: row count");
                assert_eq!(&ok, want, "{tname}: rowid set");
                checked += 1;
            }
            continue;
        }
        let Some(want) = truth.get(name) else {
            continue;
        };
        let got = read_heap(&src, &seg, file, dataobj);
        let errs: Vec<_> = got.rows.iter().filter_map(|r| r.as_ref().err()).collect();
        assert!(errs.is_empty(), "{name}: {errs:?}");
        let ok: BTreeSet<(u32, u32, u16)> = got
            .rows
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|r| {
                let (f, b) = Rdba(r.provenance.rdba).split(scheme);
                (f.unwrap_or(1024), b, r.rowid_row)
            })
            .collect();
        assert_eq!(
            ok.len(),
            counts[name],
            "{name}: row count (deleted rows must be skipped)"
        );
        assert_eq!(&ok, want, "{name}: rowid set");
        if name == "T_CHAINED" || name == "T_MIGRATED" {
            let multi = got
                .rows
                .iter()
                .filter(|r| r.as_ref().is_ok_and(|r| r.pieces.len() > 1))
                .count();
            assert!(multi > 0, "{name}: expected multi-piece rows, got none");
        }
        assert!(
            got.skipped
                .iter()
                .all(|(_, s)| !matches!(s, odr_core::dict::rows::Skip::Malformed(_))),
            "{name}: {:?}",
            got.skipped
        );
        checked += 1;
    }
    assert!(checked >= 14, "checked {checked}");
}

/// LV-061 — *in a block that failed integrity no flag-based skip applies.*
///
/// Takes the real `T_JOURNAL` blocks 3227 and 3311 of the ASSM file (the two
/// that hid a row each under seed 7 torn damage: garbage flag bytes `0x15`
/// = `D L N` and `0x0B` = `F P N`), tears their second half the same way
/// `odr-corrupt` does, and checks the reader's accounting: every slot of
/// the row directory (`nrow`) is reported — as a row tagged with the block
/// status or as `RowError::Integrity` — and nothing is `Ok` with
/// `block_status == Valid`. The clean file is the control: same slots, all
/// `Ok`/`Valid`, zero errors.
#[test]
fn lab_torn_block_accounts_for_every_slot() {
    use odr_core::dict::rows::{RowError, read_heap};
    use odr_core::dict::segment::{Extent, SegmentBlocks};
    use odr_core::dict::source::{FileInfo, ImageSet};
    let Some(g) = golden() else { return };
    let clean = read_file(&g.join("datafiles/odr_odr_assm01.dbf"));
    let info = || FileInfo {
        file: 16,
        relative: Some(16),
        block_size: BS,
        scheme: RdbaScheme::Smallfile,
        tablespace: None,
    };
    // T_JOURNAL's data object id, from the block itself (ktbbh.objd).
    let objd = DataBlock::parse(block(&clean, 3227), ByteOrder::Little)
        .unwrap()
        .ktb
        .objd;
    let seg = |blocks: &[u32]| SegmentBlocks {
        objd,
        extents: blocks
            .iter()
            .map(|&b| Extent {
                dba: Rdba::smallfile(16, b).unwrap(),
                blocks: 1,
            })
            .collect(),
        issues: Vec::new(),
        hwm: None,
    };
    let targets = [3227u32, 3311];
    let nrow: usize = targets
        .iter()
        .map(|&b| {
            usize::from(
                DataBlock::parse(block(&clean, b), ByteOrder::Little)
                    .unwrap()
                    .kdbh
                    .nrow,
            )
        })
        .sum();
    assert_eq!(nrow, 142 + 137, "26ai lab: slots of blocks 3227 + 3311");

    // Control: clean blocks → every slot Ok and Valid.
    let mut src = ImageSet::new(ByteOrder::Little);
    src.add(info(), clean.clone());
    let got = read_heap(&src, &seg(&targets), 16, objd);
    assert_eq!(got.rows.len(), nrow);
    assert!(got.rows.iter().all(|r| {
        r.as_ref()
            .is_ok_and(|r| r.block_status == BlockStatus::Valid)
    }));

    // Torn: second half overwritten with deterministic junk (same recipe as
    // odr-corrupt's `Torn`, including the bytes that produced the gap).
    let mut torn = clean;
    let mut x = 7u64;
    for &b in &targets {
        for byte in &mut torn[b as usize * BS + BS / 2..(b as usize + 1) * BS] {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            *byte = (x >> 56) as u8;
        }
        // Make sure the flag bytes of the two historical slots are the
        // ones that fooled the reader before LV-061.
        let db = DataBlock::parse(block(&torn, b), ByteOrder::Little).unwrap();
        let slot = if b == 3227 { 85 } else { 119 };
        let off = match db.slot(slot).unwrap() {
            odr_core::row::Slot::Row(o) => o,
            odr_core::row::Slot::Unused => panic!("slot {slot} unused"),
        };
        torn[b as usize * BS + off] = if b == 3227 { 0x15 } else { 0x0B };
        assert_eq!(
            classify(block(&torn, b), BS, ByteOrder::Little),
            BlockStatus::TailMismatch
        );
    }
    let mut src = ImageSet::new(ByteOrder::Little);
    src.add(info(), torn);
    let got = read_heap(&src, &seg(&targets), 16, objd);
    assert_eq!(
        got.rows.len(),
        nrow,
        "every directory slot of a torn block is accounted for"
    );
    let mut ok_tagged = 0usize;
    let mut integrity = 0usize;
    for r in &got.rows {
        match r {
            Ok(r) => {
                assert_eq!(r.block_status, BlockStatus::TailMismatch);
                ok_tagged += 1;
            }
            Err((_, RowError::Integrity(st))) => {
                assert_eq!(*st, BlockStatus::TailMismatch);
                integrity += 1;
            }
            Err((p, e)) => panic!("slot {} must be Integrity, got {e:?}", p.slot),
        }
    }
    assert!(
        ok_tagged > 0 && integrity > 0,
        "ok {ok_tagged} integrity {integrity}"
    );
    // The two slots that used to vanish are now present as Integrity.
    for (b, slot) in [(3227u32, 85u16), (3311, 119)] {
        let rdba = Rdba::smallfile(16, b).unwrap().0;
        assert!(
            got.rows.iter().any(|r| matches!(
                r,
                Err((p, RowError::Integrity(_))) if p.rdba == rdba && p.slot == slot
            )),
            "block {b} slot {slot} missing"
        );
    }
    // LV-065: the damage frontier of each torn block is known, sector
    // aligned, never above the cut (BS/2), and every row whose bytes lie
    // wholly below it is byte-identical to the clean row.
    let mut src_clean = ImageSet::new(ByteOrder::Little);
    src_clean.add(info(), read_file(&g.join("datafiles/odr_odr_assm01.dbf")));
    let clean_rows = read_heap(&src_clean, &seg(&targets), 16, objd);
    let clean_by_key: std::collections::HashMap<(u32, u16), &[Option<Vec<u8>>]> = clean_rows
        .rows
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|r| ((r.provenance.rdba, r.provenance.slot), r.columns.as_slice()))
        .collect();
    let mut below = 0usize;
    for &b in &targets {
        let rdba = Rdba::smallfile(16, b).unwrap().0;
        let f = *got
            .torn_frontier
            .get(&rdba)
            .unwrap_or_else(|| panic!("block {b}: no damage frontier"));
        assert_eq!(f % 512, 0, "block {b}: frontier {f:#x} not sector aligned");
        assert!(f <= BS / 2, "block {b}: frontier {f:#x} above the cut");
        for r in got.rows.iter().filter_map(|r| r.as_ref().ok()) {
            if r.provenance.rdba == rdba && r.span.0 + r.span.1 <= f {
                assert_eq!(
                    Some(r.columns.as_slice()),
                    clean_by_key.get(&(rdba, r.provenance.slot)).copied(),
                    "block {b} slot {}: row below frontier differs from clean",
                    r.provenance.slot
                );
                below += 1;
            }
        }
    }
    assert!(below > 0, "no row below the frontier");

    // A cut inside the row directory leaves no frontier: every offset in
    // the directory is suspect, so nothing may be placed below anything.
    let mut early = read_file(&g.join("datafiles/odr_odr_assm01.dbf"));
    let b = 3227usize;
    let db = DataBlock::parse(block(&early, 3227), ByteOrder::Little).unwrap();
    let dir_mid = db.kdbh_off + 14 + 4 * usize::from(db.kdbh.ntab) + usize::from(db.kdbh.nrow);
    for byte in &mut early[b * BS + dir_mid..(b + 1) * BS] {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        *byte = (x >> 56) as u8;
    }
    assert_eq!(
        classify(block(&early, 3227), BS, ByteOrder::Little),
        BlockStatus::TailMismatch
    );
    let mut src = ImageSet::new(ByteOrder::Little);
    src.add(info(), early);
    let got = read_heap(&src, &seg(&[3227]), 16, objd);
    let rdba = Rdba::smallfile(16, 3227).unwrap().0;
    // Either no frontier, or every row below it is still the clean row of
    // the same slot — a garbage directory entry must never place a value.
    if let Some(&f) = got.torn_frontier.get(&rdba) {
        for r in got.rows.iter().filter_map(|r| r.as_ref().ok()) {
            if r.span.0 + r.span.1 <= f {
                assert_eq!(
                    Some(r.columns.as_slice()),
                    clean_by_key.get(&(rdba, r.provenance.slot)).copied(),
                    "dir cut: slot {} below frontier {f:#x} differs from clean",
                    r.provenance.slot
                );
            }
        }
    }
}

/// Step g (plan 2026-10-18 §5): **every stored value of every heap table**
/// must equal, byte for byte, what Oracle's own `DUMP(col, 1016)` printed
/// into `rows.psv` (141,613 truth values). Columns are matched by
/// `segment_column_id` from `meta/columns.txt`, rows by ROWID. Then the
/// canonical text of NUMBER / DATE / CHAR / VARCHAR2 / RAW values is compared
/// with Oracle's `TO_CHAR` output, and `T_ADDCOL` proves the fast-ADD-COLUMN
/// rule: an absent column becomes `Inferred(DEFAULT$)`, never a NULL.
#[test]
fn lab_values_match_oracle_dump_byte_for_byte() {
    use odr_core::dict::rows::read_heap;
    use odr_core::dict::schema::{Cell, ColumnDef, TableDef, materialize};
    use odr_core::dict::source::{BlockSource, FileInfo, ImageSet};
    use odr_core::value::{Charsets, ColumnType, decode_value};
    use std::collections::{BTreeMap, HashMap};
    let Some(g) = golden() else { return };
    let mut src = ImageSet::new(ByteOrder::Little);
    for (file, rel, name, scheme) in [
        (16u32, Some(16u32), "odr_odr_assm01", RdbaScheme::Smallfile),
        (17, Some(17), "odr_odr_mssm01", RdbaScheme::Smallfile),
        (18, None, "odr_odr_big01", RdbaScheme::Bigfile),
    ] {
        src.add(
            FileInfo {
                file,
                relative: rel,
                block_size: BS,
                scheme,
                tablespace: None,
            },
            read_file(&g.join(format!("datafiles/{name}.dbf"))),
        );
    }
    let cs = Charsets {
        database: Charset::Al32Utf8,
        national: Charset::Al16Utf16,
    };
    // columns.txt: TABLE|column_id|name|data_type|len|prec|scale|nullable|segcol|hidden|virtual
    let mut cols: BTreeMap<String, Vec<(u32, String, String)>> = BTreeMap::new(); // (segcol, name, type)
    for l in std::fs::read_to_string(g.join("meta/columns.txt"))
        .unwrap()
        .lines()
    {
        let p: Vec<&str> = l.split('|').collect();
        if p.len() == 11 && !p[8].is_empty() && p[10] == "NO" {
            cols.entry(p[0].to_owned()).or_default().push((
                p[8].parse().unwrap(),
                p[2].to_owned(),
                p[3].to_owned(),
            ));
        }
    }
    // rows.psv: TABLE|ID|COL|ROWID|Typ= Len=|CharacterSet|hex,bytes|canonical
    // truth[(table, rowid)][col] = (bytes or None, canonical text)
    let mut truth: HashMap<(String, String), HashMap<String, (Option<Vec<u8>>, String)>> =
        HashMap::new();
    for l in std::fs::read_to_string(g.join("rows.psv")).unwrap().lines() {
        let p: Vec<&str> = l.splitn(8, '|').collect();
        if p.len() != 8 {
            continue;
        }
        let bytes = if p[4] == "NULL" {
            None
        } else {
            Some(
                p[6].split(',')
                    .filter(|h| !h.is_empty())
                    .map(|h| u8::from_str_radix(h, 16).unwrap())
                    .collect::<Vec<u8>>(),
            )
        };
        truth
            .entry((p[0].to_owned(), p[3].to_owned()))
            .or_default()
            .insert(p[2].to_owned(), (bytes, p[7].to_owned()));
    }
    // ROWID -> (file, block, row) from rowids.psv (avoids re-implementing base64 rowids here).
    let mut rowid_of: HashMap<(String, u32, u32, u16), String> = HashMap::new();
    for l in std::fs::read_to_string(g.join("meta/rowids.psv"))
        .unwrap()
        .lines()
    {
        let p: Vec<&str> = l.split('|').collect();
        if p.len() == 6 {
            let rel: u32 = p[3].parse().unwrap();
            let key = (
                p[0].to_owned(),
                if rel == 0 { 1024 } else { rel },
                p[4].parse().unwrap(),
                p[5].parse().unwrap(),
            );
            rowid_of.insert(key, String::new()); // filled below from rows.psv rowid strings
        }
    }
    // rows.psv carries the textual ROWID; rowids.psv the decoded one. Join them
    // through the ID column (both files list ID per row).
    let mut id_to_rowid: HashMap<(String, String), String> = HashMap::new();
    for ((t, rid), colmap) in &truth {
        if let Some((_, id_text)) = colmap.get("ID") {
            id_to_rowid.insert((t.clone(), id_text.clone()), rid.clone());
        }
    }
    for l in std::fs::read_to_string(g.join("meta/rowids.psv"))
        .unwrap()
        .lines()
    {
        let p: Vec<&str> = l.split('|').collect();
        if p.len() == 6 {
            let rel: u32 = p[3].parse().unwrap();
            let key = (
                p[0].to_owned(),
                if rel == 0 { 1024 } else { rel },
                p[4].parse().unwrap(),
                p[5].parse().unwrap(),
            );
            if let Some(r) = id_to_rowid.get(&(p[0].to_owned(), p[1].to_owned())) {
                rowid_of.insert(key, r.clone());
            }
        }
    }

    let segs = std::fs::read_to_string(g.join("meta/segments.txt")).unwrap();
    let mut values_checked = 0usize;
    let mut canonical_checked = 0usize;
    let mut tables_checked = 0usize;
    for line in segs.lines() {
        let p: Vec<&str> = line.split('|').collect();
        if p.len() < 7 || p[3] != "TABLE" {
            continue;
        }
        let (name, dataobj, file, hb): (&str, u32, u32, u32) = (
            p[0],
            p[2].parse().unwrap(),
            p[5].parse().unwrap(),
            p[6].parse().unwrap(),
        );
        let Some(tcols) = cols.get(name) else {
            continue;
        };
        let mut tcols = tcols.clone();
        tcols.sort_by_key(|c| c.0);
        let scheme = src.scheme(file).unwrap();
        let hdr = src
            .block(odr_core::dict::source::BlockAddr { file, block: hb })
            .unwrap();
        let seg = walk_segment(&hdr, ByteOrder::Little, Rdba(0), |r| {
            let a = src.resolve(r, file)?;
            src.block(a)
        })
        .unwrap();
        let got = read_heap(&src, &seg, file, dataobj);
        let mut rows_seen = 0usize;
        for r in got.rows.iter().map(|r| r.as_ref().unwrap()) {
            let (f, b) = Rdba(r.provenance.rdba).split(scheme);
            let key = (name.to_owned(), f.unwrap_or(1024), b, r.rowid_row);
            let rid = rowid_of
                .get(&key)
                .unwrap_or_else(|| panic!("{name}: no rowid for {key:?}"));
            let want = truth.get(&(name.to_owned(), rid.clone())).unwrap();
            rows_seen += 1;
            for (i, (segcol, cname, dtype)) in tcols.iter().enumerate() {
                assert_eq!(
                    usize::try_from(*segcol).unwrap(),
                    i + 1,
                    "{name}: segcol gap"
                );
                let got_bytes: Option<&[u8]> = r.columns.get(i).and_then(|c| c.as_deref());
                let Some((want_bytes, want_text)) = want.get(cname) else {
                    // hidden SYS_NC columns are not in rows.psv
                    assert!(
                        cname.starts_with("SYS_NC"),
                        "{name}.{cname}: no truth value"
                    );
                    continue;
                };
                if name == "T_ADDCOL" && (cname == "NN" || cname == "NL") {
                    continue; // fast-added: checked through `materialize` below
                }
                assert_eq!(
                    got_bytes,
                    want_bytes.as_deref(),
                    "{name}.{cname} rowid {rid}: stored bytes differ"
                );
                values_checked += 1;
                // Canonical text where Oracle's TO_CHAR format is unambiguous.
                let ty = match dtype.as_str() {
                    "NUMBER" => ColumnType::Number,
                    "DATE" => ColumnType::Date,
                    "VARCHAR2" => ColumnType::Varchar2 { national: false },
                    "CHAR" => ColumnType::Char { national: false },
                    "NVARCHAR2" => ColumnType::Varchar2 { national: true },
                    "NCHAR" => ColumnType::Char { national: true },
                    "RAW" => ColumnType::Raw,
                    _ => continue,
                };
                let v = decode_value(got_bytes, ty, cs);
                let got_text = v.canonical().map(|c| match dtype.as_str() {
                    "CHAR" | "NCHAR" | "VARCHAR2" | "NVARCHAR2" => c.trim_end().to_owned(),
                    _ => c.into_owned(),
                });
                let want_norm = match dtype.as_str() {
                    // DUMP export prints DATE as " YYYY-MM-DD HH24:MI:SS" (leading space).
                    "DATE" => want_text.trim_start().to_owned(),
                    // Oracle TO_CHAR switches to scientific notation for extreme
                    // magnitudes (1E-130, 1E+125); ODR prints exact decimals. Bytes
                    // were already compared above.
                    "NUMBER" if want_text.contains('E') => continue,
                    // Oracle TO_CHAR drops the leading zero (".01", "-.5"); ODR keeps it.
                    "NUMBER" if want_text.starts_with('.') => format!("0{want_text}"),
                    "NUMBER" if want_text.starts_with("-.") => format!("-0{}", &want_text[1..]),
                    // Arabic/UNISTR text was exported as \XXXX escapes; compare only pure-ASCII.
                    "VARCHAR2" | "CHAR" | "NVARCHAR2" | "NCHAR" if want_text.contains('\\') => {
                        continue;
                    }
                    _ => want_text.clone(),
                };
                if got_bytes.is_none() {
                    assert_eq!(got_text, None, "{name}.{cname} rowid {rid}");
                } else {
                    assert_eq!(
                        got_text.as_deref(),
                        Some(want_norm.as_str()),
                        "{name}.{cname} rowid {rid}: canonical text"
                    );
                }
                canonical_checked += 1;
            }
            // Fast ADD COLUMN DEFAULT semantics (REQ-004.3 / LV-023) on real rows.
            if name == "T_ADDCOL" {
                let col = |c: u32,
                           segcol: u32,
                           intcol: u32,
                           n: &str,
                           t: u16,
                           prop: u64,
                           d: Option<&str>| ColumnDef {
                    col: c,
                    segcol,
                    intcol,
                    name: n.into(),
                    type_code: t,
                    length: 22,
                    precision: None,
                    scale: None,
                    not_null: n == "NN" || n == "ID",
                    charset_form: 1,
                    property: prop,
                    default_text: d.map(str::to_owned),
                };
                let table = TableDef {
                    obj: 0,
                    dataobj: Some(dataobj),
                    owner: 0,
                    owner_name: None,
                    name: name.into(),
                    ts: 0,
                    file,
                    block: hb,
                    bobj: None,
                    tab: None,
                    clucols: None,
                    flags: 0,
                    property: 0,
                    columns: vec![
                        col(1, 1, 1, "ID", 2, 0, None),
                        col(2, 2, 2, "V", 1, 0, None),
                        col(3, 3, 3, "NN", 2, 0x4000_0000, Some("7")),
                        col(0, 4, 4, "SYS_NC00004$", 23, 0x20, None),
                        col(4, 5, 5, "NL", 1, 0x4000_0000, Some("'dflt'")),
                    ],
                };
                let stored: Vec<Option<&[u8]>> = r.columns.iter().map(|c| c.as_deref()).collect();
                let cells = materialize(&table, &stored, cs);
                assert_eq!(cells.len(), 4, "{name}: visible cells");
                let id_text = want["ID"].1.clone();
                let nn = &cells[2];
                let nl = &cells[3];
                let (nn_want, nl_want) = (&want["NN"], &want["NL"]);
                // NN (NOT NULL DEFAULT 7): Oracle shows 7 for old rows, 8 for the new row.
                match nn {
                    Cell::Verified(v) => assert_eq!(
                        v.canonical().as_deref(),
                        Some(nn_want.1.as_str()),
                        "T_ADDCOL id {id_text} NN"
                    ),
                    Cell::Inferred { value, .. } => {
                        assert_eq!(
                            value.canonical().as_deref(),
                            Some(nn_want.1.as_str()),
                            "T_ADDCOL id {id_text} NN inferred"
                        );
                        assert!(nn_want.0.is_some(), "Oracle reports a value for NN");
                    }
                    Cell::Unknown { reason } => {
                        panic!("T_ADDCOL id {id_text} NN unknown: {reason}")
                    }
                    other => panic!("T_ADDCOL id {id_text} NN: unexpected {other:?}"),
                }
                // NL (nullable DEFAULT 'dflt'): old rows => Inferred('dflt') (Oracle prints dflt),
                // row 2 was updated to explicit NULL, row 6 inserted with explicit NULL.
                match (nl, nl_want.0.is_some()) {
                    (Cell::Verified(v), true) => assert_eq!(
                        v.canonical().as_deref(),
                        Some(nl_want.1.as_str()),
                        "T_ADDCOL id {id_text} NL"
                    ),
                    (Cell::Verified(v), false) => {
                        assert_eq!(v.canonical(), None, "T_ADDCOL id {id_text} NL must be NULL")
                    }
                    (Cell::Inferred { value, .. }, true) => assert_eq!(
                        value.canonical().as_deref(),
                        Some(nl_want.1.as_str()),
                        "T_ADDCOL id {id_text} NL inferred"
                    ),
                    (Cell::Inferred { value, .. }, false) => panic!(
                        "T_ADDCOL id {id_text}: NL inferred {value:?} but Oracle says NULL — golden rule violated"
                    ),
                    (Cell::Unknown { reason }, _) => {
                        panic!("T_ADDCOL id {id_text} NL unknown: {reason}")
                    }
                    (other, _) => panic!("T_ADDCOL id {id_text} NL: unexpected {other:?}"),
                }
            }
        }
        assert!(rows_seen > 0, "{name}: no rows");
        tables_checked += 1;
    }
    eprintln!(
        "golden values: {tables_checked} tables, {values_checked} byte-exact, {canonical_checked} canonical"
    );
    assert!(tables_checked >= 12, "tables {tables_checked}");
    assert!(values_checked >= 140_000, "values {values_checked}");
}

/// Step f: `Dictionary::load` on the real 26ai SYSTEM file. Every ODR table
/// must come out with the obj#/dataobj#, tablespace, segment header, column
/// names, types, nullability and segcol# that Oracle reports in
/// `meta/segments.txt` and `meta/columns.txt`; owner must resolve to `ODR`;
/// `T_ADDCOL`'s fast-added columns must carry `DEFAULT$` and the property bit.
#[test]
fn lab_dictionary_resolves_odr_tables_like_oracle() {
    use odr_core::dict::resolver::Dictionary;
    use odr_core::dict::source::{FileInfo, ImageSet};
    use std::collections::BTreeMap;
    let Some(g) = golden() else { return };
    let mut src = ImageSet::new(ByteOrder::Little);
    src.add(
        FileInfo {
            file: 12,
            relative: None,
            block_size: BS,
            scheme: RdbaScheme::Bigfile,
            tablespace: None,
        },
        read_file(&g.join("datafiles/FREE_FREEPDB1_system01.dbf")),
    );
    let dict = Dictionary::load(&src, 12).unwrap();
    eprintln!(
        "dictionary: {} objects, {} tabs, {} tables with cols, {} users, {} tablespaces, {} issues",
        dict.objects.len(),
        dict.tabs.len(),
        dict.cols.len(),
        dict.users.len(),
        dict.tablespaces.len(),
        dict.issues.len()
    );
    for i in dict.issues.iter().take(20) {
        eprintln!("  issue: {i:?}");
    }
    assert_eq!(dict.catalog.version.as_deref(), Some("8.0.0.0.0"));
    // LV-057 (migrated cluster members) + LV-058 (deleted-and-overwritten
    // row): the whole 26ai dictionary now reads with no issue at all.
    assert!(dict.issues.is_empty(), "{:?}", dict.issues);
    assert!(dict.objects.len() > 1000, "OBJ$ rows");
    assert!(dict.users.values().any(|u| u == "SYS"));
    assert!(dict.users.values().any(|u| u == "ODR"));
    // Oracle truth: datafiles.txt gives ts names; segments.txt the segments.
    let ts_names: Vec<String> = std::fs::read_to_string(g.join("meta/datafiles.txt"))
        .unwrap()
        .lines()
        .filter_map(|l| l.split('|').nth(2).map(str::to_owned))
        .collect();
    for n in &ts_names {
        assert!(
            dict.tablespaces.values().any(|t| t == n),
            "TS$ missing {n}: {:?}",
            dict.tablespaces
        );
    }
    let mut cols: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for l in std::fs::read_to_string(g.join("meta/columns.txt"))
        .unwrap()
        .lines()
    {
        let p: Vec<String> = l.split('|').map(str::to_owned).collect();
        if p.len() == 11 {
            cols.entry(p[0].clone()).or_default().push(p);
        }
    }
    let tables = dict.tables();
    let odr: Vec<_> = tables
        .iter()
        .filter(|t| t.owner_name.as_deref() == Some("ODR"))
        .collect();
    let mut checked = 0;
    for line in std::fs::read_to_string(g.join("meta/segments.txt"))
        .unwrap()
        .lines()
    {
        let p: Vec<&str> = line.split('|').collect();
        if p.len() < 7 || p[3] != "TABLE" {
            continue;
        }
        let (name, obj, dataobj, ts, file, hb): (&str, u32, u32, &str, u32, u32) = (
            p[0],
            p[1].parse().unwrap(),
            p[2].parse().unwrap(),
            p[4],
            p[5].parse().unwrap(),
            p[6].parse().unwrap(),
        );
        let t = odr.iter().find(|t| t.name == name).unwrap_or_else(|| {
            panic!(
                "{name} not resolved; have {:?}",
                odr.iter().map(|t| &t.name).collect::<Vec<_>>()
            )
        });
        assert_eq!(t.obj, obj, "{name} obj#");
        assert_eq!(t.dataobj, Some(dataobj), "{name} dataobj#");
        assert_eq!(dict.tablespace_name(t.ts), Some(ts), "{name} tablespace");
        // TAB$.FILE# is the relative file number (16/17 smallfile, 1024 bigfile).
        let rel = if ts == "ODR_BIG" { 1024 } else { file };
        assert_eq!((t.file, t.block), (rel, hb), "{name} segment header");
        assert!(!t.is_clustered(), "{name}");
        assert_eq!(
            t.has_row_dependencies(),
            name == "T_ROWDEP",
            "{name} rowdep flag"
        );
        // Columns vs dba_tab_cols.
        let want = &cols[name];
        let got = &t.columns;
        assert_eq!(got.len(), want.len(), "{name}: column count");
        for w in want {
            let c = got
                .iter()
                .find(|c| c.name == w[2])
                .unwrap_or_else(|| panic!("{name}.{}: missing", w[2]));
            let segcol: u32 = w[8].parse().unwrap_or(0);
            assert_eq!(c.segcol, segcol, "{name}.{} segcol#", w[2]);
            assert_eq!(c.not_null, w[7] == "N", "{name}.{} nullability", w[2]);
            assert_eq!(c.is_visible(), w[9] == "NO", "{name}.{} hidden", w[2]);
            let ty = c.column_type();
            let want_ty = match w[3].as_str() {
                "NUMBER" => "Number",
                "VARCHAR2" | "NVARCHAR2" => "Varchar2",
                "CHAR" | "NCHAR" => "Char",
                "RAW" => "Raw",
                "DATE" => "Date",
                "BINARY_FLOAT" => "BinaryFloat",
                "BINARY_DOUBLE" => "BinaryDouble",
                s if s.starts_with("TIMESTAMP") && s.ends_with("LOCAL TIME ZONE") => "TimestampLtz",
                s if s.starts_with("TIMESTAMP") && s.ends_with("TIME ZONE") => "TimestampTz",
                s if s.starts_with("TIMESTAMP") => "Timestamp",
                s if s.starts_with("INTERVAL YEAR") => "IntervalYm",
                s if s.starts_with("INTERVAL DAY") => "IntervalDs",
                other => panic!("unexpected type {other}"),
            };
            let got_ty = format!("{ty:?}");
            assert!(
                got_ty.starts_with(want_ty),
                "{name}.{}: type {got_ty} vs {}",
                w[2],
                w[3]
            );
            if w[3] == "NVARCHAR2" || w[3] == "NCHAR" {
                assert!(
                    got_ty.contains("national: true"),
                    "{name}.{} national",
                    w[2]
                );
            }
        }
        if name == "T_ADDCOL" {
            let nn = got.iter().find(|c| c.name == "NN").unwrap();
            let nl = got.iter().find(|c| c.name == "NL").unwrap();
            assert!(
                nn.is_fast_add_default() && nl.is_fast_add_default(),
                "fast-add property bit"
            );
            assert_eq!(nn.default_text.as_deref().map(str::trim), Some("7"));
            assert_eq!(nl.default_text.as_deref().map(str::trim), Some("'dflt'"));
            assert!(nn.not_null && !nl.not_null);
        }
        checked += 1;
    }
    assert_eq!(checked, 13, "all heap tables resolved");
    // Cluster members: TAB$ must say clustered with bobj# = C_ODR and tab# 1/2;
    // CLU$ must give C_ODR's segment (segments.txt: C_ODR|72539|72539|CLUSTER|ODR_ASSM|16|2179).
    let c_odr = dict.objects.values().find(|o| o.name == "C_ODR").unwrap();
    let clu = &dict.clusters[&c_odr.obj];
    assert_eq!(
        (clu.dataobj, clu.file, clu.block, clu.cols, clu.hashkeys),
        (Some(72539), 16, 2179, 2, 0)
    );
    assert_eq!(dict.tablespace_name(clu.ts), Some("ODR_ASSM"));
    for (n, tab) in [("T_CL_DEPT", 1u32), ("T_CL_EMP", 2)] {
        let t = odr.iter().find(|t| t.name == n).unwrap();
        assert!(t.is_clustered(), "{n}");
        assert_eq!(t.bobj, Some(c_odr.obj), "{n} bobj#");
        assert_eq!(t.tab, Some(tab), "{n} tab#");
        assert_eq!(t.clucols, Some(2), "{n} clucols");
    }
    // PROPS$ (LV-056): NLS values equal meta/database.txt; NULL VALUE$ rows
    // (TDE_MASTER_KEY_ID …) are values, not issues.
    let (props, prop_issues) = dict.properties(&src, 12);
    assert!(prop_issues.is_empty(), "{prop_issues:?}");
    let db_txt = std::fs::read_to_string(g.join("meta/database.txt")).unwrap();
    for key in ["NLS_CHARACTERSET", "NLS_NCHAR_CHARACTERSET"] {
        let want = db_txt
            .lines()
            .find_map(|l| l.strip_prefix(&format!("nls|{key}|")))
            .unwrap();
        assert_eq!(props[key].as_deref(), Some(want), "{key}");
    }
    assert!(props.contains_key("TDE_MASTER_KEY_ID") && props["TDE_MASTER_KEY_ID"].is_none());
    assert!(props.len() >= 20, "{}", props.len());
    // The dictionary itself is self-consistent: OBJ$ has as many stored
    // columns in COL$ as bootstrap$ declared.
    for n in ["OBJ$", "TAB$", "COL$", "USER$", "TS$"] {
        let boot = dict.catalog.by_name(n).unwrap();
        let cols = &dict.cols[&boot.obj];
        assert_eq!(
            cols.len(),
            boot.columns.len(),
            "{n}: COL$ vs bootstrap$ column count"
        );
        for (a, b) in cols.iter().zip(&boot.columns) {
            assert_eq!(a.name, b.name, "{n}: COL$ order vs bootstrap$");
        }
    }
}

/// End-to-end without any lab metadata: SYSTEM file → `Dictionary` →
/// `TableDef` → segment walk → `read_heap` → `materialize` → compare every
/// visible cell with Oracle's canonical text in `rows.psv` (joined on the
/// `ID` column). This is the path the CLI will take on a customer's files.
#[test]
fn lab_end_to_end_dictionary_to_cells() {
    use odr_core::dict::resolver::Dictionary;
    use odr_core::dict::rows::read_heap;
    use odr_core::dict::schema::{Cell, materialize};
    use odr_core::dict::source::{BlockAddr, BlockSource, FileInfo, ImageSet};
    use odr_core::value::Charsets;
    use std::collections::HashMap;
    let Some(g) = golden() else { return };
    let mut src = ImageSet::new(ByteOrder::Little);
    for (file, rel, name, scheme) in [
        (12u32, None, "FREE_FREEPDB1_system01", RdbaScheme::Bigfile),
        (16, Some(16u32), "odr_odr_assm01", RdbaScheme::Smallfile),
        (17, Some(17), "odr_odr_mssm01", RdbaScheme::Smallfile),
        (18, None, "odr_odr_big01", RdbaScheme::Bigfile),
    ] {
        src.add(
            FileInfo {
                file,
                relative: rel,
                block_size: BS,
                scheme,
                tablespace: None,
            },
            read_file(&g.join(format!("datafiles/{name}.dbf"))),
        );
    }
    let cs = Charsets {
        database: Charset::Al32Utf8,
        national: Charset::Al16Utf16,
    };
    // truth[(table, id)][col] = canonical text (None = NULL)
    let mut truth: HashMap<(String, String), HashMap<String, Option<String>>> = HashMap::new();
    for l in std::fs::read_to_string(g.join("rows.psv")).unwrap().lines() {
        let p: Vec<&str> = l.splitn(8, '|').collect();
        if p.len() == 8 {
            truth
                .entry((p[0].to_owned(), p[1].to_owned()))
                .or_default()
                .insert(p[2].to_owned(), (p[4] != "NULL").then(|| p[7].to_owned()));
        }
    }
    let dict = Dictionary::load(&src, 12).unwrap();
    // ODR_BIG is bigfile: TAB$.FILE# = 1024 must map to absolute file 18.
    // ODR_ASSM/ODR_MSSM: relative 16/17 = absolute 16/17 in this lab.
    let abs_file = |t: &odr_core::dict::schema::TableDef| -> u32 {
        match dict.tablespace_name(t.ts) {
            Some("ODR_BIG") => 18,
            _ => t.file,
        }
    };
    let mut cells_checked = 0usize;
    let mut tables = 0usize;
    for t in dict.tables() {
        if t.owner_name.as_deref() != Some("ODR") || t.is_clustered() {
            continue;
        }
        let file = abs_file(&t);
        let hdr = src
            .block(BlockAddr {
                file,
                block: t.block,
            })
            .unwrap_or_else(|| panic!("{}: header {file}/{}", t.name, t.block));
        let seg = walk_segment(&hdr, ByteOrder::Little, Rdba(0), |r| {
            src.resolve(r, file).and_then(|a| src.block(a))
        })
        .unwrap();
        assert_eq!(Some(seg.objd), t.dataobj, "{}: objd", t.name);
        let rows = read_heap(&src, &seg, file, seg.objd);
        let visible: Vec<_> = t.visible_columns();
        let id_pos = visible.iter().position(|c| c.name == "ID").unwrap();
        let mut n = 0;
        for r in rows.rows.iter().map(|r| r.as_ref().unwrap()) {
            let stored: Vec<Option<&[u8]>> = r.columns.iter().map(|c| c.as_deref()).collect();
            let cells = materialize(&t, &stored, cs);
            assert_eq!(cells.len(), visible.len(), "{}: cell count", t.name);
            let id = cells[id_pos].canonical().unwrap();
            let want = truth
                .get(&(t.name.clone(), id.clone()))
                .unwrap_or_else(|| panic!("{} id {id}: not in rows.psv", t.name));
            for (c, cell) in visible.iter().zip(&cells) {
                let Some(w) = want.get(&c.name) else {
                    panic!("{}.{}: no truth", t.name, c.name)
                };
                let got = cell.canonical();
                let ty = format!("{:?}", c.column_type());
                // Compare where Oracle's TO_CHAR format is unambiguous (see
                // lab_values_match_oracle_dump_byte_for_byte for the rules).
                let comparable = ty.starts_with("Number")
                    || ty.starts_with("Date")
                    || ty.starts_with("Raw")
                    || ty.starts_with("Varchar2")
                    || ty.starts_with("Char");
                if !comparable {
                    continue;
                }
                match (w, got) {
                    (None, None) => {}
                    (None, Some(g)) => panic!(
                        "{}.{} id {id}: Oracle NULL, ODR {g:?} ({cell:?}) — golden rule violated",
                        t.name, c.name
                    ),
                    (Some(w), None) => {
                        panic!("{}.{} id {id}: Oracle {w:?}, ODR NULL", t.name, c.name)
                    }
                    (Some(w), Some(g)) => {
                        let mut w = w.trim_end().to_owned();
                        if ty.starts_with("Date") {
                            w = w.trim_start().to_owned();
                        }
                        if ty.starts_with("Number") {
                            if w.contains('E') {
                                continue;
                            }
                            if let Some(rest) = w.strip_prefix("-.") {
                                w = format!("-0.{rest}");
                            } else if w.starts_with('.') {
                                w = format!("0{w}");
                            }
                        }
                        if w.contains('\\') {
                            continue; // UNISTR-escaped export
                        }
                        assert_eq!(g.trim_end(), w, "{}.{} id {id} ({cell:?})", t.name, c.name);
                    }
                }
                if matches!(cell, Cell::Inferred { .. }) {
                    assert_eq!(
                        t.name, "T_ADDCOL",
                        "only fast-added columns may be inferred"
                    );
                }
                cells_checked += 1;
            }
            n += 1;
        }
        assert!(n > 0, "{}: no rows", t.name);
        tables += 1;
    }
    eprintln!("end-to-end: {tables} tables, {cells_checked} cells compared");
    assert_eq!(tables, 13);
    assert!(cells_checked > 100_000);
}

/// LV-063 — *a second Oracle version, read with the same code.*
///
/// Runs against `ODR_GOLDEN2` (a golden set from another release; first
/// produced on **Oracle 21c XE**, `gvenzl/oracle-xe:21-slim`, run
/// `20261021T093052Z`). Nothing is hard-coded: file numbers, rfile numbers
/// and schemes come from each file's own `kcvfh`, the SYSTEM file is found
/// by its bootstrap, and every heap/cluster row of every `ODR.*` table is
/// compared with Oracle's ROWID set and `COUNT(*)`; `T_JOURNAL` values are
/// compared with `rows.psv`. The dictionary must load with **0 issues** —
/// on 21c the first run reported 3 blocks of a dropped object inside a
/// reused `C_TS#` extent *above its high-water mark*; `walk_segment` now
/// reads the HWM (`ktech` @48/52/56/60/76, matched against 11 header dumps
/// on 26ai + 21c) and the reader skips such blocks.
#[test]
fn lab2_second_version_reads_like_oracle() {
    use odr_core::dict::resolver::Dictionary;
    use odr_core::dict::rows::{read_cluster, read_heap};
    use odr_core::dict::schema::materialize;
    use odr_core::dict::source::{BlockAddr, BlockSource, FileInfo, ImageSet};
    use odr_core::value::Charsets;
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    let Some(g) = std::env::var_os("ODR_GOLDEN2").map(PathBuf::from) else {
        return;
    };
    let mut src = ImageSet::new(ByteOrder::Little);
    let mut system_file = None;
    let mut infos: Vec<FileInfo> = Vec::new();
    for e in std::fs::read_dir(g.join("datafiles")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "dbf") {
            continue;
        }
        let img = read_file(&p);
        let h = FileHeader::detect(block(&img, 1)).unwrap();
        let info = FileInfo::from_header(&h);
        if p.file_name().unwrap().to_string_lossy().contains("system") {
            system_file = Some(info.file);
        }
        infos.push(info);
        src.add(info, img);
    }
    let sysf = system_file.unwrap();
    let dict = Dictionary::load(&src, sysf).unwrap();
    assert!(dict.issues.is_empty(), "{:?}", dict.issues);
    let counts: BTreeMap<String, usize> = std::fs::read_to_string(g.join("meta/rowcounts.txt"))
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('|'))
        .map(|(t, n)| (t.to_owned(), n.trim().parse().unwrap()))
        .collect();
    let mut truth: BTreeMap<String, BTreeSet<(u32, u32, u16)>> = BTreeMap::new();
    for l in std::fs::read_to_string(g.join("meta/rowids.psv"))
        .unwrap()
        .lines()
    {
        let p: Vec<&str> = l.split('|').collect();
        if p.len() == 6 {
            let rel: u32 = p[3].parse().unwrap();
            truth.entry(p[0].to_owned()).or_default().insert((
                if rel == 0 { 1024 } else { rel },
                p[4].parse().unwrap(),
                p[5].parse().unwrap(),
            ));
        }
    }
    // T_JOURNAL canonical truth: (id) -> col -> text
    let mut jt: HashMap<String, HashMap<String, Option<String>>> = HashMap::new();
    for l in std::fs::read_to_string(g.join("rows.psv")).unwrap().lines() {
        let p: Vec<&str> = l.splitn(8, '|').collect();
        if p.len() == 8 && p[0] == "T_JOURNAL" {
            jt.entry(p[1].to_owned())
                .or_default()
                .insert(p[2].to_owned(), (p[4] != "NULL").then(|| p[7].to_owned()));
        }
    }
    let find_file = |ts: u32, rfile: u32| -> u32 {
        infos
            .iter()
            .find(|i| {
                i.tablespace == Some(ts)
                    && (i.relative == Some(rfile) || (i.relative.is_none() && rfile == 1024))
            })
            .map_or_else(|| panic!("no file for ts {ts} rfile {rfile}"), |i| i.file)
    };
    let cs = Charsets {
        database: Charset::Al32Utf8,
        national: Charset::Al16Utf16,
    };
    let mut checked = 0;
    let mut journal_cells = 0usize;
    for t in dict.tables() {
        if t.owner_name.as_deref() != Some("ODR") {
            continue;
        }
        let (file, hb, cluster, objd) = if t.is_clustered() {
            let clu = dict.cluster_of(&t).unwrap();
            let tab = usize::try_from(t.tab.unwrap()).unwrap();
            let clucols = usize::try_from(t.clucols.unwrap_or(clu.cols)).unwrap();
            (
                find_file(clu.ts, clu.file),
                clu.block,
                Some((tab, clucols)),
                clu.dataobj,
            )
        } else {
            (find_file(t.ts, t.file), t.block, None, t.dataobj)
        };
        let scheme = src.scheme(file).unwrap();
        let hdr = src.block(BlockAddr { file, block: hb }).unwrap();
        let seg = walk_segment(&hdr, ByteOrder::Little, Rdba(0), |r| {
            src.resolve(r, file).and_then(|a| src.block(a))
        })
        .unwrap();
        assert!(seg.issues.is_empty(), "{}: {:?}", t.name, seg.issues);
        assert_eq!(Some(seg.objd), objd, "{}: objd", t.name);
        let rows = match cluster {
            None => read_heap(&src, &seg, file, seg.objd),
            Some((tab, clucols)) => read_cluster(&src, &seg, file, seg.objd, tab, clucols),
        };
        let errs: Vec<_> = rows.rows.iter().filter_map(|r| r.as_ref().err()).collect();
        assert!(errs.is_empty(), "{}: {errs:?}", t.name);
        let got: BTreeSet<(u32, u32, u16)> = rows
            .rows
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|r| {
                let (f, b) = Rdba(r.provenance.rdba).split(scheme);
                (f.unwrap_or(1024), b, r.rowid_row)
            })
            .collect();
        assert_eq!(got.len(), counts[&t.name], "{}: COUNT(*)", t.name);
        assert_eq!(&got, &truth[&t.name], "{}: rowid set", t.name);
        assert!(
            rows.rows.iter().all(|r| r
                .as_ref()
                .is_ok_and(|r| r.block_status == BlockStatus::Valid && r.structural.is_none())),
            "{}: every row Valid and structurally clean",
            t.name
        );
        if t.name == "T_JOURNAL" {
            let visible = t.visible_columns();
            let id_pos = visible.iter().position(|c| c.name == "ID").unwrap();
            for r in rows.rows.iter().map(|r| r.as_ref().unwrap()) {
                let stored: Vec<Option<&[u8]>> = r.columns.iter().map(|c| c.as_deref()).collect();
                let cells = materialize(&t, &stored, cs);
                let id = cells[id_pos].canonical().unwrap();
                let want = &jt[&id];
                for (c, cell) in visible.iter().zip(&cells) {
                    let w = want[&c.name].as_deref().map(|w| w.trim().to_owned());
                    if w.as_deref().is_some_and(|w| w.contains('\\')) {
                        continue; // UNISTR-escaped Arabic in the export (step g)
                    }
                    assert_eq!(
                        cell.canonical().map(|s| s.trim_end().to_owned()),
                        w,
                        "T_JOURNAL.{} id {id}",
                        c.name
                    );
                    journal_cells += 1;
                }
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 15, "ODR tables");
    assert!(
        journal_cells >= 100_000,
        "T_JOURNAL cells compared: {journal_cells}"
    );
}
