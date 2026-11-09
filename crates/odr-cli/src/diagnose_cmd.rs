//! `odr diagnose` — detect, classify and explain damage in one datafile.
//!
//! Text output (English or Arabic) or `--json`. Exit codes: 0 healthy ·
//! 1 refused / I/O error · 2 damage found. The evidence file is opened
//! read-only and hashed before and during the scan (ADR-0002).

use anyhow::{Context, Result, bail};
use odr_core::ByteOrder;
use odr_core::diagnose::{self, Depth, LayoutSource, Options, ReadAt, Report, Severity};
use odr_core::source::Evidence;
use std::path::Path;
use std::process::ExitCode;

/// Report language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lang {
    /// English.
    En,
    /// Arabic.
    Ar,
}

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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Run the command.
#[allow(clippy::too_many_arguments)]
pub(crate) fn diagnose(
    file: &Path,
    block_size: Option<usize>,
    endian: Option<ByteOrder>,
    force: bool,
    fast: bool,
    json: bool,
    lang: Lang,
    list: usize,
) -> Result<ExitCode> {
    if let Some(bs) = block_size
        && !odr_core::block::BLOCK_SIZES.contains(&bs)
    {
        bail!(
            "invalid block size {bs}; expected one of {:?}",
            odr_core::block::BLOCK_SIZES
        );
    }
    let mut ev = Evidence::open(file).with_context(|| format!("opening {}", file.display()))?;
    let sha_before = hex(ev.sha256());
    let forced = match (force, block_size, endian) {
        (true, Some(bs), e) => Some((bs, e.unwrap_or(ByteOrder::Little))),
        (true, None, _) => bail!("--force requires --block-size"),
        _ => None,
    };
    let opts = Options {
        depth: if fast {
            Depth::Physical
        } else {
            Depth::Logical
        },
        ..Options::default()
    };
    let mut rd = EvidenceReader(&mut ev);
    let Some(report) = diagnose::diagnose(&mut rd, forced, opts)? else {
        bail!(
            "{}: not an Oracle datafile — block 1 is not a file header, block 0 carries no Oracle signature, and no block size / byte order explains the blocks; pass --block-size and --endian with --force to scan anyway",
            file.display()
        );
    };
    // A command-line value that disagrees with a decided header is refused
    // unless forced (same rule as `odr scan`, LV-073).
    if !force
        && let Some(bs) = block_size
        && bs != report.layout.block_size
    {
        bail!(
            "{}: the file says block size {} but the command line says {bs}; pass --force to override",
            file.display(),
            report.layout.block_size
        );
    }
    let unchanged = report.sha256_scanned == sha_before;
    if json {
        let mut v = serde_json::to_value(&report)?;
        if let Some(o) = v.as_object_mut() {
            o.insert("file".into(), file.display().to_string().into());
            o.insert("sha256_before".into(), sha_before.clone().into());
            o.insert("unchanged".into(), unchanged.into());
            o.insert(
                "tool".into(),
                format!("odr {}", env!("CARGO_PKG_VERSION")).into(),
            );
        }
        println!("{v}");
    } else {
        print_text(file, &report, &sha_before, unchanged, lang, list);
    }
    if !unchanged {
        bail!("evidence file changed during the scan");
    }
    Ok(if report.has_damage() {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

fn sev(s: Severity, lang: Lang) -> &'static str {
    match (s, lang) {
        (Severity::Critical, Lang::En) => "CRITICAL",
        (Severity::High, Lang::En) => "HIGH",
        (Severity::Medium, Lang::En) => "MEDIUM",
        (Severity::Low, Lang::En) => "LOW",
        (Severity::Info, Lang::En) => "INFO",
        (Severity::Critical, Lang::Ar) => "حرج",
        (Severity::High, Lang::Ar) => "عالٍ",
        (Severity::Medium, Lang::Ar) => "متوسط",
        (Severity::Low, Lang::Ar) => "منخفض",
        (Severity::Info, Lang::Ar) => "معلومة",
    }
}

fn source_name(s: LayoutSource, lang: Lang) -> &'static str {
    match (s, lang) {
        (LayoutSource::FileHeader, Lang::En) => "file header (block 1)",
        (LayoutSource::OsBlock, Lang::En) => "OS block 0 signature",
        (LayoutSource::Inferred, Lang::En) => "inferred from block signatures",
        (LayoutSource::Forced, Lang::En) => "forced by operator",
        (LayoutSource::FileHeader, Lang::Ar) => "رأس الملف (الكتلة 1)",
        (LayoutSource::OsBlock, Lang::Ar) => "توقيع الكتلة 0",
        (LayoutSource::Inferred, Lang::Ar) => "مستنتَج من توقيعات الكتل",
        (LayoutSource::Forced, Lang::Ar) => "مفروض من المشغّل",
    }
}

#[allow(clippy::too_many_lines)] // one report, two languages side by side
fn print_text(file: &Path, r: &Report, sha_before: &str, unchanged: bool, lang: Lang, list: usize) {
    let c = &r.counters;
    let bs = r.layout.block_size;
    let mib = |blocks: u64| -> String {
        let bytes = blocks.saturating_mul(u64::try_from(bs).unwrap_or(8192));
        #[allow(clippy::cast_precision_loss)]
        let f = bytes as f64 / 1_048_576.0;
        format!("{f:.1} MiB")
    };
    match lang {
        Lang::En => {
            println!("ODR diagnose — Oracle datafile damage report");
            println!("file        {}", file.display());
            println!("sha256      {sha_before}");
            println!("unchanged   {unchanged}  (hash recomputed from the scanned bytes)");
            println!(
                "layout      block size {bs} · {:?} · source: {}",
                r.layout.order,
                source_name(r.layout.source, lang)
            );
            if let Some(inf) = &r.layout.inference
                && let Some(best) = inf.candidates.first()
            {
                println!(
                    "inference   best {}/{:?} self-addressed {}/{} ({}‰){}",
                    best.block_size,
                    best.order,
                    best.self_addressed,
                    best.examined,
                    best.score_milli(),
                    if inf.accepted {
                        ""
                    } else {
                        " — not conclusive"
                    }
                );
            }
            println!(
                "size        {} bytes · {} blocks on disk{}{}",
                r.file_len,
                r.blocks_present.saturating_sub(1),
                r.layout
                    .declared_blocks_header
                    .map_or(String::new(), |d| format!(" · header declares {d}")),
                if r.last_block_partial {
                    " · last block PARTIAL"
                } else {
                    ""
                }
            );
            println!();
            println!("blocks (dbv classes)");
            println!("  examined    {}", c.examined);
            println!("  data        {}", c.data);
            println!("  index       {}", c.index);
            println!("  lob         {}", c.lob);
            println!("  seg         {}", c.seg);
            println!("  other       {}", c.other);
            println!("  empty       {}", c.empty);
            println!(
                "  damaged     {}  ({} of used blocks)",
                c.damaged_total(),
                permille(
                    c.damaged_total(),
                    c.used_clean().saturating_add(c.damaged_total())
                )
            );
            for (k, v) in &c.damaged {
                println!("    {k:<16}{v}");
            }
            if let Some(s) = c.highest_scn {
                println!("  highest SCN {s}");
            }
            println!();
            println!("findings ({})", r.findings.len());
            for f in &r.findings {
                println!(
                    "  [{}] {:?} — {} block(s), confidence {}‰",
                    sev(f.severity, lang),
                    f.code,
                    f.blocks,
                    f.confidence_milli
                );
                for e in &f.evidence {
                    println!("      evidence: {e}");
                }
                println!("      cause:    {}", f.cause_en);
                println!("      recovery: {}", f.recovery_en);
                if !f.oracle_errors.is_empty() {
                    println!("      oracle:   {}", f.oracle_errors.join(", "));
                }
            }
            if !r.regions.is_empty() {
                println!();
                println!(
                    "damage regions ({}; first {})",
                    r.regions.len(),
                    r.regions.len().min(50)
                );
                for g in r.regions.iter().take(50) {
                    println!(
                        "  blocks {:>8}..{:<8} {:>7} blk {:>10}  {}",
                        g.start,
                        g.end().saturating_sub(1),
                        g.len,
                        mib(g.len),
                        g.code
                    );
                }
            }
            if !r.objects.is_empty() {
                println!();
                println!("objects (DATA_OBJECT_ID → clean blocks / live rows / logical faults)");
                for (id, o) in &r.objects {
                    println!(
                        "  {id:<8} blocks {:>6} (data {} idx {} lob {})  rows {}  faults {}  range {}..{}",
                        o.blocks,
                        o.data_blocks,
                        o.index_blocks,
                        o.lob_blocks,
                        o.live_rows.map_or("-".to_owned(), |n| n.to_string()),
                        o.logical_faults,
                        o.first_block,
                        o.last_block
                    );
                }
                if r.unattributed_damaged > 0 {
                    println!(
                        "  (unattributed: {} damaged data-type blocks whose object id cannot be trusted)",
                        r.unattributed_damaged
                    );
                }
            }
            if list > 0 && !r.damaged_blocks.is_empty() {
                println!();
                println!(
                    "damaged blocks (first {} of {}{})",
                    list.min(r.damaged_blocks.len()),
                    r.damaged_blocks.len(),
                    if r.damaged_blocks_truncated { "+" } else { "" }
                );
                for b in r.damaged_blocks.iter().take(list) {
                    println!(
                        "  {:>10}  {:<16} {:?}{}",
                        b.index,
                        b.damage.code(),
                        b.kind,
                        b.type_byte
                            .map_or(String::new(), |t| format!("  type 0x{t:02x}"))
                    );
                }
            }
        }
        Lang::Ar => {
            println!("تشخيص ODR — تقرير تلف ملف بيانات أوراكل");
            println!("الملف          {}", file.display());
            println!("البصمة         {sha_before}");
            println!(
                "لم يتغيّر       {}  (أُعيد حساب البصمة من البايتات الممسوحة)",
                if unchanged { "نعم" } else { "لا" }
            );
            println!(
                "التخطيط        حجم الكتلة {bs} · {:?} · المصدر: {}",
                r.layout.order,
                source_name(r.layout.source, lang)
            );
            println!(
                "الحجم          {} بايت · {} كتلة على القرص{}{}",
                r.file_len,
                r.blocks_present.saturating_sub(1),
                r.layout
                    .declared_blocks_header
                    .map_or(String::new(), |d| format!(" · الرأس يعلن {d}")),
                if r.last_block_partial {
                    " · الكتلة الأخيرة ناقصة"
                } else {
                    ""
                }
            );
            println!();
            println!("الكتل (تصنيف dbv)");
            println!("  المفحوصة      {}", c.examined);
            println!("  بيانات        {}", c.data);
            println!("  فهارس         {}", c.index);
            println!("  LOB           {}", c.lob);
            println!("  رؤوس مقاطع    {}", c.seg);
            println!("  أخرى          {}", c.other);
            println!("  فارغة         {}", c.empty);
            println!(
                "  تالفة         {}  ({} من الكتل المستخدمة)",
                c.damaged_total(),
                permille(
                    c.damaged_total(),
                    c.used_clean().saturating_add(c.damaged_total())
                )
            );
            for (k, v) in &c.damaged {
                println!("    {:<16}{v}", damage_ar(k));
            }
            println!();
            println!("النتائج ({})", r.findings.len());
            for f in &r.findings {
                println!(
                    "  [{}] {:?} — {} كتلة، الثقة {}‰",
                    sev(f.severity, lang),
                    f.code,
                    f.blocks,
                    f.confidence_milli
                );
                for e in &f.evidence {
                    println!("      الدليل:    {e}");
                }
                println!("      السبب:     {}", f.cause_ar);
                println!("      الاسترجاع: {}", f.recovery_ar);
                if !f.oracle_errors.is_empty() {
                    println!("      أخطاء أوراكل: {}", f.oracle_errors.join("، "));
                }
            }
            if !r.regions.is_empty() {
                println!();
                println!(
                    "مناطق التلف ({}؛ أول {})",
                    r.regions.len(),
                    r.regions.len().min(50)
                );
                for g in r.regions.iter().take(50) {
                    println!(
                        "  الكتل {:>8}..{:<8} {:>7} كتلة {:>10}  {}",
                        g.start,
                        g.end().saturating_sub(1),
                        g.len,
                        mib(g.len),
                        damage_ar(&g.code)
                    );
                }
            }
            if !r.objects.is_empty() {
                println!();
                println!("الكائنات (DATA_OBJECT_ID ← كتل سليمة / صفوف حيّة / أخطاء منطقية)");
                for (id, o) in &r.objects {
                    println!(
                        "  {id:<8} كتل {:>6} (بيانات {} فهرس {} LOB {})  صفوف {}  أخطاء {}  المدى {}..{}",
                        o.blocks,
                        o.data_blocks,
                        o.index_blocks,
                        o.lob_blocks,
                        o.live_rows.map_or("-".to_owned(), |n| n.to_string()),
                        o.logical_faults,
                        o.first_block,
                        o.last_block
                    );
                }
            }
            if list > 0 && !r.damaged_blocks.is_empty() {
                println!();
                println!(
                    "الكتل التالفة (أول {} من {})",
                    list.min(r.damaged_blocks.len()),
                    r.damaged_blocks.len()
                );
                for b in r.damaged_blocks.iter().take(list) {
                    println!(
                        "  {:>10}  {:<16} {:?}",
                        b.index,
                        damage_ar(b.damage.code()),
                        b.kind
                    );
                }
            }
        }
    }
}

fn permille(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "0‰".to_owned();
    }
    format!(
        "{}‰",
        part.saturating_mul(1000).checked_div(whole).unwrap_or(0)
    )
}

fn damage_ar(code: &str) -> &'static str {
    match code {
        "truncated" => "مقطوعة",
        "zeroed" => "أصفار",
        "encrypted" => "مشفَّرة",
        "garbage" => "محتوى غريب",
        "marked_corrupt" => "موسومة تالفة",
        "checksum_fail" => "فشل checksum",
        "torn" => "كتابة مقطوعة",
        "misplaced" => "في غير مكانها",
        "logical" => "تلف منطقي",
        _ => "أخرى",
    }
}
