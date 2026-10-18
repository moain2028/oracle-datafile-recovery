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
