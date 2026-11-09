//! `odr triage` — diagnose every datafile in a directory (or a list of
//! files) and rank them by what needs attention first.
//!
//! Ranking: lowest recoverability score first; ties broken by damaged block
//! count. Non-Oracle files are listed separately, never silently skipped.
//! Exit codes: 0 all healthy · 1 I/O / usage error · 2 at least one file
//! damaged or undecidable.

use anyhow::{Context, Result};
use odr_core::diagnose::{self, Band, Depth, Options, ReadAt};
use odr_core::source::Evidence;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::diagnose_cmd::Lang;

struct EvidenceReader<'a>(&'a mut Evidence);

impl ReadAt for EvidenceReader<'_> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0
            .read_at(offset, buf)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
    fn len(&self) -> u64 {
        self.0.len()
    }
}

/// One row of the triage table.
#[derive(Debug, serde::Serialize)]
struct Row {
    file: String,
    bytes: u64,
    score: Option<u8>,
    band: Option<Band>,
    damaged: u64,
    examined: u64,
    live_rows: Option<u64>,
    top_finding: Option<String>,
    layout_source: Option<String>,
    error: Option<String>,
}

fn collect(inputs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in inputs {
        if p.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(p)
                .with_context(|| format!("reading {}", p.display()))?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|q| {
                    q.is_file()
                        && q.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                            e.eq_ignore_ascii_case("dbf") || e.eq_ignore_ascii_case("ora")
                        })
                })
                .collect();
            entries.sort();
            out.extend(entries);
        } else {
            out.push(p.clone());
        }
    }
    Ok(out)
}

fn one(path: &Path, fast: bool) -> Row {
    let name = path.display().to_string();
    let mut ev = match Evidence::open(path) {
        Ok(e) => e,
        Err(e) => {
            return Row {
                file: name,
                bytes: 0,
                score: None,
                band: None,
                damaged: 0,
                examined: 0,
                live_rows: None,
                top_finding: None,
                layout_source: None,
                error: Some(e.to_string()),
            };
        }
    };
    let bytes = ev.len();
    let opts = Options {
        depth: if fast {
            Depth::Physical
        } else {
            Depth::Logical
        },
        ..Options::default()
    };
    let mut rd = EvidenceReader(&mut ev);
    match diagnose::diagnose(&mut rd, None, opts) {
        Ok(Some(r)) => {
            let r = r.with_triage(&name);
            let t = r.triage.as_ref();
            Row {
                file: name,
                bytes,
                score: t.map(|t| t.score),
                band: t.map(|t| t.band),
                damaged: r.counters.damaged_total(),
                examined: r.counters.examined,
                live_rows: t.and_then(|t| t.live_rows),
                top_finding: r.findings.first().map(|f| format!("{:?}", f.code)),
                layout_source: Some(format!("{:?}", r.layout.source)),
                error: None,
            }
        }
        Ok(None) => Row {
            file: name,
            bytes,
            score: None,
            band: None,
            damaged: 0,
            examined: 0,
            live_rows: None,
            top_finding: None,
            layout_source: None,
            error: Some(
                "not an Oracle datafile (no header, no OS signature, no inferable layout)".into(),
            ),
        },
        Err(e) => Row {
            file: name,
            bytes,
            score: None,
            band: None,
            damaged: 0,
            examined: 0,
            live_rows: None,
            top_finding: None,
            layout_source: None,
            error: Some(e.to_string()),
        },
    }
}

/// Run the command.
pub(crate) fn triage(inputs: &[PathBuf], fast: bool, json: bool, lang: Lang) -> Result<ExitCode> {
    let files = collect(inputs)?;
    if files.is_empty() {
        anyhow::bail!("no .dbf files found");
    }
    let mut rows: Vec<Row> = files.iter().map(|p| one(p, fast)).collect();
    // Worst first: undecidable, then lowest score, then most damage.
    rows.sort_by(|a, b| {
        let ka = (
            a.error.is_none(),
            a.score.unwrap_or(0),
            u64::MAX.saturating_sub(a.damaged),
        );
        let kb = (
            b.error.is_none(),
            b.score.unwrap_or(0),
            u64::MAX.saturating_sub(b.damaged),
        );
        ka.cmp(&kb)
    });
    let any_bad = rows.iter().any(|r| r.error.is_some() || r.damaged > 0);
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        let (h_file, h_score, h_dmg, h_rows, h_top, h_total) = match lang {
            Lang::En => (
                "file",
                "score",
                "damaged/examined",
                "live rows",
                "top finding",
                "files",
            ),
            Lang::Ar => (
                "الملف",
                "الدرجة",
                "تالفة/مفحوصة",
                "صفوف حيّة",
                "أهم نتيجة",
                "ملفات",
            ),
        };
        println!(
            "{:<4} {:<7} {:<18} {:<11} {:<22} {}",
            "#", h_score, h_dmg, h_rows, h_top, h_file
        );
        for (i, r) in rows.iter().enumerate() {
            let score = match (r.score, r.band) {
                (Some(s), Some(b)) => format!("{s:>3} {}", band_short(b, lang)),
                _ => match lang {
                    Lang::En => "  —  ERR".into(),
                    Lang::Ar => "  —  خطأ".into(),
                },
            };
            let dmg = if r.error.is_some() {
                String::new()
            } else {
                format!("{}/{}", r.damaged, r.examined)
            };
            println!(
                "{:<4} {:<7} {:<18} {:<11} {:<22} {}",
                i.saturating_add(1),
                score,
                dmg,
                r.live_rows.map_or(String::new(), |n| n.to_string()),
                r.top_finding.clone().unwrap_or_default(),
                r.file
            );
            if let Some(e) = &r.error {
                println!("     ↳ {e}");
            }
        }
        let total_rows: u64 = rows
            .iter()
            .filter_map(|r| r.live_rows)
            .fold(0, u64::saturating_add);
        let total_dmg: u64 = rows.iter().map(|r| r.damaged).fold(0, u64::saturating_add);
        println!();
        match lang {
            Lang::En => println!(
                "{} {h_total} · {} undecidable · {total_dmg} damaged blocks · {total_rows} live rows counted",
                rows.len(),
                rows.iter().filter(|r| r.error.is_some()).count()
            ),
            Lang::Ar => println!(
                "{} {h_total} · {} غير قابل للتحديد · {total_dmg} كتلة تالفة · {total_rows} صف حيّ",
                rows.len(),
                rows.iter().filter(|r| r.error.is_some()).count()
            ),
        }
    }
    Ok(if any_bad {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

fn band_short(b: Band, lang: Lang) -> &'static str {
    match (b, lang) {
        (Band::Excellent, Lang::En) => "EXC",
        (Band::Good, Lang::En) => "GOOD",
        (Band::Partial, Lang::En) => "PART",
        (Band::Poor, Lang::En) => "POOR",
        (Band::Lost, Lang::En) => "LOST",
        (Band::Excellent, Lang::Ar) => "ممتاز",
        (Band::Good, Lang::Ar) => "جيد",
        (Band::Partial, Lang::Ar) => "جزئي",
        (Band::Poor, Lang::Ar) => "ضعيف",
        (Band::Lost, Lang::Ar) => "ضائع",
    }
}
