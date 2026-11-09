//! `odr fingerprint` — name scavenged objects when the dictionary is gone.
//!
//! **Learn** from a healthy database of the same product (`odr dict --json`
//! output): every table becomes a *fingerprint* — its ordered column-type
//! shape. **Match** a dictionary-less `odr scavenge` report: every
//! `OBJ_<objd>` is compared with every fingerprint and the best candidates
//! are reported **with a confidence score and the reason** — never as a
//! fact. This is the schema-knowledge a vendor (e.g. an ERP house) has and
//! generic tools do not.
//!
//! Matching is deterministic: type-class sequence alignment (exact
//! position, same length) plus a tolerant score for ADD-COLUMN drift; ties
//! are reported as ambiguous. No value content is used — only shapes — so
//! no customer data leaves the machine in a fingerprint file.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

/// Coarse type class, the unit of comparison. Both `odr dict` type names
/// and `odr scavenge` inferred-type strings map onto it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Class {
    Number,
    Text,
    Date,
    Timestamp,
    Interval,
    Lob,
    Raw,
    Rowid,
    Binary,
    Unknown,
}

impl Class {
    fn letter(self) -> char {
        match self {
            Self::Number => 'N',
            Self::Text => 'S',
            Self::Date => 'D',
            Self::Timestamp => 'T',
            Self::Interval => 'I',
            Self::Lob => 'L',
            Self::Raw => 'R',
            Self::Rowid => 'W',
            Self::Binary => 'F',
            Self::Unknown => '?',
        }
    }

    /// From an `odr dict` `type` string (`Debug` of `ColumnType`).
    fn from_dict(t: &str) -> Self {
        let u = t.to_ascii_uppercase();
        if u.starts_with("VARCHAR2")
            || u.starts_with("CHAR")
            || u.starts_with("NVARCHAR")
            || u.starts_with("NCHAR")
            || u == "LONG"
        {
            Self::Text
        } else if u == "NUMBER" {
            Self::Number
        } else if u == "DATE" {
            Self::Date
        } else if u.starts_with("TIMESTAMP") {
            Self::Timestamp
        } else if u.starts_with("INTERVAL") {
            Self::Interval
        } else if u.starts_with("CLOB") || u.starts_with("BLOB") || u.starts_with("NCLOB") {
            Self::Lob
        } else if u.starts_with("RAW") || u == "LONGRAW" {
            Self::Raw
        } else if u == "ROWID" {
            Self::Rowid
        } else if u.starts_with("BINARY") {
            Self::Binary
        } else {
            Self::Unknown
        }
    }

    /// From an `odr scavenge` `inferred_type` string (`NUMBER`, `DATE`,
    /// `TEXT`, `TIMESTAMP…`, `INTERVAL…`, `LOB`, `RAW`, `RAW(ambiguous)`…).
    fn from_inferred(t: &str) -> Self {
        let u = t.to_ascii_uppercase();
        if u.starts_with("TEXT") {
            Self::Text
        } else if u.starts_with("NUMBER") {
            Self::Number
        } else if u.starts_with("DATE") {
            Self::Date
        } else if u.starts_with("TIMESTAMP") {
            Self::Timestamp
        } else if u.starts_with("INTERVAL") {
            Self::Interval
        } else if u.starts_with("LOB") {
            Self::Lob
        } else if u.starts_with("RAW") {
            Self::Raw
        } else {
            Self::Unknown
        }
    }
}

/// One table's fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Fingerprint {
    /// `OWNER.TABLE`.
    pub name: String,
    /// Ordered classes of stored columns.
    pub shape: Vec<Class>,
    /// Compact letter string, for humans (`NSDNN…`).
    pub shape_str: String,
    /// Column names, in the same order (for the suggestion output).
    pub columns: Vec<String>,
}

/// Fingerprint file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Book {
    pub format: u32,
    pub product: String,
    pub source_db: String,
    pub tables: Vec<Fingerprint>,
}

fn shape_str(shape: &[Class]) -> String {
    shape.iter().map(|c| c.letter()).collect()
}

/// Learn fingerprints from an `odr dict --json` report.
pub(crate) fn learn(
    dict_json: &Path,
    product: &str,
    owners: &[String],
    out: &Path,
) -> Result<ExitCode> {
    let v: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dict_json).with_context(|| format!("reading {}", dict_json.display()))?,
    )?;
    let Some(tables) = v.get("tables").and_then(serde_json::Value::as_array) else {
        bail!(
            "{}: not an `odr dict --json` report (no `tables`)",
            dict_json.display()
        );
    };
    let db = v
        .get("files")
        .and_then(serde_json::Value::as_array)
        .and_then(|f| f.first())
        .and_then(|f| f.get("db_name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?")
        .to_owned();
    let mut fps = Vec::new();
    for t in tables {
        let owner = t
            .get("owner")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?");
        let name = t
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?");
        // Skip the data dictionary itself: its shapes are the same in every
        // Oracle database and would only produce false matches.
        if owner == "SYS" || owner == "SYSTEM" {
            continue;
        }
        // `--owner` restricts the book to the product's schemas; Oracle's
        // own option schemas (CTXSYS, MDSYS, …) otherwise flood short
        // shapes with false candidates.
        if !owners.is_empty() && !owners.iter().any(|o| o.eq_ignore_ascii_case(owner)) {
            continue;
        }
        let Some(cols) = t.get("columns").and_then(serde_json::Value::as_array) else {
            continue;
        };
        let mut shape = Vec::with_capacity(cols.len());
        let mut names = Vec::with_capacity(cols.len());
        for c in cols {
            shape.push(Class::from_dict(
                c.get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
            ));
            names.push(
                c.get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?")
                    .to_owned(),
            );
        }
        if shape.is_empty() {
            continue;
        }
        fps.push(Fingerprint {
            name: format!("{owner}.{name}"),
            shape_str: shape_str(&shape),
            shape,
            columns: names,
        });
    }
    let book = Book {
        format: 1,
        product: product.to_owned(),
        source_db: db,
        tables: fps,
    };
    std::fs::write(out, serde_json::to_string_pretty(&book)?)
        .with_context(|| format!("writing {}", out.display()))?;
    println!(
        "{} table fingerprints from {} written to {} (shapes only — no data)",
        book.tables.len(),
        dict_json.display(),
        out.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// Similarity in per-mille between a scavenged shape and a fingerprint.
/// Exact same length and classes ⇒ 1000. Otherwise: positional agreement
/// over the shorter length, scaled down by the length difference, so that
/// an ADD COLUMN at the end still scores high while a different table does
/// not. `Unknown` in the scavenged shape (RAW/ambiguous) is neutral.
fn similarity(scav: &[Class], fp: &[Class]) -> u32 {
    if scav.is_empty() || fp.is_empty() {
        return 0;
    }
    if scav == fp {
        return 1000;
    }
    let n = scav.len().min(fp.len());
    let mut agree = 0u32;
    let mut counted = 0u32;
    for (a, b) in scav.iter().zip(fp.iter()).take(n) {
        if *a == Class::Unknown {
            continue; // neutral: inference abstained
        }
        counted = counted.saturating_add(1);
        if a == b {
            agree = agree.saturating_add(1);
        }
    }
    if counted == 0 {
        return 0;
    }
    let pos = agree.saturating_mul(1000).checked_div(counted).unwrap_or(0);
    let longer = u32::try_from(scav.len().max(fp.len())).unwrap_or(u32::MAX);
    let shorter = u32::try_from(n).unwrap_or(1);
    // Length penalty: ratio of shorter/longer (1 extra column on 10 ⇒ ×0.91).
    pos.saturating_mul(shorter)
        .checked_div(longer.max(1))
        .unwrap_or(0)
}

#[derive(Debug, Serialize)]
struct Candidate {
    table: String,
    score_milli: u32,
    shape: String,
    columns: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Match {
    object: String,
    data_object_id: u64,
    scavenged_shape: String,
    verdict: &'static str,
    candidates: Vec<Candidate>,
}

/// Minimum score to name a candidate at all.
const MIN_SCORE: u32 = 700;
/// A best candidate must lead the runner-up by this much to be "likely".
const MIN_LEAD: u32 = 100;

/// Match a scavenge report against a fingerprint book.
#[allow(clippy::too_many_lines)] // one pass: load, score, rank, print
pub(crate) fn match_report(scavenge_json: &Path, book_path: &Path, json: bool) -> Result<ExitCode> {
    let book: Book = serde_json::from_slice(
        &std::fs::read(book_path).with_context(|| format!("reading {}", book_path.display()))?,
    )?;
    let v: serde_json::Value = serde_json::from_slice(
        &std::fs::read(scavenge_json)
            .with_context(|| format!("reading {}", scavenge_json.display()))?,
    )?;
    let Some(objects) = v.get("objects").and_then(serde_json::Value::as_array) else {
        bail!(
            "{}: not an `odr scavenge` report.json (no `objects`)",
            scavenge_json.display()
        );
    };
    let mut out = Vec::new();
    for o in objects {
        let name = o
            .get("object")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_owned();
        let objd = o
            .get("data_object_id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let shape: Vec<Class> = o
            .get("columns")
            .and_then(serde_json::Value::as_array)
            .map(|cs| {
                cs.iter()
                    .map(|c| {
                        Class::from_inferred(
                            c.get("inferred_type")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or(""),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut cands: Vec<Candidate> = book
            .tables
            .iter()
            .map(|fp| Candidate {
                table: fp.name.clone(),
                score_milli: similarity(&shape, &fp.shape),
                shape: fp.shape_str.clone(),
                columns: fp.columns.clone(),
            })
            .filter(|c| c.score_milli >= MIN_SCORE)
            .collect();
        cands.sort_by(|a, b| {
            b.score_milli
                .cmp(&a.score_milli)
                .then(a.table.cmp(&b.table))
        });
        let verdict = match cands.as_slice() {
            [] => "no match",
            [only] => {
                if only.score_milli == 1000 {
                    "exact shape"
                } else {
                    "likely"
                }
            }
            [best, second, ..] => {
                if best.score_milli == 1000 && second.score_milli < 1000 {
                    "exact shape"
                } else if best.score_milli.saturating_sub(second.score_milli) >= MIN_LEAD {
                    "likely"
                } else {
                    "ambiguous"
                }
            }
        };
        cands.truncate(3);
        out.push(Match {
            object: name,
            data_object_id: objd,
            scavenged_shape: shape_str(&shape),
            verdict,
            candidates: cands,
        });
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "tool": "odr fingerprint match", "book": book_path.display().to_string(),
                "product": book.product, "source_db": book.source_db,
                "note": "every name below is a suggestion from column-type shape only; verify before relying on it",
                "matches": out,
            }))?
        );
    } else {
        println!(
            "fingerprint book: {} ({} tables, product {})",
            book_path.display(),
            book.tables.len(),
            book.product
        );
        println!(
            "every name is a SUGGESTION from column-type shape only — verify before relying on it\n"
        );
        let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
        for m in &out {
            let e = tally.entry(m.verdict).or_insert(0);
            *e = e.saturating_add(1);
            println!(
                "{:<12} shape {:<24} {}",
                m.object, m.scavenged_shape, m.verdict
            );
            for c in &m.candidates {
                println!(
                    "             → {:<32} {}‰  {}",
                    c.table, c.score_milli, c.shape
                );
                if m.verdict != "ambiguous" {
                    println!("               columns: {}", c.columns.join(", "));
                }
            }
        }
        println!();
        for (k, v) in tally {
            println!("{k:<12} {v}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_map_both_vocabularies() {
        assert_eq!(Class::from_dict("Varchar2 { len: 100 }"), Class::Text);
        assert_eq!(Class::from_dict("Number"), Class::Number);
        assert_eq!(Class::from_dict("Clob { .. }"), Class::Lob);
        assert_eq!(Class::from_inferred("TEXT"), Class::Text);
        assert_eq!(Class::from_inferred("RAW(ambiguous)"), Class::Raw);
        assert_eq!(
            Class::from_inferred("INTERVAL DAY TO SECOND"),
            Class::Interval
        );
    }

    #[test]
    fn similarity_rewards_exact_and_tolerates_add_column() {
        use Class::{Date, Number, Text, Unknown};
        let fp = [Number, Text, Date, Number, Number, Text];
        assert_eq!(similarity(&fp, &fp), 1000);
        // one column appended: 6/7 positions ⇒ 1000 × 6/7 = 857
        let plus = [Number, Text, Date, Number, Number, Text, Number];
        assert_eq!(similarity(&plus, &fp), 857);
        // abstained column is neutral
        let abst = [Number, Unknown, Date, Number, Number, Text];
        assert_eq!(similarity(&abst, &fp), 1000 * 6 / 6);
        // different table of same length
        let other = [Text, Text, Text, Text, Text, Text];
        assert!(similarity(&other, &fp) < MIN_SCORE);
        assert_eq!(similarity(&[], &fp), 0);
    }
}
