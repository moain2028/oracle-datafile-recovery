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
