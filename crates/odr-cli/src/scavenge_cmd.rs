//! `odr scavenge` — dictionary-less extraction (M4).
//!
//! When the SYSTEM datafile is gone there are no table names, no column
//! names and no column types. What the data blocks still carry is exact:
//! the data object id (`ktbbh.objd`), the row directory and the stored
//! column bytes. ODR groups data blocks by `objd`, writes one table
//! `OBJ_<objd>` per object with columns `COL_1..COL_n`, and labels *every*
//! cell **inferred** — never verified — because the column type is a
//! guess, however well supported.
//!
//! The type of a column is inferred **per object, not per cell**: a column
//! is called `NUMBER` only if every non-NULL value of that column across
//! the object decodes as a strictly normalised Oracle NUMBER; `DATE` only
//! if every value is 7 bytes and decodes as a valid date; and so on in a
//! fixed precedence (`DATE`, `TIMESTAMP`, `NUMBER`, `TEXT`). A column where
//! the values disagree stays `RAW` and is printed as hex. The rule that
//! named the type is written in `_inference` for every row, so a reader
//! can see exactly what was assumed.
//!
//! Integrity rules are the same as `odr extract`: a block that fails its
//! checksum or tail check (LV-059), or a structural invariant (LV-062), is
//! untrusted and its rows go to `rejected.csv` with the reason. A row that
//! is chained, migrated or incomplete is rejected too: without the
//! dictionary the continuation cannot be verified to belong to this row.

use anyhow::{Context, Result};
use odr_core::ByteOrder;
use odr_core::block::{BlockHeader, BlockStatus, classify};
use odr_core::charset::{self, Charset};
use odr_core::dict::kcvfh::FileHeader;
use odr_core::row::DataBlock;
use odr_core::source::Evidence;
use odr_core::types::{IntervalDs, IntervalYm, LobLocator, OraDate, OraNumber, OraTimestamp};
use odr_core::value::{ColumnType, decode_value};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Inferred type of one column of one object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guess {
    Date,
    Timestamp,
    /// `TIMESTAMP WITH TIME ZONE`: 13 bytes, valid date/time, offset or
    /// region in range (LV-072).
    TimestampTz,
    /// `INTERVAL YEAR TO MONTH`: 5 bytes, months −11..11, signs agree.
    IntervalYm,
    /// `INTERVAL DAY TO SECOND`: 11 bytes, fields in range, signs agree.
    IntervalDs,
    Number,
    Text,
    /// A LOB locator (`00 70 00 01` header, LV-068) — CLOB/BLOB. Only the
    /// inline part is printed; out-of-line locators stay hex.
    Lob,
    Raw,
}

/// Candidates tried, in display order. `Raw` is the fallback, not a candidate.
///
/// **Not** a candidate (measured, LV-072): national text (`AL16UTF16`).
/// Every 2-byte NUMBER (`c1 02`…) is also one printable UTF-16 code unit
/// and every even-length ASCII string is valid UTF-16 too, so on the 26ai
/// corpus an NTEXT candidate made 15 NUMBER and 2 TEXT columns ambiguous
/// without naming a single national column unambiguously. National text
/// therefore stays `RAW` (hex) — a declared limit of byte-only inference.
/// `BINARY_FLOAT`/`BINARY_DOUBLE` are not candidates either: any 4/8 bytes
/// decode, so they would fit every NUMBER/DATE column of that width.
const CANDIDATES: [Guess; 8] = [
    Guess::Date,
    Guess::Timestamp,
    Guess::TimestampTz,
    Guess::IntervalYm,
    Guess::IntervalDs,
    Guess::Number,
    Guess::Text,
    Guess::Lob,
];

impl Guess {
    const fn name(self) -> &'static str {
        match self {
            Self::Date => "DATE",
            Self::Timestamp => "TIMESTAMP",
            Self::TimestampTz => "TIMESTAMP WITH TIME ZONE",
            Self::IntervalYm => "INTERVAL YEAR TO MONTH",
            Self::IntervalDs => "INTERVAL DAY TO SECOND",
            Self::Number => "NUMBER",
            Self::Text => "TEXT",
            Self::Lob => "LOB",
            Self::Raw => "RAW",
        }
    }
    const fn column_type(self, cs: Charset) -> Option<ColumnType> {
        let _ = cs;
        match self {
            Self::Date => Some(ColumnType::Date),
            Self::Timestamp => Some(ColumnType::Timestamp),
            Self::TimestampTz => Some(ColumnType::TimestampTz),
            Self::IntervalYm => Some(ColumnType::IntervalYm),
            Self::IntervalDs => Some(ColumnType::IntervalDs),
            Self::Number => Some(ColumnType::Number),
            Self::Text => Some(ColumnType::Varchar2 { national: false }),
            // Character/binary is read from the locator itself; the column
            // type only selects the LOB branch of the decoder.
            Self::Lob => Some(ColumnType::Clob { national: false }),
            Self::Raw => None,
        }
    }
}

/// Printable text: no C0 control bytes other than TAB/LF/CR.
fn printable(raw: &[u8]) -> bool {
    raw.iter()
        .all(|&b| b >= 0x20 || matches!(b, 0x09 | 0x0A | 0x0D))
}

/// Does `raw` decode *strictly* as `g`? Strict means the decoder accepts
/// it with no error and no invalid sequence.
fn fits(raw: &[u8], g: Guess, cs: Charset) -> bool {
    match g {
        Guess::Date => OraDate::decode(raw).is_ok(),
        Guess::Timestamp => OraTimestamp::decode(raw).is_ok(),
        Guess::TimestampTz => OraTimestamp::decode_tz(raw).is_ok(),
        Guess::IntervalYm => IntervalYm::decode(raw).is_ok(),
        Guess::IntervalDs => IntervalDs::decode(raw).is_ok(),
        Guess::Number => OraNumber::decode(raw).is_ok(),
        // Text: exact in the character set and free of C0 control bytes
        // other than TAB/LF/CR — binary NUMBER/DATE bytes almost always
        // contain one, which is what keeps them out of this bucket.
        Guess::Text => charset::decode(raw, cs).is_exact() && printable(raw),
        Guess::Lob => LobLocator::parse(raw).is_ok(),
        Guess::Raw => true,
    }
}

/// Why a column stayed RAW.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RawWhy {
    /// No non-NULL value at all.
    NoEvidence,
    /// No candidate type fits every value.
    NoneFits,
    /// More than one candidate fits every value — the bytes alone cannot
    /// tell them apart, so no value is printed as either.
    Ambiguous(Vec<Guess>),
}

/// Fraction of evidence blocks a type must fit to be named: two thirds.
/// Below that there is no single type the object agrees on.
const AGREE_NUM: usize = 2;
const AGREE_DEN: usize = 3;

/// `(file, block)` of a data block.
type BlockKey = (u32, u32);
/// Non-NULL values of one column inside one block.
type BlockValues<'a> = (BlockKey, Vec<&'a [u8]>);
/// One stored row: file, block, slot, columns in segment order.
type StoredRow = (u32, u32, u16, Vec<Option<Vec<u8>>>);

/// Outcome of inferring one column of one object.
struct ColumnGuess {
    guess: Result<Guess, RawWhy>,
    /// Blocks holding at least one value of this column that does **not**
    /// fit the named type — the LV-064 rule in dictionary-less form: one
    /// value that breaks the type every other block agrees on is evidence
    /// that the whole block is altered, so all of its rows are rejected.
    tainted: Vec<BlockKey>,
}

/// Infer one column's type **per block, then per object**. `by_block` is
/// `(block, non-NULL values of this column in that block)`.
///
/// A candidate fits a block if it fits every value in it. A type is named
/// when exactly one candidate fits at least two thirds of the blocks that
/// carry a value (DATE/TIMESTAMP(0) are byte-identical and merge); the
/// blocks it does not fit are tainted. No candidate reaching the bar, or
/// two reaching it, means RAW — reported, never resolved by precedence.
fn infer(by_block: &[BlockValues<'_>], cs: Charset) -> ColumnGuess {
    let evidence: Vec<&BlockValues<'_>> = by_block.iter().filter(|(_, v)| !v.is_empty()).collect();
    if evidence.is_empty() {
        return ColumnGuess {
            guess: Err(RawWhy::NoEvidence),
            tainted: Vec::new(),
        };
    }
    let fits_block = |g: Guess, vals: &[&[u8]]| vals.iter().all(|v| fits(v, g, cs));
    let reaching: Vec<Guess> = CANDIDATES
        .into_iter()
        .filter(|&g| {
            let n = evidence.iter().filter(|(_, v)| fits_block(g, v)).count();
            n.saturating_mul(AGREE_DEN) >= evidence.len().saturating_mul(AGREE_NUM)
        })
        .collect();
    let chosen = match reaching.as_slice() {
        [] => Err(RawWhy::NoneFits),
        [g] => Ok(*g),
        [Guess::Date, Guess::Timestamp] => Ok(Guess::Date),
        many => Err(RawWhy::Ambiguous(many.to_vec())),
    };
    let tainted = match chosen {
        Ok(g) => evidence
            .iter()
            .filter(|(_, v)| !fits_block(g, v))
            .map(|(b, _)| *b)
            .collect(),
        Err(_) => Vec::new(),
    };
    ColumnGuess {
        guess: chosen,
        tainted,
    }
}

/// One object's rows as stored bytes, with provenance.
#[derive(Default)]
struct ObjectRows {
    /// `(file, block, slot, columns)` for rows from trusted blocks.
    rows: Vec<StoredRow>,
    /// `(file, block, slot, reason)` for everything else.
    rejected: Vec<(u32, u32, u16, String)>,
    /// Widest column count seen.
    ncols: usize,
    blocks: usize,
}

fn bump(map: &mut BTreeMap<String, usize>, key: String) {
    let e = map.entry(key).or_default();
    *e = e.saturating_add(1);
}

fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `odr scavenge`.
#[allow(clippy::too_many_lines)]
pub(crate) fn scavenge(
    files: &[PathBuf],
    out: &Path,
    charset_name: Option<&str>,
    only_objd: Option<u32>,
    min_rows: usize,
) -> Result<std::process::ExitCode> {
    let cs = match charset_name {
        Some(n) => {
            Charset::from_oracle_name(n).with_context(|| format!("unknown character set {n}"))?
        }
        None => Charset::Al32Utf8,
    };
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let mut objects: BTreeMap<u32, ObjectRows> = BTreeMap::new();
    let mut inputs = Vec::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut order_seen: Option<ByteOrder> = None;

    for p in files {
        let mut ev = Evidence::open(p).with_context(|| format!("opening {}", p.display()))?;
        // Block size and byte order from kcvfh (block 1). Without it the
        // file cannot be read: block size is not guessed here.
        let mut hdr: Option<FileHeader> = None;
        for bs in [2048usize, 4096, 8192, 16384, 32768] {
            let mut buf = vec![0u8; bs];
            if ev.read_at(bs as u64, &mut buf)? != bs {
                continue;
            }
            for o in [ByteOrder::Little, ByteOrder::Big] {
                if let Ok(h) = FileHeader::parse(&buf, o)
                    && usize::try_from(h.block_size).ok() == Some(bs)
                {
                    hdr = Some(h);
                }
            }
            if hdr.is_some() {
                break;
            }
        }
        let Some(h) = hdr else {
            anyhow::bail!(
                "{}: block 1 is not an Oracle datafile header (scavenge needs the block size and byte order from it)",
                p.display()
            );
        };
        let order = h.order;
        if let Some(o) = order_seen
            && o != order
        {
            anyhow::bail!("{}: byte order differs from the other files", p.display());
        }
        order_seen = Some(order);
        let bs = usize::try_from(h.block_size).context("block size")?;
        let file = u32::from(h.file_number);
        let bs64 = u64::try_from(bs).context("bs")?;
        let nblocks = ev.len().checked_div(bs64).unwrap_or(0);
        let mut buf = vec![0u8; bs];
        for b in 1..nblocks {
            let Ok(b32) = u32::try_from(b) else { break };
            if ev.read_at(b.saturating_mul(bs64), &mut buf)? != bs {
                break;
            }
            let status = classify(&buf, bs, order);
            if matches!(status, BlockStatus::Zeroed | BlockStatus::Truncated) {
                continue;
            }
            let Ok(bh) = BlockHeader::parse(&buf, order) else {
                bump(&mut skipped, format!("integrity: {status:?}"));
                continue;
            };
            if bh.type_byte != 0x06 {
                if status != BlockStatus::Valid {
                    bump(&mut skipped, format!("integrity: {status:?}"));
                }
                continue;
            }
            let Ok(db) = DataBlock::parse(&buf, order) else {
                bump(
                    &mut skipped,
                    if status == BlockStatus::Valid {
                        format!(
                            "type 0x06 block that is not a table data block (index leaf/branch or unsupported layout), objd {}",
                            odr_core::row::data_object_id(&buf, order).unwrap_or(0)
                        )
                    } else {
                        format!("integrity: {status:?}")
                    },
                );
                continue;
            };
            let objd = db.ktb.objd;
            if only_objd.is_some_and(|o| o != objd) {
                continue;
            }
            // Clustered blocks carry rows of several tables: without the
            // dictionary they cannot be told apart. Reported, not guessed.
            // (`kdbhflag` K bit is not reliable — LV-018; `ntab` and the row
            // flags are.)
            if db.kdbh.ntab > 1 {
                bump(&mut skipped, "cluster block (needs dictionary)".to_owned());
                continue;
            }
            let structural = if status == BlockStatus::Valid {
                db.structural_check()
            } else {
                None
            };
            let trusted = status == BlockStatus::Valid && structural.is_none();
            let o = objects.entry(objd).or_default();
            o.blocks = o.blocks.saturating_add(1);
            for slot in 0..db.kdbh.nrow {
                let untrusted_reason = || match structural {
                    Some(f) => format!("block structure: {f}"),
                    None => format!("block integrity: {status:?}"),
                };
                match db.row(slot) {
                    Ok(None) => {
                        if !trusted {
                            o.rejected.push((file, b32, slot, untrusted_reason()));
                        }
                    }
                    Err(e) => {
                        if !trusted {
                            o.rejected.push((file, b32, slot, untrusted_reason()));
                        } else if db
                            .row_flag(slot)
                            .is_some_and(|f| f & odr_core::row::flag::D != 0)
                        {
                            // deleted stub whose bytes were reused (LV-058)
                        } else {
                            o.rejected
                                .push((file, b32, slot, format!("row did not decode: {e}")));
                        }
                    }
                    Ok(Some(p)) => {
                        if !trusted {
                            o.rejected.push((file, b32, slot, untrusted_reason()));
                            continue;
                        }
                        if p.is_deleted() {
                            continue;
                        }
                        if !p.is_complete() || p.next.is_some() || p.head.is_some() {
                            o.rejected.push((
                                file,
                                b32,
                                slot,
                                "chained or migrated row piece (continuation cannot be verified without the dictionary)".to_owned(),
                            ));
                            continue;
                        }
                        if p.is_cluster_key() || p.is_cluster_member() {
                            o.rejected.push((
                                file,
                                b32,
                                slot,
                                "cluster row (needs dictionary)".to_owned(),
                            ));
                            continue;
                        }
                        o.ncols = o.ncols.max(p.columns.len());
                        o.rows.push((
                            file,
                            b32,
                            slot,
                            p.columns.iter().map(|c| c.map(<[u8]>::to_vec)).collect(),
                        ));
                    }
                }
            }
        }
        let unchanged = ev.verify_unchanged()?;
        inputs.push(serde_json::json!({
            "path": p.display().to_string(), "sha256": hex(ev.sha256()), "unchanged": unchanged,
            "file": file, "block_size": bs, "blocks": nblocks, "order": format!("{order:?}"),
        }));
    }

    // Write one table per object.
    let mut summary = Vec::new();
    let mut any_rejected = false;
    for (objd, o) in &objects {
        if o.rows.len() < min_rows && o.rejected.is_empty() {
            continue;
        }
        let ncols = o.ncols;
        // Per-column inference over every non-NULL value of the object.
        let column_guesses: Vec<ColumnGuess> = (0..ncols)
            .map(|i| {
                let mut by_block: BTreeMap<BlockKey, Vec<&[u8]>> = BTreeMap::new();
                for (file, block, _, cols) in &o.rows {
                    let e = by_block.entry((*file, *block)).or_default();
                    if let Some(v) = cols.get(i).and_then(|c| c.as_deref()) {
                        e.push(v);
                    }
                }
                infer(&by_block.into_iter().collect::<Vec<_>>(), cs)
            })
            .collect();
        // LV-064 (dictionary-less form): a block with a value that breaks
        // the agreed type is rejected whole, with the column named.
        let mut tainted: BTreeMap<BlockKey, String> = BTreeMap::new();
        for (i, cg) in column_guesses.iter().enumerate() {
            if let Ok(g) = cg.guess {
                for b in &cg.tainted {
                    tainted.entry(*b).or_insert_with(|| {
                        format!(
                            "block tainted: a COL_{} value does not decode as {}, the type the other blocks of OBJ_{objd} agree on (LV-064)",
                            i.saturating_add(1),
                            g.name()
                        )
                    });
                }
            }
        }
        let inferred: Vec<Result<Guess, RawWhy>> =
            column_guesses.into_iter().map(|c| c.guess).collect();
        let guesses: Vec<Guess> = inferred
            .iter()
            .map(|r| r.as_ref().copied().unwrap_or(Guess::Raw))
            .collect();
        let describe = |r: &Result<Guess, RawWhy>| match r {
            Ok(g) => g.name().to_owned(),
            Err(RawWhy::NoEvidence) => "RAW(no non-NULL value)".to_owned(),
            Err(RawWhy::NoneFits) => "RAW(no type fits every value)".to_owned(),
            Err(RawWhy::Ambiguous(gs)) => format!(
                "RAW(ambiguous: {})",
                gs.iter().map(|g| g.name()).collect::<Vec<_>>().join("|")
            ),
        };
        let rule = inferred
            .iter()
            .enumerate()
            .map(|(i, r)| format!("COL_{}={}", i.saturating_add(1), describe(r)))
            .collect::<Vec<_>>()
            .join(" ");
        let why = format!(
            "dictionary-less: types inferred per column from all values of OBJ_{objd} ({rule})"
        );
        let dir = out.join(format!("OBJ_{objd}"));
        std::fs::create_dir_all(&dir)?;
        let header: String = (1..=ncols).fold(String::from("_file,_block,_slot"), |mut s, i| {
            use std::fmt::Write as _;
            let _ = write!(s, ",COL_{i}");
            s
        });
        let mut w_inf = std::io::BufWriter::new(std::fs::File::create(dir.join("inferred.csv"))?);
        let mut w_rej = std::io::BufWriter::new(std::fs::File::create(dir.join("rejected.csv"))?);
        writeln!(w_inf, "{header},_inference")?;
        writeln!(w_rej, "{header},_reason")?;
        let (mut n_inf, mut n_rej) = (0usize, 0usize);
        for (file, block, slot, cols) in &o.rows {
            let mut line = format!("{file},{block},{slot}");
            let mut reject: Option<String> = tainted.get(&(*file, *block)).cloned();
            for i in 0..ncols {
                line.push(',');
                let Some(raw) = cols.get(i).and_then(|c| c.as_deref()) else {
                    continue;
                };
                match guesses
                    .get(i)
                    .copied()
                    .unwrap_or(Guess::Raw)
                    .column_type(cs)
                {
                    None => line.push_str(&hex(raw)),
                    Some(ty) => {
                        let v = decode_value(
                            Some(raw),
                            ty,
                            odr_core::value::Charsets {
                                database: cs,
                                national: Charset::Al16Utf16,
                            },
                        );
                        if !v.is_exact() {
                            // Cannot happen after `fits`, but never trust it.
                            reject = Some(format!(
                                "column {} does not decode as inferred type",
                                i.saturating_add(1)
                            ));
                        }
                        if let Some(t) = v.canonical() {
                            line.push_str(&csv(&t));
                        }
                    }
                }
            }
            match reject {
                None => {
                    writeln!(w_inf, "{line},{}", csv(&why))?;
                    n_inf = n_inf.saturating_add(1);
                }
                Some(r) => {
                    writeln!(w_rej, "{line},{}", csv(&r))?;
                    n_rej = n_rej.saturating_add(1);
                }
            }
        }
        for (file, block, slot, reason) in &o.rejected {
            writeln!(
                w_rej,
                "{file},{block},{slot}{},{}",
                ",".repeat(ncols),
                csv(reason)
            )?;
            n_rej = n_rej.saturating_add(1);
        }
        w_inf.flush()?;
        w_rej.flush()?;
        any_rejected |= n_rej > 0;
        summary.push(serde_json::json!({
            "object": format!("OBJ_{objd}"), "data_object_id": objd, "blocks": o.blocks,
            "columns": inferred.iter().enumerate().map(|(i, r)| serde_json::json!({"name": format!("COL_{}", i.saturating_add(1)), "inferred_type": describe(r)})).collect::<Vec<_>>(),
            "rows": {"verified": 0, "inferred": n_inf, "rejected": n_rej},
            "tainted_blocks": tainted.len(),
        }));
    }
    let report = serde_json::json!({
        "tool": "odr scavenge", "version": env!("CARGO_PKG_VERSION"),
        "mode": "dictionary-less (M4): no table or column names; every cell is inferred, none verified",
        "charset": format!("{cs:?}"), "charset_source": if charset_name.is_some() { "--charset" } else { "default AL32UTF8 (no PROPS$ without SYSTEM)" },
        "inputs": inputs, "objects": summary, "skipped_blocks": skipped,
    });
    std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!(
        "{} object(s) written to {}; every cell is inferred (no dictionary)",
        report
            .get("objects")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len),
        out.display()
    );
    Ok(std::process::ExitCode::from(if any_rejected {
        2
    } else {
        0
    }))
}
