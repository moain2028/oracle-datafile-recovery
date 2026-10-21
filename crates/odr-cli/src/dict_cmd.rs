//! `odr dict` and `odr extract` — dictionary-driven, read-only extraction.
//!
//! Three output layers (REQ-004.3), kept apart to the file level:
//!
//! * `verified.csv`  — every cell decoded from bytes present in the row;
//! * `inferred.csv`  — at least one cell supplied by a documented rule
//!   (fast `ADD COLUMN … DEFAULT`), with the rule named in `_inference`;
//! * `rejected.csv`  — at least one cell that could not be trusted (invalid
//!   charset bytes, unsupported type, unknown) or a row that did not decode;
//!   the reason is in `_reason`. Never silently dropped.
//!
//! `report.json` records SHA-256 of every input (before and after), the
//! dictionary issues, per-layer counts and skipped blocks.

use anyhow::{Context, Result, bail};
use odr_core::ByteOrder;
use odr_core::block::Rdba;
use odr_core::charset::Charset;
use odr_core::dict::kcvfh::FileHeader;
use odr_core::dict::resolver::Dictionary;
use odr_core::dict::rows::{RowError, Skip, read_cluster, read_heap};
use odr_core::dict::schema::{Cell, TableDef, materialize};
use odr_core::dict::segment::walk_segment;
use odr_core::dict::source::{BlockAddr, BlockSource, FileInfo, FileSet};
use odr_core::source::Evidence;
use odr_core::value::{Charsets, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// One opened input with its header.
struct Opened {
    path: PathBuf,
    header: FileHeader,
    sha256: String,
    /// Header came from `--assume`, not from block 1 (LV-060).
    assumed: bool,
}

/// Operator-supplied identity for a file whose block 1 is destroyed
/// (`--assume PATH=file:FILE#,ts:TS#[,rfile:N],bs:BLOCK_SIZE`). Nothing is
/// inferred from the data blocks: the values are taken verbatim and written
/// to the report as *assumed*, and every row from such a file is reported
/// in the `inferred` layer at best (LV-060).
#[derive(Debug, Clone)]
pub(crate) struct Assume {
    pub path: PathBuf,
    pub file: u32,
    pub ts: u32,
    pub rfile: Option<u32>,
    pub block_size: usize,
}

impl Assume {
    pub(crate) fn parse(spec: &str) -> Result<Self> {
        let (path, kv) = spec
            .split_once('=')
            .context("--assume wants PATH=file:N,ts:N[,rfile:N],bs:N")?;
        let (mut file, mut ts, mut rfile, mut bs) = (None, None, None, None);
        for part in kv.split(',') {
            let (k, v) = part
                .split_once(':')
                .context("--assume: key:value expected")?;
            let n: u32 = v
                .trim()
                .parse()
                .with_context(|| format!("--assume {k}: {v}"))?;
            match k.trim() {
                "file" => file = Some(n),
                "ts" => ts = Some(n),
                "rfile" => rfile = Some(n),
                "bs" => bs = Some(usize::try_from(n)?),
                other => bail!("--assume: unknown key {other}"),
            }
        }
        let block_size = bs.context("--assume: bs missing")?;
        if !odr_core::block::BLOCK_SIZES.contains(&block_size) {
            bail!("--assume: bs {block_size} is not an Oracle block size");
        }
        Ok(Self {
            path: PathBuf::from(path),
            file: file.context("--assume: file missing")?,
            ts: ts.context("--assume: ts missing")?,
            rfile,
            block_size,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Open every file, parse `kcvfh`, register in a `FileSet`. All files must
/// share one byte order (one database). Returns the set and the per-file
/// metadata; the SYSTEM file is whichever one yields `bootstrap$` rows.
fn open_all(paths: &[PathBuf], assume: &[Assume]) -> Result<(FileSet, Vec<Opened>)> {
    if paths.is_empty() {
        bail!("no datafiles given");
    }
    let mut order: Option<ByteOrder> = None;
    let mut opened = Vec::new();
    let mut evs = Vec::new();
    for p in paths {
        let mut ev = Evidence::open(p).with_context(|| format!("opening {}", p.display()))?;
        if let Some(a) = assume.iter().find(|a| &a.path == p) {
            // LV-060: block 1 is not consulted at all; the operator's word
            // is recorded and the byte order is taken from the other files
            // (or little-endian when this is the only one — reported).
            let h = FileHeader {
                order: order.unwrap_or(ByteOrder::Little),
                compat: 0,
                dbid: 0,
                db_name: String::new(),
                file_size_blocks: u64::try_from(a.block_size)
                    .ok()
                    .and_then(|bs| ev.len().checked_div(bs))
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0),
                block_size: u32::try_from(a.block_size)?,
                file_number: u16::try_from(a.file)?,
                file_type: 0,
                root_dba: Rdba(0),
                creation_scn: odr_core::block::Scn(0),
                status: 0,
                checkpoint_count: 0,
                tablespace: Some(a.ts),
                tablespace_name: None,
                relative_file: a.rfile,
                checkpoint_scn: None,
            };
            opened.push(Opened {
                path: p.clone(),
                sha256: hex(ev.sha256()),
                header: h,
                assumed: true,
            });
            evs.push(ev);
            continue;
        }
        // kcvfh lives in block 1; the block size is inside it, so read the
        // largest possible block and let the parser bound itself.
        let mut buf = vec![0u8; 32 * 1024 * 2];
        let n = ev.read_at(0, &mut buf)?;
        buf.truncate(n);
        // Try every supported block size for block 1.
        let mut header = None;
        for bs in odr_core::block::BLOCK_SIZES {
            let Some(b1) = buf.get(bs..bs.saturating_mul(2)) else {
                continue;
            };
            if let Ok(h) = FileHeader::detect(b1)
                && usize::try_from(h.block_size).ok() == Some(bs)
            {
                header = Some(h);
                break;
            }
        }
        let Some(h) = header else {
            bail!("{}: block 1 is not an Oracle datafile header", p.display());
        };
        match order {
            None => order = Some(h.order),
            Some(o) if o != h.order => {
                bail!("{}: byte order differs from the other files", p.display());
            }
            _ => {}
        }
        opened.push(Opened {
            path: p.clone(),
            sha256: hex(ev.sha256()),
            header: h,
            assumed: false,
        });
        evs.push(ev);
    }
    let order = order.context("no files")?;
    // An assumed file registered before a real one took the default order.
    for o in &mut opened {
        if o.assumed {
            o.header.order = order;
        }
    }
    let mut set = FileSet::new(order, 4096);
    for (o, ev) in opened.iter().zip(evs) {
        set.add(FileInfo::from_header(&o.header), ev);
    }
    Ok((set, opened))
}

/// Find the SYSTEM file: the one whose root segment decodes `bootstrap$`.
fn load_dictionary(set: &FileSet, opened: &[Opened]) -> Result<(Dictionary, u32)> {
    let mut last_err = None;
    for o in opened {
        let f = u32::from(o.header.file_number);
        if !o.header.has_dictionary_root() {
            continue;
        }
        match Dictionary::load(set, f) {
            Ok(d) => return Ok((d, f)),
            Err(e) => last_err = Some((o.path.clone(), e)),
        }
    }
    match last_err {
        Some((p, e)) => bail!(
            "no SYSTEM datafile among the inputs (last tried {}: {e})",
            p.display()
        ),
        None => bail!("no SYSTEM datafile among the inputs (no file has a dictionary root)"),
    }
}

/// `odr dict`.
#[allow(clippy::too_many_lines)] // one report; the JSON and text shapes are kept side by side
pub(crate) fn dict(
    files: &[PathBuf],
    json: bool,
    assume: &[Assume],
) -> Result<std::process::ExitCode> {
    let (set, opened) = open_all(files, assume)?;
    let (d, sysf) = load_dictionary(&set, &opened)?;
    let tables = d.tables();
    let (props, _) = d.properties(&set, sysf);
    if json {
        let files: Vec<_> = opened
            .iter()
            .map(|o| {
                serde_json::json!({
                    "path": o.path.display().to_string(), "sha256": o.sha256,
                    "file": o.header.file_number, "relative_file": o.header.relative_file,
                    "tablespace": o.header.tablespace, "tablespace_name": o.header.tablespace_name,
                    "block_size": o.header.block_size, "blocks": o.header.file_size_blocks,
                    "order": format!("{:?}", o.header.order), "db_name": o.header.db_name,
                    "system": u32::from(o.header.file_number) == sysf,
                    "assumed": o.assumed,
                })
            })
            .collect();
        let tabs: Vec<_> = tables
            .iter()
            .map(|t| {
                serde_json::json!({
                    "owner": t.owner_name, "owner_id": t.owner, "name": t.name, "obj": t.obj,
                    "dataobj": t.dataobj, "tablespace": d.tablespace_name(t.ts), "ts": t.ts,
                    "file": t.file, "block": t.block, "clustered": t.is_clustered(),
                    "iot": t.is_iot(), "row_dependencies": t.has_row_dependencies(),
                    "columns": t.visible_columns().iter().map(|c| serde_json::json!({
                        "name": c.name, "type": format!("{:?}", c.column_type()), "type_code": c.type_code,
                        "length": c.length, "precision": c.precision, "scale": c.scale,
                        "not_null": c.not_null, "segcol": c.segcol,
                        "fast_add_default": c.is_fast_add_default(), "default": c.default_text,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let v = serde_json::json!({
            "files": files,
            "bootstrap_version": d.catalog.version,
            "nls": {"NLS_CHARACTERSET": props.get("NLS_CHARACTERSET"), "NLS_NCHAR_CHARACTERSET": props.get("NLS_NCHAR_CHARACTERSET"), "NLS_RDBMS_VERSION": props.get("NLS_RDBMS_VERSION")},
            "objects": d.objects.len(), "users": d.users, "tablespaces": d.tablespaces,
            "tables": tabs, "issues": d.issues,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        for o in &opened {
            println!(
                "file {:>4}  rfile {:<5} ts {:<3} {:<16} bs {:<5} blocks {:<8} {}{}",
                o.header.file_number,
                o.header
                    .relative_file
                    .map_or("big".to_owned(), |r| r.to_string()),
                o.header
                    .tablespace
                    .map_or("-".to_owned(), |t| t.to_string()),
                o.header.tablespace_name.clone().unwrap_or_default(),
                o.header.block_size,
                o.header.file_size_blocks,
                o.path.display(),
                if u32::from(o.header.file_number) == sysf {
                    "  [SYSTEM]"
                } else {
                    ""
                }
            );
        }
        let prop = |k: &str| {
            props
                .get(k)
                .and_then(Option::as_deref)
                .unwrap_or("?")
                .to_owned()
        };
        println!(
            "nls: charset {} · nchar {} · rdbms {}",
            prop("NLS_CHARACTERSET"),
            prop("NLS_NCHAR_CHARACTERSET"),
            prop("NLS_RDBMS_VERSION")
        );
        println!(
            "dictionary: bootstrap {} · {} objects · {} tables · {} users · {} tablespaces · {} issues",
            d.catalog.version.clone().unwrap_or_default(),
            d.objects.len(),
            tables.len(),
            d.users.len(),
            d.tablespaces.len(),
            d.issues.len()
        );
        let mut by_owner: BTreeMap<String, Vec<&TableDef>> = BTreeMap::new();
        for t in &tables {
            by_owner
                .entry(
                    t.owner_name
                        .clone()
                        .unwrap_or_else(|| format!("#{}", t.owner)),
                )
                .or_default()
                .push(t);
        }
        for (owner, ts) in &by_owner {
            if owner == "SYS" || owner == "SYSTEM" {
                println!(
                    "{owner}: {} tables (dictionary; use --json to list)",
                    ts.len()
                );
                continue;
            }
            for t in ts {
                println!(
                    "  {owner}.{:<30} obj {:<7} ts {:<12} {}{}{}  {} cols",
                    t.name,
                    t.obj,
                    d.tablespace_name(t.ts).unwrap_or("?"),
                    if t.is_clustered() { "cluster " } else { "" },
                    if t.is_iot() { "iot " } else { "" },
                    if t.has_row_dependencies() {
                        "rowdep "
                    } else {
                        ""
                    },
                    t.visible_columns().len()
                );
            }
        }
        for i in d.issues.iter().take(10) {
            println!("issue: {i:?}");
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// CSV-escape a field.
fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// Where a decoded row goes.
enum Layer {
    Verified,
    Inferred(String),
    Rejected(String),
}

fn classify(
    cells: &[Cell],
    block_status: odr_core::block::BlockStatus,
    structural: Option<odr_core::row::StructuralFault>,
) -> Layer {
    // LV-059: a row from a block that fails its checksum / tail check can
    // decode perfectly and still hold altered bytes (bitrot lab: 3 rows of
    // 20,000 with one wrong character). It is never *verified*.
    if block_status != odr_core::block::BlockStatus::Valid {
        return Layer::Rejected(format!("block integrity: {block_status:?}"));
    }
    // LV-062: a valid checksum is not proof of content; a block violating
    // a structural invariant (forge lab) is rejected whole.
    if let Some(f) = structural {
        return Layer::Rejected(format!("block structure: {f}"));
    }
    let mut inferred = Vec::new();
    for (i, c) in (1usize..).zip(cells) {
        match c {
            Cell::Verified(v) => {
                if !v.is_exact() {
                    return Layer::Rejected(format!(
                        "column {i} not exact: {}",
                        match v {
                            Value::Text { invalid, .. } =>
                                format!("{invalid} invalid byte sequences"),
                            Value::Invalid { .. } => "invalid encoding".to_owned(),
                            Value::Unsupported { .. } => "unsupported type".to_owned(),
                            _ => "not exact".to_owned(),
                        }
                    ));
                }
            }
            Cell::Inferred { source, .. } => inferred.push(format!("column {i}: {source:?}")),
            Cell::Unknown { reason } => return Layer::Rejected(format!("column {i}: {reason}")),
            _ => return Layer::Rejected(format!("column {i}: unrecognised cell")),
        }
    }
    if inferred.is_empty() {
        Layer::Verified
    } else {
        Layer::Inferred(inferred.join("; "))
    }
}

/// `odr extract`.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
pub(crate) fn extract(
    files: &[PathBuf],
    table: &str,
    out: &Path,
    charset: Option<&str>,
    assume: &[Assume],
) -> Result<std::process::ExitCode> {
    let Some((owner, name)) = table.split_once('.') else {
        bail!("--table must be OWNER.NAME");
    };
    let (set, opened) = open_all(files, assume)?;
    let (d, sysf) = load_dictionary(&set, &opened)?;
    let Some(t) = d.table(&owner.to_ascii_uppercase(), &name.to_ascii_uppercase()) else {
        bail!("table {table} not found in the dictionary (try `odr dict` to list)");
    };
    if t.is_iot() {
        bail!("{table} is index-organised; not supported yet");
    }
    // Character sets come from SYS.PROPS$ (LV-056); `--charset` overrides.
    // Every text cell is still checked for exact decodability; anything that
    // is not exact goes to `rejected`.
    let (props, prop_issues) = d.properties(&set, sysf);
    let from_props = |key: &str| -> Result<Option<Charset>> {
        match props.get(key).and_then(Option::as_deref) {
            None => Ok(None),
            Some(n) => Charset::from_oracle_name(n)
                .map(Some)
                .with_context(|| format!("PROPS$ {key} = {n}: character set not supported by ODR")),
        }
    };
    let database = match charset {
        Some(n) => {
            Charset::from_oracle_name(n).with_context(|| format!("unknown character set {n}"))?
        }
        None => from_props("NLS_CHARACTERSET")?
            .context("NLS_CHARACTERSET not readable from PROPS$; pass --charset explicitly")?,
    };
    let national = from_props("NLS_NCHAR_CHARACTERSET")?.unwrap_or(Charset::Al16Utf16);
    let cs = Charsets { database, national };
    // Which input file holds a segment: FILE# is relative within TS# (LV-055).
    let find_file = |ts: u32, rfile: u32| -> Option<u32> {
        opened
            .iter()
            .find(|o| {
                o.header.tablespace == Some(ts)
                    && (o.header.relative_file == Some(rfile)
                        || (o.header.relative_file.is_none() && rfile == 1024))
            })
            .map(|o| u32::from(o.header.file_number))
    };
    let missing = |ts: u32, rfile: u32| {
        anyhow::anyhow!(
            "the datafile holding {table} (tablespace {} rfile {rfile}) was not given",
            d.tablespace_name(ts).unwrap_or("?")
        )
    };
    let (hdr_file, hdr_block, cluster, expect_objd) = if t.is_clustered() {
        let clu = d
            .cluster_of(&t)
            .context("clustered table whose cluster has no CLU$ row")?;
        if clu.hashkeys != 0 {
            bail!("{table} is in a HASH cluster; only indexed clusters are supported");
        }
        let tab = usize::try_from(t.tab.context("clustered table without TAB#")?)?;
        let clucols = usize::try_from(t.clucols.unwrap_or(clu.cols))?;
        let f = find_file(clu.ts, clu.file).ok_or_else(|| missing(clu.ts, clu.file))?;
        (f, clu.block, Some((tab, clucols)), clu.dataobj)
    } else {
        let f = find_file(t.ts, t.file).ok_or_else(|| missing(t.ts, t.file))?;
        (f, t.block, None::<(usize, usize)>, t.dataobj)
    };
    let hdr = set
        .block(BlockAddr {
            file: hdr_file,
            block: hdr_block,
        })
        .context("segment header block unreadable")?;
    let seg = walk_segment(&hdr, set.order(), Rdba(0), |r| {
        set.resolve(r, hdr_file).and_then(|a| set.block(a))
    })
    .context("walking the segment")?;
    if Some(seg.objd) != expect_objd {
        bail!(
            "segment header says data object {} but the dictionary says {expect_objd:?}: refusing (truncated/moved table?)",
            seg.objd
        );
    }
    let rows = match cluster {
        None => read_heap(&set, &seg, hdr_file, seg.objd),
        Some((tab, clucols)) => read_cluster(&set, &seg, hdr_file, seg.objd, tab, clucols),
    };

    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let visible = t.visible_columns();
    let mut header_line = String::from("_file,_block,_slot");
    for c in &visible {
        header_line.push(',');
        header_line.push_str(&csv(&c.name));
    }
    let mut w_ver = std::io::BufWriter::new(std::fs::File::create(out.join("verified.csv"))?);
    let mut w_inf = std::io::BufWriter::new(std::fs::File::create(out.join("inferred.csv"))?);
    let mut w_rej = std::io::BufWriter::new(std::fs::File::create(out.join("rejected.csv"))?);
    writeln!(w_ver, "{header_line}")?;
    writeln!(w_inf, "{header_line},_inference")?;
    writeln!(w_rej, "{header_line},_reason")?;
    let file_assumed = opened
        .iter()
        .any(|o| o.assumed && u32::from(o.header.file_number) == hdr_file);
    let (mut n_ver, mut n_inf, mut n_rej) = (0usize, 0usize, 0usize);
    let scheme = set
        .scheme(hdr_file)
        .context("segment file not registered")?;
    for r in &rows.rows {
        match r {
            Err((prov, e)) => {
                let (_, b) = Rdba(prov.rdba).split(scheme);
                // LV-061: a slot of a damaged block whose flags could not be
                // trusted is accounted for under the integrity reason.
                let why = match e {
                    RowError::Integrity(st) => format!("block integrity: {st:?}"),
                    RowError::Structural(f) => format!("block structure: {f}"),
                    other => format!("row did not decode: {other:?}"),
                };
                writeln!(
                    w_rej,
                    "{},{b},{}{},{}",
                    prov.file,
                    prov.slot,
                    ",".repeat(visible.len()),
                    csv(&why)
                )?;
                n_rej = n_rej.saturating_add(1);
            }
            Ok(r) => {
                let stored: Vec<Option<&[u8]>> = r.columns.iter().map(|c| c.as_deref()).collect();
                let cells = materialize(&t, &stored, cs);
                let (_, b) = Rdba(r.provenance.rdba).split(scheme);
                let mut line = format!("{},{b},{}", r.provenance.file, r.rowid_row);
                for c in &cells {
                    line.push(',');
                    if let Some(v) = c.canonical() {
                        line.push_str(&csv(&v));
                    }
                }
                let layer = match classify(&cells, r.block_status, r.structural) {
                    // LV-060: identity of the file was asserted, not read.
                    Layer::Verified if file_assumed => {
                        Layer::Inferred("file identity from --assume (header destroyed)".into())
                    }
                    l => l,
                };
                match layer {
                    Layer::Verified => {
                        writeln!(w_ver, "{line}")?;
                        n_ver = n_ver.saturating_add(1);
                    }
                    Layer::Inferred(why) => {
                        writeln!(w_inf, "{line},{}", csv(&why))?;
                        n_inf = n_inf.saturating_add(1);
                    }
                    Layer::Rejected(why) => {
                        writeln!(w_rej, "{line},{}", csv(&why))?;
                        n_rej = n_rej.saturating_add(1);
                    }
                }
            }
        }
    }
    w_ver.flush()?;
    w_inf.flush()?;
    w_rej.flush()?;
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    for (_, s) in &rows.skipped {
        let k = match s {
            Skip::Unavailable => "unavailable".to_owned(),
            Skip::NotDataBlock(t) => format!("not a data block (type {t:#04x})"),
            Skip::ForeignObject(o) => format!("foreign object {o}"),
            Skip::Malformed(e) => format!("malformed: {e}"),
            Skip::Integrity(st) => format!("integrity: {st:?}"),
            Skip::AboveHighWater => "above high-water mark".to_owned(),
            _ => "other".to_owned(),
        };
        let n = skipped.entry(k).or_default();
        *n = n.saturating_add(1);
    }
    let report = serde_json::json!({
        "table": format!("{}.{}", t.owner_name.clone().unwrap_or_default(), t.name),
        "obj": t.obj, "dataobj": t.dataobj, "tablespace": d.tablespace_name(t.ts),
        "segment": {"file": hdr_file, "block": hdr_block, "blocks": seg.len(), "extents": seg.extents.len(), "issues": seg.issues.iter().map(|i| format!("{i:?}")).collect::<Vec<_>>()},
        "data_blocks": rows.data_blocks,
        "rows": {"verified": n_ver, "inferred": n_inf, "rejected": n_rej},
        "skipped_blocks": skipped,
        "columns": visible.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        "database_charset": format!("{database:?}"), "national_charset": format!("{national:?}"),
        "props": {"NLS_CHARACTERSET": props.get("NLS_CHARACTERSET"), "NLS_NCHAR_CHARACTERSET": props.get("NLS_NCHAR_CHARACTERSET"), "issues": prop_issues},
        "inputs": opened.iter().map(|o| serde_json::json!({"path": o.path.display().to_string(), "sha256": o.sha256, "file": o.header.file_number, "assumed": o.assumed})).collect::<Vec<_>>(),
        "dictionary_issues": d.issues,
    });
    std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!(
        "{table}: verified {n_ver} · inferred {n_inf} · rejected {n_rej} · data blocks {} · skipped {} → {}",
        rows.data_blocks,
        rows.skipped.len(),
        out.display()
    );
    Ok(if n_rej > 0 {
        std::process::ExitCode::from(2)
    } else {
        std::process::ExitCode::SUCCESS
    })
}
