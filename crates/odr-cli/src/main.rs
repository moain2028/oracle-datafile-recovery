//! `odr` — Oracle Datafile Recovery CLI. **Owner: Astra**.
//!
//! Output contract: human text by default, `--json` for machine output.
//! Exit codes: 0 ok · 1 usage/IO error · 2 completed with damaged blocks.

mod diagnose_cmd;
mod dict_cmd;
mod scavenge_cmd;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use odr_core::ByteOrder;
use odr_core::block::{BLOCK_SIZES, BlockStatus};
use odr_core::scan::scan_image;
use odr_core::source::Evidence;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "odr", version, about = "Oracle Datafile Recovery (read-only)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Endian {
    Little,
    Big,
}

#[derive(Clone, Copy, ValueEnum)]
enum LangArg {
    En,
    Ar,
}

#[derive(Subcommand)]
enum Cmd {
    /// Detect, classify and explain damage in a datafile (read-only):
    /// every block judged (data/index/LOB/empty…, zeroed/encrypted/torn/
    /// checksum/misplaced/logical), damage regions, per-object summary and
    /// interpreted findings with cause, recoverability and Oracle errors.
    /// Block size and byte order come from the header, the OS block, or
    /// are inferred from block signatures when both are destroyed.
    Diagnose {
        /// Path to the datafile.
        file: PathBuf,
        /// Block size (only honoured with `--force`; otherwise it must agree
        /// with the file).
        #[arg(long)]
        block_size: Option<usize>,
        /// Byte order (with `--force`).
        #[arg(long, value_enum)]
        endian: Option<Endian>,
        /// Use `--block-size`/`--endian` even when the file disagrees or
        /// cannot be read. Recorded in the output.
        #[arg(long)]
        force: bool,
        /// Physical checks only (skip the per-block structural check and
        /// row counting). Faster; misses logical corruption.
        #[arg(long)]
        fast: bool,
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Report language for text output.
        #[arg(long, value_enum, default_value = "en")]
        lang: LangArg,
        /// List up to N damaged blocks individually.
        #[arg(long, default_value_t = 0)]
        list: usize,
        /// Also write a self-contained HTML report (printable, same language
        /// as `--lang`) to this path.
        #[arg(long)]
        html: Option<PathBuf>,
        /// Also write a DB3F file (Database Forensic File Format, DFRWS
        /// 2019: one JSON header line, then one JSON object per block).
        #[arg(long)]
        db3f: Option<PathBuf>,
    },
    /// Classify every block of a datafile (read-only) and print a summary.
    Scan {
        /// Path to the datafile.
        file: PathBuf,
        /// Block size in bytes. Default: read from the file header (block 1,
        /// `kcvfh`); an explicit value that disagrees with the header is
        /// reported and the header wins unless `--force` is given.
        #[arg(long)]
        block_size: Option<usize>,
        /// Byte order of the source platform. Default: detected from the
        /// file header; same disagreement rule as `--block-size`.
        #[arg(long, value_enum)]
        endian: Option<Endian>,
        /// Use the given `--block-size`/`--endian` even if block 1 disagrees
        /// or is unreadable (header destroyed). Recorded in the output.
        #[arg(long)]
        force: bool,
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Read the data dictionary from a SYSTEM datafile (plus any other
    /// datafiles of the same database) and list tables and columns.
    Dict {
        /// Datafiles (one must be SYSTEM).
        files: Vec<PathBuf>,
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Identity for a file whose header (block 1) is destroyed:
        /// `PATH=file:N,ts:N[,rfile:N],bs:N`. Repeatable. Recorded as assumed.
        #[arg(long = "assume")]
        assume: Vec<String>,
    },
    /// Extract one table into three layers: verified / inferred / rejected.
    Extract {
        /// Datafiles (SYSTEM + the file(s) holding the table).
        files: Vec<PathBuf>,
        /// Table as OWNER.NAME.
        #[arg(long)]
        table: String,
        /// Output directory (created).
        #[arg(long)]
        out: PathBuf,
        /// Database character set name (overrides PROPS$).
        #[arg(long)]
        charset: Option<String>,
        /// Identity for a file whose header (block 1) is destroyed:
        /// `PATH=file:N,ts:N[,rfile:N],bs:N`. Repeatable. Rows from such a
        /// file are never reported as verified.
        #[arg(long = "assume")]
        assume: Vec<String>,
    },
    /// Dictionary-less extraction (no SYSTEM file): group data blocks by
    /// data object id and write `OBJ_<objd>/inferred.csv` with `COL_n`
    /// columns. Column types are inferred per object and named in
    /// `_inference`; **no cell is ever verified** in this mode.
    Scavenge {
        /// Datafiles to scan (any; SYSTEM not needed).
        files: Vec<PathBuf>,
        /// Output directory (created).
        #[arg(long)]
        out: PathBuf,
        /// Database character set for TEXT inference (default AL32UTF8).
        #[arg(long)]
        charset: Option<String>,
        /// Only this data object id.
        #[arg(long)]
        objd: Option<u32>,
        /// Skip objects with fewer rows than this (and nothing rejected).
        #[arg(long, default_value_t = 1)]
        min_rows: usize,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("odr: error: {e:#}");
            ExitCode::from(1)
        }
    }
}

#[allow(clippy::too_many_lines)] // one dispatch per subcommand
fn run() -> Result<ExitCode> {
    let (file, block_size_arg, endian_arg, force, json) = match Cli::parse().cmd {
        Cmd::Diagnose {
            file,
            block_size,
            endian,
            force,
            fast,
            json,
            lang,
            list,
            html,
            db3f,
        } => {
            let order = endian.map(|e| match e {
                Endian::Little => ByteOrder::Little,
                Endian::Big => ByteOrder::Big,
            });
            let lang = match lang {
                LangArg::En => diagnose_cmd::Lang::En,
                LangArg::Ar => diagnose_cmd::Lang::Ar,
            };
            return diagnose_cmd::diagnose(
                &file,
                block_size,
                order,
                force,
                fast,
                json,
                lang,
                list,
                html.as_deref(),
                db3f.as_deref(),
            );
        }
        Cmd::Scan {
            file,
            block_size,
            endian,
            force,
            json,
        } => (file, block_size, endian, force, json),
        Cmd::Dict {
            files,
            json,
            assume,
        } => {
            let a = assume
                .iter()
                .map(|s| dict_cmd::Assume::parse(s))
                .collect::<Result<Vec<_>>>()?;
            return dict_cmd::dict(&files, json, &a);
        }
        Cmd::Extract {
            files,
            table,
            out,
            charset,
            assume,
        } => {
            let a = assume
                .iter()
                .map(|s| dict_cmd::Assume::parse(s))
                .collect::<Result<Vec<_>>>()?;
            return dict_cmd::extract(&files, &table, &out, charset.as_deref(), &a);
        }
        Cmd::Scavenge {
            files,
            out,
            charset,
            objd,
            min_rows,
        } => {
            return scavenge_cmd::scavenge(&files, &out, charset.as_deref(), objd, min_rows);
        }
    };
    if let Some(bs) = block_size_arg
        && !BLOCK_SIZES.contains(&bs)
    {
        bail!("invalid block size {bs}; expected one of {BLOCK_SIZES:?}");
    }
    let mut ev = Evidence::open(&file).with_context(|| format!("opening {}", file.display()))?;
    // Reference path: whole-file read. Astra replaces with streamed parallel scan.
    let len = usize::try_from(ev.len()).context("file too large for this build")?;
    let mut img = vec![0u8; len];
    ev.read_at(0, &mut img)?;
    let (block_size, order, source) =
        resolve_layout(&file, &img, block_size_arg, endian_arg, force)?;
    let report = scan_image(&img, block_size, order);
    let unchanged = ev.verify_unchanged()?;
    let sha = hex64(ev.sha256());
    let damaged = report
        .blocks
        .len()
        .saturating_sub(report.count(BlockStatus::Valid))
        .saturating_sub(report.count(BlockStatus::Zeroed));

    if json {
        let v = serde_json::json!({
            "file": file.display().to_string(), "sha256": sha, "unchanged": unchanged,
            "block_size": block_size, "byte_order": format!("{order:?}"), "layout_source": source,
            "blocks": report.blocks.len(),
            "valid": report.count(BlockStatus::Valid), "zeroed": report.count(BlockStatus::Zeroed),
            "damaged": damaged,
        });
        println!("{v}");
    } else {
        println!("file      {}", file.display());
        println!("sha256    {sha}  (unchanged after scan: {unchanged})");
        println!("layout    block size {block_size} · {order:?} ({source})");
        println!("blocks    {}", report.blocks.len());
        for s in [
            BlockStatus::Valid,
            BlockStatus::Zeroed,
            BlockStatus::ChecksumFail,
            BlockStatus::TailMismatch,
            BlockStatus::Foreign,
            BlockStatus::Truncated,
        ] {
            println!("  {:<13} {}", format!("{s:?}"), report.count(s));
        }
    }
    if !unchanged {
        bail!("evidence file changed during scan");
    }
    Ok(if damaged > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

/// Lower-case hex of a digest.
fn hex64(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Block size / byte order for `odr scan`: from the file's own header
/// (block 1, `kcvfh`), tried at every legal block size in both byte orders
/// — exactly one must parse and name its own size. Scanning with a wrong
/// size classifies every block as damaged: a wrong report, not a missing
/// one. Command-line values that disagree with the header are refused
/// unless `--force`; with an unreadable header `--force` is required.
/// Returns `(block_size, order, "header" | "forced" | …)`.
fn resolve_layout(
    file: &std::path::Path,
    img: &[u8],
    block_size_arg: Option<usize>,
    endian_arg: Option<Endian>,
    force: bool,
) -> Result<(usize, ByteOrder, &'static str)> {
    let to_order = |e: Endian| match e {
        Endian::Little => ByteOrder::Little,
        Endian::Big => ByteOrder::Big,
    };
    match (detect_header(img), block_size_arg, endian_arg) {
        (Some((hbs, ho)), bs, en) => {
            let bs = bs.unwrap_or(hbs);
            let o = en.map_or(ho, to_order);
            if (bs, o) == (hbs, ho) {
                Ok((bs, o, "header"))
            } else if force {
                Ok((bs, o, "forced"))
            } else {
                bail!(
                    "{}: header says block size {hbs} / {ho:?} but the command line says {bs} / {o:?}; pass --force to override the header",
                    file.display()
                )
            }
        }
        (None, Some(bs), en) if force => Ok((
            bs,
            to_order(en.unwrap_or(Endian::Little)),
            "forced (header unreadable)",
        )),
        (None, _, _) => bail!(
            "{}: block 1 is not an Oracle datafile header in any block size / byte order; pass --block-size and --endian with --force to scan anyway",
            file.display()
        ),
    }
}

/// Find the datafile header in block 1: for every legal block size, in both
/// byte orders, `kcvfh` must parse **and** declare that same block size.
/// Returns `None` unless exactly one `(size, order)` qualifies.
fn detect_header(img: &[u8]) -> Option<(usize, ByteOrder)> {
    let mut found = None;
    for bs in BLOCK_SIZES {
        let Some(block) = img.get(bs..bs.saturating_mul(2)) else {
            continue;
        };
        for order in [ByteOrder::Little, ByteOrder::Big] {
            if let Ok(h) = odr_core::dict::kcvfh::FileHeader::parse(block, order)
                && usize::try_from(h.block_size).ok() == Some(bs)
            {
                if found.is_some() {
                    return None;
                }
                found = Some((bs, order));
            }
        }
    }
    found
}
