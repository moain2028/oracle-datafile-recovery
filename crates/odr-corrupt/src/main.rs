//! `odr-corrupt` — damage a COPY of a datafile deterministically.
//!
//! ```text
//! odr-corrupt --input golden/odr_assm01.dbf --output case/odr_assm01.dbf \
//!             --block-size 8192 --seed 7 --op bitrot:5 --op ransom:65536:65536:16
//! ```
//! Writes `<output>` and `<output>.manifest.json`. The input is opened
//! read-only; an existing output is never overwritten.

use anyhow::{Context, Result, bail};
use clap::Parser;
use odr_corrupt::{Op, Spec, apply};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "odr-corrupt",
    version,
    about = "Deterministic damage for COPIES of Oracle datafiles"
)]
struct Cli {
    /// Source datafile (read-only, never modified).
    #[arg(long)]
    input: PathBuf,
    /// Damaged copy to create (must not exist).
    #[arg(long)]
    output: PathBuf,
    /// Block size of the datafile.
    #[arg(long, default_value_t = 8192)]
    block_size: u64,
    /// Seed for every random choice.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Operation, repeatable, applied in order:
    /// bitrot:N · torn:N · zero:N · ransom:LEAD[:CHUNK:EVERY] · wipe-header ·
    /// truncate:BYTES · swap:N · holes:N:LEN · forge:N
    #[arg(long = "op", required = true)]
    ops: Vec<String>,
    /// Restrict block-granular ops to blocks FIRST:LAST (inclusive).
    #[arg(long)]
    blocks: Option<String>,
}

fn num(s: Option<&str>, what: &str) -> Result<u64> {
    let s = s.with_context(|| format!("missing {what}"))?;
    s.parse()
        .with_context(|| format!("{what}: '{s}' is not a number"))
}

fn parse_op(s: &str) -> Result<Op> {
    let mut p = s.split(':');
    let kind = p.next().unwrap_or_default();
    let op = match kind {
        "bitrot" => Op::Bitrot {
            count: num(p.next(), "count")?,
        },
        "torn" => Op::Torn {
            count: num(p.next(), "count")?,
        },
        "zero" => Op::Zero {
            count: num(p.next(), "count")?,
        },
        "ransom" => {
            let lead_bytes = num(p.next(), "lead bytes")?;
            let (chunk, every) = match p.next() {
                Some(c) => (num(Some(c), "chunk")?, num(p.next(), "every")?),
                None => (0, 0),
            };
            Op::Ransom {
                lead_bytes,
                chunk,
                every,
            }
        }
        "wipe-header" => Op::WipeHeader,
        "truncate" => Op::Truncate {
            keep_bytes: num(p.next(), "bytes")?,
        },
        "swap" => Op::Swap {
            count: num(p.next(), "count")?,
        },
        "holes" => Op::Holes {
            count: num(p.next(), "count")?,
            len: num(p.next(), "len")?,
        },
        "forge" => Op::Forge {
            count: num(p.next(), "count")?,
        },
        other => bail!("unknown op '{other}'"),
    };
    if p.next().is_some() {
        bail!("too many fields in '{s}'");
    }
    Ok(op)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let same = std::fs::canonicalize(&cli.input).ok() == std::fs::canonicalize(&cli.output).ok()
        && cli.output.exists();
    if same {
        bail!("output must differ from input (evidence is never modified)");
    }
    let blocks = match cli.blocks.as_deref() {
        None => None,
        Some(r) => {
            let (a, b) = r
                .split_once(':')
                .with_context(|| format!("--blocks '{r}' must be FIRST:LAST"))?;
            Some((num(Some(a), "first block")?, num(Some(b), "last block")?))
        }
    };
    let spec = Spec {
        block_size: cli.block_size,
        seed: cli.seed,
        ops: cli.ops.iter().map(|s| parse_op(s)).collect::<Result<_>>()?,
        blocks,
    };
    let input =
        std::fs::read(&cli.input).with_context(|| format!("reading {}", cli.input.display()))?;
    let (out, manifest) = apply(&input, &spec)?;
    let mpath = {
        let mut p = cli.output.clone().into_os_string();
        p.push(".manifest.json");
        PathBuf::from(p)
    };
    for p in [&cli.output, &mpath] {
        if p.exists() {
            bail!("{} already exists; refusing to overwrite", p.display());
        }
    }
    let create = |p: &PathBuf| OpenOptions::new().write(true).create_new(true).open(p);
    create(&cli.output)?.write_all(&out)?;
    create(&mpath)?.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    println!(
        "wrote {} ({} changes)\n  input  sha256 {}\n  output sha256 {}\n  manifest {}",
        cli.output.display(),
        manifest.changes.len(),
        manifest.input_sha256,
        manifest.output_sha256,
        mpath.display()
    );
    Ok(())
}
