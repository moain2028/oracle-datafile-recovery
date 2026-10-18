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
    clippy::assert_is_empty
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
