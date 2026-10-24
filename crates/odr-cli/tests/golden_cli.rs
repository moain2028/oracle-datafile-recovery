//! CLI against the real 26ai golden set (skipped unless `ODR_GOLDEN` is set;
//! see `docs/runbooks/agent-environment.md`). Row counts and the three
//! output layers are compared with Oracle's own `rowcounts.txt` / `rows.psv`.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::arithmetic_side_effects
)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

/// Golden v2 (2026-10-24 lab run: + `T_LOB_BASIC`, `T_LOB_SECURE`,
/// `T_COMPRESS`). The damage tests keep `ODR_GOLDEN` (v1) because their
/// seeds, block numbers and recall floors are measurements of those bytes.
fn golden_v2() -> Option<PathBuf> {
    std::env::var_os("ODR_GOLDEN_V2").map(PathBuf::from)
}

fn golden() -> Option<PathBuf> {
    std::env::var_os("ODR_GOLDEN").map(PathBuf::from)
}

fn files(g: &std::path::Path) -> Vec<PathBuf> {
    [
        "FREE_FREEPDB1_system01",
        "odr_odr_assm01",
        "odr_odr_mssm01",
        "odr_odr_big01",
    ]
    .iter()
    .map(|n| g.join(format!("datafiles/{n}.dbf")))
    .collect()
}

#[test]
fn dict_json_lists_odr_tables() {
    let Some(g) = golden() else { return };
    let out = Command::new(env!("CARGO_BIN_EXE_odr"))
        .arg("dict")
        .args(files(&g))
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["bootstrap_version"], "8.0.0.0.0");
    let sys: Vec<_> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["system"] == true)
        .collect();
    assert_eq!(sys.len(), 1);
    assert_eq!(sys[0]["file"], 12);
    let odr: Vec<&str> = v["tables"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["owner"] == "ODR")
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(odr.len() >= 15, "{odr:?}");
    assert!(odr.contains(&"T_JOURNAL") && odr.contains(&"T_CL_EMP"));
}

#[test]
fn extract_every_heap_table_matches_oracle_counts_and_layers() {
    let Some(g) = golden() else { return };
    let counts: HashMap<String, usize> = std::fs::read_to_string(g.join("meta/rowcounts.txt"))
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once('|'))
        .map(|(t, n)| (t.to_owned(), n.trim().parse().unwrap()))
        .collect();
    let tmp = std::env::temp_dir().join(format!("odr_cli_golden_{}", std::process::id()));
    for (table, want_inferred) in [
        ("T_JOURNAL", 0usize),
        ("T_ADDCOL", 5),
        ("T_ROWDEP", 0),
        ("T_BIG", 0),
        ("T_CHAINED", 0),
        ("T_MIGRATED", 0),
        ("T_DELETED", 0),
        ("T_NULLS", 0),
        ("T_TEXT", 0),
        ("T_DATETIME", 0),
        ("T_NUMBER", 0),
        ("T_BINARY", 0),
        ("T_MSSM", 0),
        ("T_CL_DEPT", 0),
        ("T_CL_EMP", 0),
    ] {
        let want_rejected = 0usize;
        let out_dir = tmp.join(table);
        let out = Command::new(env!("CARGO_BIN_EXE_odr"))
            .arg("extract")
            .args(files(&g))
            .args(["--table", &format!("ODR.{table}"), "--out"])
            .arg(&out_dir)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(if want_rejected > 0 { 2 } else { 0 }),
            "{table}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out_dir.join("report.json")).unwrap())
                .unwrap();
        let rows = &report["rows"];
        let (v, i, r) = (
            rows["verified"].as_u64().unwrap() as usize,
            rows["inferred"].as_u64().unwrap() as usize,
            rows["rejected"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            v + i + r,
            counts[table],
            "{table}: verified+inferred+rejected == Oracle COUNT(*)"
        );
        assert_eq!(i, want_inferred, "{table}: inferred rows");
        assert_eq!(r, want_rejected, "{table}: rejected rows");

        // CSV line counts agree with the report (header + rows).
        let lines = |f: &str| {
            std::fs::read_to_string(out_dir.join(f))
                .unwrap()
                .lines()
                .count()
                - 1
        };
        assert_eq!(lines("verified.csv"), v, "{table}");
        assert_eq!(lines("inferred.csv"), i, "{table}");
        assert_eq!(lines("rejected.csv"), r, "{table}");
        // Every input's SHA-256 is recorded (chain of custody).
        assert_eq!(report["inputs"].as_array().unwrap().len(), 4);
        for inp in report["inputs"].as_array().unwrap() {
            assert_eq!(inp["sha256"].as_str().unwrap().len(), 64);
        }
    }
    // T_ADDCOL semantics, straight from the CSVs: old rows inferred with
    // NN=7 and NL=dflt (except id 2: explicit NULL), new row verified.
    let inf = std::fs::read_to_string(tmp.join("T_ADDCOL/inferred.csv")).unwrap();
    let rows: Vec<Vec<&str>> = inf
        .lines()
        .skip(1)
        .map(|l| l.split(',').collect())
        .collect();
    for r in &rows {
        let (id, nn, nl) = (r[3], r[5], r[6]);
        assert_eq!(nn, "7", "id {id}");
        assert_eq!(nl, if id == "2" { "" } else { "dflt" }, "id {id}");
    }
    let ver = std::fs::read_to_string(tmp.join("T_ADDCOL/verified.csv")).unwrap();
    let addcol6 = std::fs::read_to_string(g.join("meta/rowids.psv"))
        .unwrap()
        .lines()
        .find(|l| l.starts_with("T_ADDCOL|6|"))
        .map(|l| {
            let p: Vec<&str> = l.split('|').collect();
            format!("16,{},{},6,new,8,", p[4], p[5])
        })
        .unwrap();
    assert!(
        ver.lines().nth(1).unwrap().starts_with(&addcol6),
        "T_ADDCOL new row: {ver}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A damage recipe applied to a copy of the ASSM file (LV-070 test).
type Damage<'a> = Box<dyn Fn(&mut Vec<u8>) + 'a>;

/// LV-068 / LV-070 — inline LOBs are read from the row; out-of-line LOBs
/// are assembled from the LOB segment through the locator's chunk map
/// (every chunk block: integrity + type + LOB id + exact length), so all
/// 6 rows of `T_LOB_BASIC` (`BasicFile`) and `T_LOB_SECURE` (`SecureFile`)
/// are verified. Then three damaged copies prove the refusal path: a bit
/// flip inside a chunk ⇒ `ChecksumFail`, a zeroed chunk ⇒ `Zeroed`, a
/// chunk overwritten by another LOB's block (valid checksum) ⇒
/// `ForeignLob` — the row is rejected with the reason, the other 5 rows
/// stay verified, nothing is printed as a partial LOB. Golden v2 only.
#[test]
fn extract_assembles_out_of_line_lobs_and_refuses_damaged_chunks() {
    let Some(g) = golden_v2() else { return };
    let tmp = std::env::temp_dir().join(format!("odr_cli_lob_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let assm = g.join("datafiles/odr_odr_assm01.dbf");
    let sys = g.join("datafiles/FREE_FREEPDB1_system01.dbf");
    let run = |table: &str, assm: &std::path::Path, out_dir: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_odr"))
            .arg("extract")
            .arg(&sys)
            .arg(assm)
            .args(["--table", &format!("ODR.{table}"), "--out"])
            .arg(out_dir)
            .output()
            .unwrap()
    };
    // Clean: 6/6 verified, out-of-line 6000-char CLOB and 6000-byte BLOB included.
    let mut chunk_blocks: Vec<(String, u32)> = Vec::new();
    for table in ["T_LOB_BASIC", "T_LOB_SECURE"] {
        let out_dir = tmp.join(table);
        let out = run(table, &assm, &out_dir);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{table}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let ver = std::fs::read_to_string(out_dir.join("verified.csv")).unwrap();
        assert_eq!(ver.lines().count(), 7, "{table}: 6 verified rows\n{ver}");
        assert!(
            ver.contains(",1,inline ascii clob,DEADBEEF00FF,inline both"),
            "{table}: {ver}"
        );
        assert!(
            ver.contains(",2,فاتورة رقم 1 - مرفق,,"),
            "{table}: Arabic CLOB: {ver}"
        );
        assert!(
            ver.contains(",3,,,empty lobs") && ver.contains(",4,,,null lobs"),
            "{table}: {ver}"
        );
        let near = ver.lines().find(|l| l.contains(",5,")).unwrap();
        assert_eq!(near.matches('x').count(), 1900, "{table}: inline CLOB");
        assert_eq!(near.matches("79").count(), 1900, "{table}: inline BLOB hex");
        let big = ver.lines().find(|l| l.contains(",6,")).unwrap();
        let cols: Vec<&str> = big.split(',').collect();
        assert_eq!(cols[4].len(), 6000, "{table}: out-of-line CLOB chars");
        assert!(cols[4].bytes().all(|b| b == b'L'), "{table}: CLOB content");
        assert_eq!(cols[5].len(), 12_000, "{table}: out-of-line BLOB hex");
        assert!(
            cols[5].as_bytes().chunks(2).all(|c| c == b"4D"),
            "{table}: BLOB content"
        );
        // Chunk blocks named by the locator (from the clean block bytes).
        let img = std::fs::read(&assm).unwrap();
        let locs = lob_locators_of_row6(&img, &g, table);
        for (col, loc) in locs {
            if let odr_core::types::LobData::OutOfLine { chunks, .. } = loc.data {
                chunk_blocks.push((
                    format!("{table}:{col}"),
                    odr_core::block::Rdba(chunks[0].0).block(),
                ));
            }
        }
    }
    assert_eq!(chunk_blocks.len(), 4, "{chunk_blocks:?}");
    // Damage the chunk blocks three ways; each must reject row 6 only.
    let clean = std::fs::read(&assm).unwrap();
    let bs = 8192usize;
    let find = |key: &str| chunk_blocks.iter().find(|(k, _)| k == key).unwrap().1 as usize;
    let cases: [(&str, &str, Damage<'_>, &str); 3] = [
        (
            "bitflip",
            "T_LOB_BASIC",
            Box::new(|d: &mut Vec<u8>| {
                let b = find("T_LOB_BASIC:C");
                d[b * bs + 0x100] ^= 1;
            }),
            "ChecksumFail",
        ),
        (
            "zeroed",
            "T_LOB_SECURE",
            Box::new(|d: &mut Vec<u8>| {
                let b = find("T_LOB_SECURE:B");
                d[b * bs..(b + 1) * bs].fill(0);
            }),
            "Zeroed",
        ),
        (
            "foreign",
            "T_LOB_BASIC",
            Box::new(|d: &mut Vec<u8>| {
                let (b, o) = (find("T_LOB_BASIC:B"), find("T_LOB_BASIC:C"));
                let src: Vec<u8> = d[o * bs..(o + 1) * bs].to_vec();
                d[b * bs..(b + 1) * bs].copy_from_slice(&src);
            }),
            "ForeignLob",
        ),
    ];
    for (name, table, damage, want) in cases {
        let mut d = clean.clone();
        damage(&mut d);
        let dmg = tmp.join(format!("{name}.dbf"));
        std::fs::write(&dmg, &d).unwrap();
        let out_dir = tmp.join(format!("out_{name}"));
        let out = run(table, &dmg, &out_dir);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let ver = std::fs::read_to_string(out_dir.join("verified.csv")).unwrap();
        assert_eq!(
            ver.lines().count(),
            6,
            "{name}: 5 rows stay verified\n{ver}"
        );
        assert!(
            !ver.lines().any(|l| l.contains(",6,")),
            "{name}: row 6 must not be verified"
        );
        let rej = std::fs::read_to_string(out_dir.join("rejected.csv")).unwrap();
        assert!(
            rej.contains("LOB out of line could not be assembled")
                && rej.contains(want)
                && rej.contains("LV-070"),
            "{name}: {rej}"
        );
        let _ = std::fs::remove_file(&dmg);
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Locators of row ID=6 of a LOB table, read straight from the block the
/// ROWID in `meta/rowids.psv` points at: `(column name, locator)`.
fn lob_locators_of_row6<'a>(
    img: &'a [u8],
    g: &std::path::Path,
    table: &str,
) -> Vec<(&'static str, odr_core::types::LobLocator<'a>)> {
    let line = std::fs::read_to_string(g.join("meta/rowids.psv"))
        .unwrap()
        .lines()
        .find(|l| l.starts_with(&format!("{table}|6|")))
        .unwrap()
        .to_owned();
    let p: Vec<&str> = line.split('|').collect();
    let (block, slot): (usize, u16) = (p[4].parse().unwrap(), p[5].parse().unwrap());
    let db = odr_core::row::DataBlock::parse(
        &img[block * 8192..(block + 1) * 8192],
        odr_core::ByteOrder::Little,
    )
    .unwrap();
    let row = db.row(slot).unwrap().unwrap();
    let mut out = Vec::new();
    for (name, idx) in [("C", 1usize), ("B", 2)] {
        if let Some(Some(raw)) = row.columns.get(idx) {
            out.push((name, odr_core::types::LobLocator::parse(raw).unwrap()));
        }
    }
    out
}

/// LV-071 — a BASIC-compressed table is decoded through its 9ir2 symbol
/// table: 2,000/2,000 rows verified, canonical text identical to Oracle's
/// `rows.psv`, exit 0, `compressed_blocks` reported.
#[test]
fn extract_decodes_basic_compressed_table_exactly() {
    let Some(golden) = golden_v2() else { return };
    let out_dir = std::env::temp_dir().join(format!("odr_cli_compress_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out_dir);
    let out = Command::new(env!("CARGO_BIN_EXE_odr"))
        .arg("extract")
        .arg(golden.join("datafiles/FREE_FREEPDB1_system01.dbf"))
        .arg(golden.join("datafiles/odr_odr_assm01.dbf"))
        .args(["--table", "ODR.T_COMPRESS", "--out"])
        .arg(&out_dir)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let verified = std::fs::read_to_string(out_dir.join("verified.csv")).unwrap();
    let lines: Vec<&str> = verified.lines().collect();
    assert_eq!(lines.len(), 2_001, "2,000 rows + header");
    // Truth: rows.psv canonical column (field 8) per (ID, column).
    let mut truth = std::collections::HashMap::new();
    for line in std::fs::read_to_string(golden.join("rows.psv"))
        .unwrap()
        .lines()
    {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.first() == Some(&"T_COMPRESS") {
            truth.insert(
                (parts[1].to_owned(), parts[2].to_owned()),
                parts[7].to_owned(),
            );
        }
    }
    for row in &lines[1..] {
        let fields: Vec<&str> = row.split(',').collect();
        let (id, val, num) = (fields[3], fields[4], fields[5]);
        assert_eq!(truth[&(id.to_owned(), "ID".to_owned())], id);
        assert_eq!(truth[&(id.to_owned(), "V".to_owned())], val, "row {id}");
        assert_eq!(truth[&(id.to_owned(), "N".to_owned())], num, "row {id}");
    }
    let report = std::fs::read_to_string(out_dir.join("report.json")).unwrap();
    assert!(report.contains("\"compressed_blocks\": 3"), "{report}");
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn extract_refuses_unknown_table_and_missing_system() {
    let Some(g) = golden() else { return };
    let out = Command::new(env!("CARGO_BIN_EXE_odr"))
        .arg("extract")
        .args(files(&g))
        .args(["--table", "ODR.NOPE", "--out"])
        .arg(std::env::temp_dir().join("odr_cli_nope"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));
    // Without SYSTEM there is no dictionary: refuse, never guess a schema.
    let out = Command::new(env!("CARGO_BIN_EXE_odr"))
        .arg("dict")
        .arg(g.join("datafiles/odr_odr_assm01.dbf"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no SYSTEM datafile"));
}

/// Extract `ODR.T_JOURNAL` from `[system, assm]`, return the output dir.
fn extract_journal(
    g: &std::path::Path,
    assm: &std::path::Path,
    out_dir: &std::path::Path,
    extra: &[String],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_odr"))
        .arg("extract")
        .arg(g.join("datafiles/FREE_FREEPDB1_system01.dbf"))
        .arg(assm)
        .args(["--table", "ODR.T_JOURNAL", "--out"])
        .arg(out_dir)
        .args(extra)
        .output()
        .unwrap()
}

/// Data lines of a CSV (header dropped), as a set.
fn csv_rows(p: &std::path::Path) -> std::collections::HashSet<String> {
    std::fs::read_to_string(p)
        .unwrap()
        .lines()
        .skip(1)
        .map(str::to_owned)
        .collect()
}

/// Number of clean rows whose block (CSV column `_block`) is entirely zero
/// in `damaged` — rows that no reader can enumerate, hence the only rows a
/// damaged extraction may fail to account for (LV-061).
fn truth_rows_in_zeroed_blocks(
    truth: &std::collections::HashSet<String>,
    damaged: &[u8],
    bs: usize,
) -> usize {
    truth
        .iter()
        .filter(|l| {
            let b: usize = l.split(',').nth(1).unwrap().parse().unwrap();
            damaged[b * bs..(b + 1) * bs].iter().all(|&x| x == 0)
        })
        .count()
}

/// LV-059 — *precision of the verified layer is 1.0 under damage.*
///
/// Deterministic `odr-corrupt` copies of the ASSM file (bit rot, zeroed
/// blocks, torn writes, scattered holes). Every line that ODR calls
/// *verified* must be byte-identical to the clean extraction; damaged blocks
/// may only lower recall (rows go to `rejected.csv` with the integrity
/// reason) — never produce a wrong value with a green label.
#[test]
fn damaged_copies_never_yield_a_wrong_verified_cell() {
    use odr_corrupt::{Op, Spec, apply};
    let Some(g) = golden() else { return };
    let tmp = std::env::temp_dir().join(format!("odr_cli_damage_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let clean_path = g.join("datafiles/odr_odr_assm01.dbf");
    let clean_bytes = std::fs::read(&clean_path).unwrap();
    let clean_out = tmp.join("clean");
    assert!(
        extract_journal(&g, &clean_path, &clean_out, &[])
            .status
            .success()
    );
    let truth = csv_rows(&clean_out.join("verified.csv"));
    assert_eq!(truth.len(), 20000);

    let cases: [(&str, Op, f64); 4] = [
        ("zero_200", Op::Zero { count: 200 }, 0.95),
        ("bitrot_400", Op::Bitrot { count: 400 }, 0.93),
        ("holes_20_16", Op::Holes { count: 20, len: 16 }, 0.99),
        ("torn_200", Op::Torn { count: 200 }, 0.98),
    ];
    for (name, op, min_recall) in cases {
        let spec = Spec {
            block_size: 8192,
            seed: 7,
            ops: vec![op],
            blocks: None,
        };
        let (bytes, manifest) = apply(&clean_bytes, &spec).unwrap();
        assert!(!manifest.changes.is_empty(), "{name}: nothing damaged");
        let dmg = tmp.join(format!("{name}.dbf"));
        std::fs::write(&dmg, &bytes).unwrap();
        let out_dir = tmp.join(name);
        let out = extract_journal(&g, &dmg, &out_dir, &[]);
        // Exit 2 (rejected rows present) or 0 (holes may miss every row) —
        // never a hard failure.
        assert!(
            matches!(out.status.code(), Some(0 | 2)),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let verified = csv_rows(&out_dir.join("verified.csv"));
        let wrong: Vec<&String> = verified.iter().filter(|l| !truth.contains(*l)).collect();
        assert!(wrong.is_empty(), "{name}: wrong verified rows {wrong:?}");
        // LV-065: only a torn block may yield *inferred* rows (those wholly
        // below the damage frontier); every one of them must equal the
        // clean row, and the `_inference` column must name the rule.
        let inferred_text = std::fs::read_to_string(out_dir.join("inferred.csv")).unwrap();
        let inferred: Vec<&str> = csv_records(&inferred_text).into_iter().skip(1).collect();
        if name == "torn_200" {
            assert!(
                !inferred.is_empty(),
                "{name}: torn partial recovery produced nothing"
            );
        } else {
            assert!(
                inferred.is_empty(),
                "{name}: unexpected inferred rows {inferred:?}"
            );
        }
        for rec in &inferred {
            let (row, why) = rec.rsplit_once(',').unwrap();
            assert!(
                why.contains("torn block") && why.contains("LV-065"),
                "{name}: {why}"
            );
            assert!(truth.contains(row), "{name}: wrong inferred row {row}");
        }
        // Recall floor (measured 2026-10-18; see lab-verify LV-059).
        #[allow(clippy::cast_precision_loss)]
        let recall = verified.len() as f64 / truth.len() as f64;
        assert!(recall >= min_recall, "{name}: recall {recall}");
        // Every rejected row names its reason: the block failed integrity,
        // or the row itself did not decode (a flipped length byte). Rows
        // may contain quoted newlines, so count via the report, not lines.
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out_dir.join("report.json")).unwrap())
                .unwrap();
        let rejected = report["rows"]["rejected"].as_u64().unwrap() as usize;
        // LV-061 — exact accounting: every one of the 20,000 rows is either
        // verified, rejected with a reason, or inside a block that was
        // zeroed whole (no row directory left to enumerate; the block is
        // listed in `skipped_blocks` under `integrity: Zeroed`). Nothing
        // is dropped silently: in a torn block a garbage flag byte that
        // looks like `D`/continuation (seed 7: 0x15, 0x0B) is rejected.
        let zeroed_rows = truth_rows_in_zeroed_blocks(&truth, &bytes, 8192);
        assert_eq!(
            verified.len() + inferred.len() + rejected + zeroed_rows,
            truth.len(),
            "{name}: verified {} + inferred {} + rejected {rejected} + unreadable(zeroed) {zeroed_rows}",
            verified.len(),
            inferred.len()
        );
        if name == "zero_200" {
            assert!(zeroed_rows > 0, "{name}: expected zeroed data blocks");
            let skipped = report["skipped_blocks"].as_object().unwrap();
            assert!(skipped.contains_key("integrity: Zeroed"), "{skipped:?}");
        } else {
            assert_eq!(zeroed_rows, 0, "{name}");
        }
        let rej = std::fs::read_to_string(out_dir.join("rejected.csv")).unwrap();
        assert_eq!(
            rej.matches("block integrity: ").count() + rej.matches("row did not decode: ").count(),
            rejected,
            "{name}: every rejected row carries a reason"
        );
        // Input hash in the report is the damaged file, not the original.
        let inputs = report["inputs"].as_array().unwrap();
        assert!(
            inputs
                .iter()
                .any(|i| i["sha256"] == manifest.output_sha256.as_str()),
            "{name}: damaged input hash not recorded"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Split CSV text into records, honouring quoted fields that contain
/// newlines (rejected rows may carry raw text). Returns the raw record text.
fn csv_records(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted) = (0usize, false);
    for (i, ch) in text.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '\n' if !quoted => {
                out.push(text[start..i].trim_end_matches('\r'));
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// LV-062 — *a valid checksum is not proof of content.*
///
/// Damage aimed at `T_JOURNAL`'s first extent (blocks 3204..=3327 of the
/// ASSM file) so every pattern hits the table:
/// * **swap** (misdirected writes): every swapped block is rejected as
///   `block structure: header says block …` — precision 1.0, recall drops;
/// * **forge** (byte changed, `chkval` recomputed): every block whose
///   row directory / lengths / lock bytes were hit is rejected as
///   `block structure`; a forged byte *inside a column value* is detected
///   only when the value stops decoding (bad UTF-8, NUMBER/DATE out of
///   range) — the residue is measured and bounded here, never hidden;
/// * **ransom** (`--assume`, encrypted header + every 10th 8 KiB chunk):
///   encrypted blocks are `integrity: Foreign`, the rest inferred; recall
///   ≥ 0.85, no wrong value.
///
/// Accounting is exact for all three: verified + inferred + rejected ==
/// 20,000 (ransom: minus rows of encrypted blocks).
#[test]
fn forged_swapped_and_encrypted_copies_are_rejected_not_believed() {
    use odr_corrupt::{Op, Spec, apply};
    let Some(g) = golden() else { return };
    let tmp = std::env::temp_dir().join(format!("odr_cli_forge_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let clean_path = g.join("datafiles/odr_odr_assm01.dbf");
    let clean_bytes = std::fs::read(&clean_path).unwrap();
    let clean_out = tmp.join("clean");
    assert!(
        extract_journal(&g, &clean_path, &clean_out, &[])
            .status
            .success()
    );
    let truth = csv_rows(&clean_out.join("verified.csv"));
    assert_eq!(truth.len(), 20000);
    let report_of = |dir: &std::path::Path| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap()
    };
    let rejected_blocks = |dir: &std::path::Path| -> std::collections::HashSet<u64> {
        csv_records(&std::fs::read_to_string(dir.join("rejected.csv")).unwrap())
            .into_iter()
            .skip(1)
            .map(|r| r.split(',').nth(1).unwrap().parse().unwrap())
            .collect()
    };

    // --- swap: 20 pairs inside the extent, 3 seeds.
    for seed in [7u64, 8, 9] {
        let spec = Spec {
            block_size: 8192,
            seed,
            ops: vec![Op::Swap { count: 20 }],
            blocks: Some((3204, 3327)),
        };
        let (bytes, manifest) = apply(&clean_bytes, &spec).unwrap();
        let dmg = tmp.join(format!("swap_{seed}.dbf"));
        std::fs::write(&dmg, &bytes).unwrap();
        let out_dir = tmp.join(format!("swap_{seed}"));
        let out = extract_journal(&g, &dmg, &out_dir, &[]);
        assert_eq!(out.status.code(), Some(2), "swap {seed}");
        let verified = csv_rows(&out_dir.join("verified.csv"));
        assert!(
            verified.iter().all(|l| truth.contains(l)),
            "swap {seed}: wrong verified"
        );
        let rep = report_of(&out_dir);
        let rejected = rep["rows"]["rejected"].as_u64().unwrap() as usize;
        assert_eq!(verified.len() + rejected, 20000, "swap {seed}: accounting");
        let hit: std::collections::HashSet<u64> =
            manifest.changes.iter().map(|c| c.block).collect();
        let rej = rejected_blocks(&out_dir);
        let missed: Vec<_> = hit.iter().filter(|b| !rej.contains(b)).collect();
        assert!(
            missed.is_empty(),
            "swap {seed}: swapped blocks not rejected {missed:?}"
        );
        let txt = std::fs::read_to_string(out_dir.join("rejected.csv")).unwrap();
        assert!(
            txt.contains("block structure: header says block"),
            "swap {seed}"
        );
    }

    // --- forge: 60 blocks inside the extent, 3 seeds.
    let (mut forged_blocks, mut detected_blocks, mut wrong_rows) = (0usize, 0usize, 0usize);
    for seed in [7u64, 8, 9] {
        let spec = Spec {
            block_size: 8192,
            seed,
            ops: vec![Op::Forge { count: 60 }],
            blocks: Some((3204, 3327)),
        };
        let (bytes, manifest) = apply(&clean_bytes, &spec).unwrap();
        let dmg = tmp.join(format!("forge_{seed}.dbf"));
        std::fs::write(&dmg, &bytes).unwrap();
        let out_dir = tmp.join(format!("forge_{seed}"));
        let out = extract_journal(&g, &dmg, &out_dir, &[]);
        assert!(matches!(out.status.code(), Some(0 | 2)), "forge {seed}");
        let verified = csv_rows(&out_dir.join("verified.csv"));
        let rep = report_of(&out_dir);
        let rejected = rep["rows"]["rejected"].as_u64().unwrap() as usize;
        assert_eq!(verified.len() + rejected, 20000, "forge {seed}: accounting");
        let hit: std::collections::HashSet<u64> = manifest
            .changes
            .iter()
            .filter(|c| c.what == "forge_value")
            .map(|c| c.block)
            .collect();
        let rej = rejected_blocks(&out_dir);
        forged_blocks += hit.len();
        detected_blocks += hit.iter().filter(|b| rej.contains(b)).count();
        let wrong = verified.iter().filter(|l| !truth.contains(*l)).count();
        wrong_rows += wrong;
        // A wrong verified row may only come from a forged block whose
        // forged byte sits inside a column value (checked by block id).
        for l in verified.iter().filter(|l| !truth.contains(*l)) {
            let b: u64 = l.split(',').nth(1).unwrap().parse().unwrap();
            assert!(
                hit.contains(&b),
                "forge {seed}: wrong row from an untouched block {l}"
            );
        }
        // Untouched blocks are never rejected (no false positives).
        let fp: Vec<_> = rej.iter().filter(|b| !hit.contains(b)).collect();
        assert!(fp.is_empty(), "forge {seed}: false positives {fp:?}");
    }
    // Measured 2026-10-21 (26ai, seeds 7/8/9, 144 forged blocks): 39 blocks
    // rejected structurally, 110 wrong rows from value-interior forgeries
    // out of 54,000 verified rows (0.2 %). The bound below is the honest
    // ceiling of what single-block structural checks can do; lowering it
    // needs cross-block evidence (indexes / undo), tracked as M6.
    assert!(
        detected_blocks * 100 >= forged_blocks * 20,
        "forge detection {detected_blocks}/{forged_blocks}"
    );
    assert!(
        wrong_rows <= 150,
        "forge: {wrong_rows} wrong verified rows in 3 seeds"
    );

    // --- ransom with --assume.
    let spec = Spec {
        block_size: 8192,
        seed: 7,
        ops: vec![Op::Ransom {
            lead_bytes: 16384,
            chunk: 8192,
            every: 10,
        }],
        blocks: None,
    };
    let (bytes, manifest) = apply(&clean_bytes, &spec).unwrap();
    let dmg = tmp.join("ransom.dbf");
    std::fs::write(&dmg, &bytes).unwrap();
    let out_dir = tmp.join("ransom");
    let assume = format!("{}=file:16,ts:7,rfile:16,bs:8192", dmg.display());
    let out = extract_journal(&g, &dmg, &out_dir, &["--assume".to_owned(), assume]);
    assert!(
        matches!(out.status.code(), Some(0 | 2)),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(csv_rows(&out_dir.join("verified.csv")).is_empty());
    let inferred = csv_rows(&out_dir.join("inferred.csv"));
    let tag = ",file identity from --assume (header destroyed)";
    for l in &inferred {
        let body = l.strip_suffix(tag).unwrap();
        assert!(truth.contains(body), "ransom: wrong inferred value {body}");
    }
    let rep = report_of(&out_dir);
    let rejected = rep["rows"]["rejected"].as_u64().unwrap() as usize;
    // Rows whose block the manifest says was encrypted: unreadable by any
    // tool. An encrypted block whose random format nibble happens to be 2
    // classifies TailMismatch rather than Foreign — still an integrity skip.
    let encrypted: std::collections::HashSet<u64> =
        manifest.changes.iter().map(|c| c.block).collect();
    let encrypted_rows = truth
        .iter()
        .filter(|l| encrypted.contains(&l.split(',').nth(1).unwrap().parse::<u64>().unwrap()))
        .count();
    assert_eq!(
        inferred.len() + rejected + encrypted_rows,
        20000,
        "ransom: accounting"
    );
    #[allow(clippy::cast_precision_loss)]
    let recall = inferred.len() as f64 / 20000.0;
    assert!(recall >= 0.85, "ransom recall {recall}");
    let skipped = rep["skipped_blocks"].as_object().unwrap();
    assert!(skipped.contains_key("integrity: Foreign"), "{skipped:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// LV-064 — *a block with one undecodable row is tainted; its siblings are
/// not verified.*
///
/// Oracle's `chkval` is a 16-bit XOR of the block (verified on 26ai and 11g
/// blocks): two bit flips in the same bit position of the same 16-bit lane
/// cancel, so 1.8 % of double-hit bitrot blocks still pass the checksum
/// (10 seeds × 2 versions). When such a block has a row whose value does
/// not decode exactly, every other row of that block must be rejected with
/// `block tainted`, never verified. Here: seed 5 on the 26ai ASSM file,
/// block 3307 (two flips, chkval unchanged, column 4 of one row no longer
/// decodes) — before LV-064 its 136 siblings were verified; one of them
/// carried a wrong value.
#[test]
fn tainted_block_siblings_are_not_verified() {
    use odr_corrupt::{Op, Spec, apply};
    let Some(g) = golden() else { return };
    let tmp = std::env::temp_dir().join(format!("odr_cli_taint_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let clean_path = g.join("datafiles/odr_odr_assm01.dbf");
    let clean_bytes = std::fs::read(&clean_path).unwrap();
    let clean_out = tmp.join("clean");
    assert!(
        extract_journal(&g, &clean_path, &clean_out, &[])
            .status
            .success()
    );
    let truth = csv_rows(&clean_out.join("verified.csv"));
    let spec = Spec {
        block_size: 8192,
        seed: 5,
        ops: vec![Op::Bitrot { count: 400 }],
        blocks: Some((3204, 3327)),
    };
    let (bytes, manifest) = apply(&clean_bytes, &spec).unwrap();
    // Blocks hit twice or more whose checksum still folds to zero.
    let mut hits: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for c in &manifest.changes {
        *hits.entry(c.block).or_default() += 1;
    }
    let colliding: Vec<u64> = hits
        .iter()
        .filter(|(b, n)| {
            **n >= 2
                && odr_core::block::xor_fold(&bytes[**b as usize * 8192..(**b as usize + 1) * 8192])
                    == 0
        })
        .map(|(b, _)| *b)
        .collect();
    assert!(
        !colliding.is_empty(),
        "seed 5 must produce at least one checksum collision"
    );
    let dmg = tmp.join("bitrot.dbf");
    std::fs::write(&dmg, &bytes).unwrap();
    let out_dir = tmp.join("bitrot");
    let out = extract_journal(&g, &dmg, &out_dir, &[]);
    assert_eq!(out.status.code(), Some(2));
    let verified = csv_rows(&out_dir.join("verified.csv"));
    let rej = std::fs::read_to_string(out_dir.join("rejected.csv")).unwrap();
    // Any colliding block that has a 'not exact' row must have *no* verified row.
    let mut exercised = 0usize;
    for b in &colliding {
        let has_inexact = csv_records(&rej)
            .iter()
            .any(|r| r.split(',').nth(1) == Some(&b.to_string()) && r.contains("not exact"));
        if has_inexact {
            exercised += 1;
            let v_here = verified
                .iter()
                .filter(|l| l.split(',').nth(1) == Some(&b.to_string()))
                .count();
            assert_eq!(
                v_here, 0,
                "block {b}: tainted block still has verified rows"
            );
            assert!(
                rej.contains("block tainted"),
                "block {b}: siblings must say why"
            );
        }
    }
    assert!(
        exercised >= 1,
        "seed 5 must contain at least one tainted block (had {} colliding)",
        colliding.len()
    );
    // Measured ceiling (2026-10-21, seed 5): the wrong verified rows left
    // (1, in block 3241) are only in colliding blocks where *no* row became
    // inexact — the honest residue of a 16-bit checksum. Bounded here;
    // never silently accepted.
    let wrong: Vec<&String> = verified.iter().filter(|l| !truth.contains(*l)).collect();
    for l in &wrong {
        let b: u64 = l.split(',').nth(1).unwrap().parse().unwrap();
        assert!(
            colliding.contains(&b),
            "wrong verified row outside a checksum-colliding block: {l}"
        );
    }
    assert!(wrong.len() <= 1, "wrong verified rows {}", wrong.len());
    let _ = std::fs::remove_dir_all(&tmp);
}

/// LV-060 — *destroyed header ⇒ no identity ⇒ refuse; `--assume` ⇒ inferred.*
///
/// With blocks 0..=1 wiped the file cannot prove file#/tablespace. ODR must
/// refuse rather than guess. When the operator supplies the identity, every
/// row is at best *inferred* and labelled with the reason, and the values
/// are the clean ones (the data blocks are intact).
#[test]
fn wiped_header_requires_assume_and_downgrades_to_inferred() {
    use odr_corrupt::{Op, Spec, apply};
    let Some(g) = golden() else { return };
    let tmp = std::env::temp_dir().join(format!("odr_cli_wipe_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let clean_path = g.join("datafiles/odr_odr_assm01.dbf");
    let clean_out = tmp.join("clean");
    assert!(
        extract_journal(&g, &clean_path, &clean_out, &[])
            .status
            .success()
    );
    let truth = csv_rows(&clean_out.join("verified.csv"));

    let spec = Spec {
        block_size: 8192,
        seed: 7,
        ops: vec![Op::WipeHeader],
        blocks: None,
    };
    let (bytes, _) = apply(&std::fs::read(&clean_path).unwrap(), &spec).unwrap();
    let dmg = tmp.join("wipe-header.dbf");
    std::fs::write(&dmg, &bytes).unwrap();

    // 1. Without --assume: refuse.
    let out = extract_journal(&g, &dmg, &tmp.join("refused"), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not an Oracle datafile header"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 2. Wrong identity (file 17 is ODR_MSSM): the dictionary points
    //    elsewhere, so the table's blocks are not in this file — no rows,
    //    never rows from the wrong file called T_JOURNAL.
    let wrong = format!("{}=file:17,ts:8,rfile:17,bs:8192", dmg.display());
    let out = extract_journal(
        &g,
        &dmg,
        &tmp.join("wrong"),
        &["--assume".to_owned(), wrong],
    );
    assert_ne!(
        out.status.code(),
        Some(0),
        "wrong identity must not succeed"
    );

    // 3. Correct identity: all 20000 rows, all inferred, values identical.
    let right = format!("{}=file:16,ts:7,rfile:16,bs:8192", dmg.display());
    let out_dir = tmp.join("assumed");
    let out = extract_journal(&g, &dmg, &out_dir, &["--assume".to_owned(), right]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(csv_rows(&out_dir.join("verified.csv")).is_empty());
    let inferred = csv_rows(&out_dir.join("inferred.csv"));
    assert_eq!(inferred.len(), 20000);
    let tag = ",file identity from --assume (header destroyed)";
    for l in &inferred {
        let Some(body) = l.strip_suffix(tag) else {
            panic!("missing inference label: {l}");
        };
        assert!(
            truth.contains(body),
            "inferred value differs from clean: {body}"
        );
    }
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("report.json")).unwrap())
            .unwrap();
    let assumed: Vec<_> = report["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["assumed"] == true)
        .collect();
    assert_eq!(assumed.len(), 1, "report must flag the assumed input");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// M4 — *dictionary-less mode never verifies and never guesses silently.*
///
/// `odr scavenge` on the ASSM file **without** the SYSTEM file: `T_JOURNAL`
/// comes out as `OBJ_<dataobj>` with `COL_1..COL_7`, every row in
/// `inferred.csv` (0 verified), the per-column type rule named in
/// `_inference`, and every cell text identical to what the dictionary path
/// produced — the types were inferred, the values were not.
#[test]
fn scavenge_without_system_infers_journal_exactly_and_never_verifies() {
    let Some(g) = golden() else { return };
    let tmp = std::env::temp_dir().join(format!("odr_cli_scavenge_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let assm = g.join("datafiles/odr_odr_assm01.dbf");
    let truth_dir = tmp.join("truth");
    assert!(extract_journal(&g, &assm, &truth_dir, &[]).status.success());
    // Dictionary path: `_file,_block,_slot,ID,...` → key by (block, slot).
    let truth_text = std::fs::read_to_string(truth_dir.join("verified.csv")).unwrap();
    let truth: std::collections::HashMap<(String, String), String> = csv_records(&truth_text)
        .into_iter()
        .skip(1)
        .map(|r| {
            let mut it = r.splitn(4, ',');
            let _file = it.next().unwrap();
            let b = it.next().unwrap().to_owned();
            let s = it.next().unwrap().to_owned();
            ((b, s), it.next().unwrap().to_owned())
        })
        .collect();
    assert_eq!(truth.len(), 20000);
    let out_dir = tmp.join("scv");
    let out = Command::new(env!("CARGO_BIN_EXE_odr"))
        .arg("scavenge")
        .arg(&assm)
        .args(["--out"])
        .arg(&out_dir)
        .output()
        .unwrap();
    assert!(
        matches!(out.status.code(), Some(0 | 2)),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("report.json")).unwrap())
            .unwrap();
    let objects = report["objects"].as_array().unwrap();
    // No object has a verified row: no dictionary ⇒ nothing is verified.
    for o in objects {
        assert_eq!(o["rows"]["verified"], 0, "{o}");
    }
    let journal = objects
        .iter()
        .find(|o| o["rows"]["inferred"] == 20000)
        .unwrap_or_else(|| panic!("an object with 20,000 inferred rows (T_JOURNAL): {objects:?}"));
    let types: Vec<&str> = journal["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["inferred_type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        ["NUMBER", "DATE", "TEXT", "TEXT", "NUMBER", "NUMBER", "TEXT"],
        "ID, DOC_DATE, ACCOUNT_NO, DESCRIPTION, DEBIT, CREDIT, CURRENCY"
    );
    assert!(
        !out_dir
            .join(journal["object"].as_str().unwrap())
            .join("verified.csv")
            .exists()
    );
    let inf = std::fs::read_to_string(
        out_dir
            .join(journal["object"].as_str().unwrap())
            .join("inferred.csv"),
    )
    .unwrap();
    let recs = csv_records(&inf);
    assert_eq!(recs.len(), 20001);
    assert_eq!(
        recs[0],
        "_file,_block,_slot,COL_1,COL_2,COL_3,COL_4,COL_5,COL_6,COL_7,_inference"
    );
    for r in &recs[1..] {
        let (row, why) = r.rsplit_once(',').unwrap();
        // `_inference` is quoted (it contains commas? no — spaces); it must
        // name the mode and every column's rule.
        assert!(
            why.contains("dictionary-less") && why.contains("COL_2=DATE"),
            "{why}"
        );
        let mut it = row.splitn(4, ',');
        let _file = it.next().unwrap();
        let key = (it.next().unwrap().to_owned(), it.next().unwrap().to_owned());
        let cells = it.next().unwrap();
        assert_eq!(
            Some(cells),
            truth.get(&key).map(String::as_str),
            "block {} slot {}: scavenged cells differ from the dictionary path",
            key.0,
            key.1
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// LV-072 — type candidates of `odr scavenge` measured against the
/// dictionary. Protocol: the candidate set was tuned on 26ai only; this
/// test runs the **same** comparison on every golden set it is given
/// (`ODR_GOLDEN` = 26ai tuning set, `ODR_GOLDEN2` = 11g/21c hold-out) and
/// requires **zero wrongly named columns**. Abstentions (`RAW(...)`) are
/// allowed and counted; TIMESTAMP(0) named DATE is not wrong (7-byte
/// values are byte-identical and print identically).
#[test]
fn scavenge_type_candidates_never_name_a_wrong_type() {
    let sets: Vec<PathBuf> = [golden(), std::env::var_os("ODR_GOLDEN2").map(PathBuf::from)]
        .into_iter()
        .flatten()
        .collect();
    if sets.is_empty() {
        return;
    }
    for g in sets {
        let dfs: Vec<PathBuf> = std::fs::read_dir(g.join("datafiles"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let system = dfs
            .iter()
            .find(|p| p.to_string_lossy().contains("system"))
            .unwrap()
            .clone();
        let data: Vec<&PathBuf> = dfs
            .iter()
            .filter(|p| p.to_string_lossy().contains("odr_odr"))
            .collect();
        let dict = Command::new(env!("CARGO_BIN_EXE_odr"))
            .arg("dict")
            .arg("--json")
            .arg(&system)
            .args(&data)
            .output()
            .unwrap();
        let dict: serde_json::Value = serde_json::from_slice(&dict.stdout).unwrap();
        let tmp = std::env::temp_dir().join(format!(
            "odr_cli_scv_types_{}_{}",
            std::process::id(),
            g.file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let out = Command::new(env!("CARGO_BIN_EXE_odr"))
            .arg("scavenge")
            .args(&data)
            .arg("--out")
            .arg(&tmp)
            .output()
            .unwrap();
        assert!(matches!(out.status.code(), Some(0 | 2)));
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.join("report.json")).unwrap())
                .unwrap();
        let (mut named, mut abstained, mut wrong) = (0usize, 0usize, Vec::new());
        let mut seen = std::collections::BTreeSet::new();
        for obj in report["objects"].as_array().unwrap() {
            let dataobj = obj["data_object_id"].as_u64().unwrap();
            let Some(table) = dict["tables"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["owner"] == "ODR" && t["dataobj"].as_u64() == Some(dataobj))
            else {
                continue;
            };
            let cols: Vec<&serde_json::Value> = table["columns"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["hidden"] != true)
                .collect();
            for (i, c) in obj["columns"].as_array().unwrap().iter().enumerate() {
                let Some(truth) = cols.get(i) else { break };
                let truth = truth["type"].as_str().unwrap_or("?").to_owned();
                let inferred = c["inferred_type"].as_str().unwrap();
                if inferred.starts_with("RAW(") {
                    abstained += 1;
                    continue;
                }
                named += 1;
                seen.insert(inferred.to_owned());
                let ok = match inferred {
                    "NUMBER" => truth == "Number",
                    "DATE" => truth == "Date" || truth == "Timestamp",
                    "TIMESTAMP" => truth == "Timestamp" || truth == "TimestampLtz",
                    "TIMESTAMP WITH TIME ZONE" => truth == "TimestampTz",
                    "INTERVAL YEAR TO MONTH" => truth == "IntervalYm",
                    "INTERVAL DAY TO SECOND" => truth == "IntervalDs",
                    "TEXT" => truth.contains("national: false"),
                    "LOB" => truth.starts_with("Clob") || truth == "Blob",
                    other => panic!("unknown inferred type {other}"),
                };
                if !ok {
                    wrong.push(format!(
                        "{}.{} is {truth} but scavenge said {inferred}",
                        table["name"], cols[i]["name"]
                    ));
                }
            }
        }
        eprintln!(
            "{}: named {named}, abstained {abstained}, wrong {}, types seen {seen:?}",
            g.display(),
            wrong.len()
        );
        assert!(wrong.is_empty(), "{wrong:#?}");
        assert!(named >= 45, "{named} named columns");
        // The LV-072 candidates must actually fire on T_DATETIME.
        for t in [
            "TIMESTAMP WITH TIME ZONE",
            "INTERVAL YEAR TO MONTH",
            "INTERVAL DAY TO SECOND",
        ] {
            assert!(seen.contains(t), "{t} never named");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// LV-073 — `odr scan` takes block size and byte order from the file's own
/// header (block 1, `kcvfh`) instead of assuming 8 KiB little-endian:
/// a text file / random bytes / an empty file are refused (exit 1) instead
/// of being reported as "N damaged blocks"; a real datafile reports
/// `layout_source = header`; a wiped header needs `--force` and says so.
#[test]
fn scan_reads_layout_from_the_header_and_refuses_non_datafiles() {
    let tmp = std::env::temp_dir().join(format!("odr_cli_scan_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let text = tmp.join("text.dbf");
    std::fs::write(&text, "this is not an oracle datafile\n".repeat(400)).unwrap();
    let empty = tmp.join("empty.dbf");
    std::fs::write(&empty, b"").unwrap();
    for f in [&text, &empty] {
        let out = Command::new(env!("CARGO_BIN_EXE_odr"))
            .arg("scan")
            .arg(f)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", f.display());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("not an Oracle datafile header") && err.contains("--force"),
            "{err}"
        );
    }
    // Forced: scans anyway and says the layout was forced.
    let out = Command::new(env!("CARGO_BIN_EXE_odr"))
        .args(["scan", "--block-size", "8192", "--force", "--json"])
        .arg(&text)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["layout_source"], "forced (header unreadable)");
    if let Some(g) = golden() {
        let assm = g.join("datafiles/odr_odr_assm01.dbf");
        let out = Command::new(env!("CARGO_BIN_EXE_odr"))
            .args(["scan", "--json"])
            .arg(&assm)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["block_size"], 8192);
        assert_eq!(v["byte_order"], "Little");
        assert_eq!(v["layout_source"], "header");
        assert_eq!(v["damaged"], 0);
        // A block size that disagrees with the header is refused without --force.
        let out = Command::new(env!("CARGO_BIN_EXE_odr"))
            .args(["scan", "--block-size", "4096"])
            .arg(&assm)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("header says block size 8192"));
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
