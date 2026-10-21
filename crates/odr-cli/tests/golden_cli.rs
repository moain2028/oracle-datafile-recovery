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
    assert_eq!(odr.len(), 15, "{odr:?}");
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
        let out_dir = tmp.join(table);
        let out = Command::new(env!("CARGO_BIN_EXE_odr"))
            .arg("extract")
            .args(files(&g))
            .args(["--table", &format!("ODR.{table}"), "--out"])
            .arg(&out_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
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
            v + i,
            counts[table],
            "{table}: verified+inferred == Oracle COUNT(*)"
        );
        assert_eq!(i, want_inferred, "{table}: inferred rows");
        assert_eq!(r, 0, "{table}: rejected rows");
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
    assert!(
        ver.lines()
            .nth(1)
            .unwrap()
            .starts_with("16,2724,5,6,new,8,")
    );
    let _ = std::fs::remove_dir_all(&tmp);
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
        let inferred = csv_rows(&out_dir.join("inferred.csv"));
        assert!(inferred.is_empty(), "{name}: unexpected inferred rows");
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
            verified.len() + rejected + zeroed_rows,
            truth.len(),
            "{name}: verified {} + rejected {rejected} + unreadable(zeroed) {zeroed_rows}",
            verified.len()
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
