//! `odr` — Oracle Datafile Recovery CLI. **Owner: Astra**.
//!
//! Output contract: human text by default, `--json` for machine output.
//! Exit codes: 0 ok · 1 usage/IO error · 2 completed with damaged blocks.

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

#[derive(Subcommand)]
enum Cmd {
    /// Classify every block of a datafile (read-only) and print a summary.
    Scan {
        /// Path to the datafile.
        file: PathBuf,
        /// Block size in bytes.
        #[arg(long, default_value_t = 8192)]
        block_size: usize,
        /// Byte order of the source platform.
        #[arg(long, value_enum, default_value_t = Endian::Little)]
        endian: Endian,
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

fn run() -> Result<ExitCode> {
    let (file, block_size, endian, json) = match Cli::parse().cmd {
        Cmd::Scan {
            file,
            block_size,
            endian,
            json,
        } => (file, block_size, endian, json),
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
    if !BLOCK_SIZES.contains(&block_size) {
        bail!("invalid block size {block_size}; expected one of {BLOCK_SIZES:?}");
    }
    let order = match endian {
        Endian::Little => ByteOrder::Little,
        Endian::Big => ByteOrder::Big,
    };
    let mut ev = Evidence::open(&file).with_context(|| format!("opening {}", file.display()))?;
    // Reference path: whole-file read. Astra replaces with streamed parallel scan.
    let len = usize::try_from(ev.len()).context("file too large for this build")?;
    let mut img = vec![0u8; len];
    ev.read_at(0, &mut img)?;
    let report = scan_image(&img, block_size, order);
    let unchanged = ev.verify_unchanged()?;
    let sha = ev
        .sha256()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        });
    let damaged = report
        .blocks
        .len()
        .saturating_sub(report.count(BlockStatus::Valid))
        .saturating_sub(report.count(BlockStatus::Zeroed));

    if json {
        let v = serde_json::json!({
            "file": file.display().to_string(), "sha256": sha, "unchanged": unchanged,
            "block_size": block_size, "blocks": report.blocks.len(),
            "valid": report.count(BlockStatus::Valid), "zeroed": report.count(BlockStatus::Zeroed),
            "damaged": damaged,
        });
        println!("{v}");
    } else {
        println!("file      {}", file.display());
        println!("sha256    {sha}  (unchanged after scan: {unchanged})");
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
