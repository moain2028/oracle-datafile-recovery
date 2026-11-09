//! From per-block verdicts to *explanations*: what happened to this file,
//! how sure we are, what evidence says so, and what can still be recovered.
//!
//! Every rule is deterministic and cites the numbers it used. Confidence is
//! a per-mille integer (no floats in the report) so two runs on the same
//! bytes produce identical output.

use super::{Region, Report};
use serde::{Deserialize, Serialize};

/// How bad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Nothing wrong, or purely informational.
    Info,
    /// Isolated damage; most data recoverable.
    Low,
    /// Noticeable damage.
    Medium,
    /// Large or structurally important damage.
    High,
    /// File header / large ransomware region / ≥ 10 % of used blocks.
    Critical,
}

/// Stable finding codes (tests and the Windows app key on these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum FindingCode {
    /// No damage at all.
    Healthy,
    /// Block 1 is not a `kcvfh`.
    HeaderDestroyed,
    /// Fewer blocks than declared, or a partial last block.
    Truncated,
    /// A high-entropy region starting at block 0/1.
    RansomwareHead,
    /// ≥ 4 damaged regions at a constant stride.
    RansomwarePeriodic,
    /// Encrypted blocks without a pattern.
    EncryptedScattered,
    /// Zeroed regions inside the used range.
    ZeroedRegions,
    /// Isolated checksum failures.
    BitRot,
    /// Torn / fractured writes.
    TornWrites,
    /// Blocks that name another position.
    MisdirectedWrites,
    /// Clean checksums, broken internals.
    LogicalCorruption,
    /// `seq_kcbh = 0xFF`.
    MarkedCorrupt,
    /// Low-entropy foreign bytes.
    GarbageOverwrite,
    /// More blocks on disk than the header declares (grown file or wrong
    /// header) — informational.
    SizeMismatch,
}

/// One interpreted finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Code.
    pub code: FindingCode,
    /// Severity.
    pub severity: Severity,
    /// Confidence, per mille.
    pub confidence_milli: u16,
    /// Blocks involved.
    pub blocks: u64,
    /// Measured facts this finding rests on (stable wording, English).
    pub evidence: Vec<String>,
    /// Likely cause, English.
    pub cause_en: String,
    /// Likely cause, Arabic.
    pub cause_ar: String,
    /// What is still recoverable and how, English.
    pub recovery_en: String,
    /// Arabic.
    pub recovery_ar: String,
    /// Oracle errors a running instance would have raised on these blocks.
    pub oracle_errors: Vec<String>,
}

/// A span of consecutive damaged blocks regardless of per-block code, with
/// the per-code tally inside it. Random bytes parse as an Oracle header
/// about 1 time in 16 (format nibble `2`), so an encrypted span is
/// *interleaved* with `checksum_fail` / `torn` verdicts; merging first and
/// voting after is what makes the span visible as one event.
#[derive(Debug, Clone)]
struct Span {
    start: u64,
    len: u64,
    encrypted: u64,
    zeroed: u64,
}

impl Span {
    const fn end(&self) -> u64 {
        self.start.saturating_add(self.len)
    }
    /// Encrypted blocks are ≥ 60 % of the span.
    fn encrypted_dominated(&self) -> bool {
        self.encrypted.saturating_mul(10) >= self.len.saturating_mul(6)
    }
}

fn spans(regions: &[Region]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for g in regions {
        let enc = u64::from(g.code == "encrypted").saturating_mul(g.len);
        let zer = u64::from(g.code == "zeroed" || g.code == "header_zeroed").saturating_mul(g.len);
        match out.last_mut() {
            Some(s) if s.end() == g.start => {
                s.len = s.len.saturating_add(g.len);
                s.encrypted = s.encrypted.saturating_add(enc);
                s.zeroed = s.zeroed.saturating_add(zer);
            }
            _ => out.push(Span {
                start: g.start,
                len: g.len,
                encrypted: enc,
                zeroed: zer,
            }),
        }
    }
    out
}

fn pct_milli(part: u64, whole: u64) -> u64 {
    part.saturating_mul(1000).checked_div(whole).unwrap_or(0)
}

/// Severity from the share of *used* blocks affected, with a floor.
fn severity_by_share(blocks: u64, used: u64, floor: Severity) -> Severity {
    let share = pct_milli(blocks, used.max(1));
    let s = if share >= 100 {
        Severity::Critical
    } else if share >= 10 {
        Severity::High
    } else if share >= 1 {
        Severity::Medium
    } else {
        Severity::Low
    };
    s.max(floor)
}

/// Detect a constant stride among span starts (ransomware that encrypts
/// every k-th chunk). Returns `(stride_blocks, matching_spans)` when at
/// least four spans are in phase within ±1 block.
fn periodic(starts: &[u64]) -> Option<(u64, u64)> {
    if starts.len() < 4 {
        return None;
    }
    let gaps: Vec<u64> = starts
        .windows(2)
        .filter_map(|w| match w {
            [a, b] => Some(b.saturating_sub(*a)),
            _ => None,
        })
        .collect();
    let mut best: Option<(u64, u64)> = None;
    for &g in &gaps {
        if g == 0 {
            continue;
        }
        let n = gaps.iter().filter(|&&x| x.abs_diff(g) <= 1).count();
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        if n64 >= 3 && best.is_none_or(|(_, bn)| n64 > bn) {
            best = Some((g, n64.saturating_add(1)));
        }
    }
    best.filter(|&(_, n)| n >= 4)
}

/// Build findings from a finished report (most severe first).
#[allow(clippy::too_many_lines)] // one rule per block; the table in docs/design/diagnose.md §3.3
pub fn findings(r: &Report) -> Vec<Finding> {
    let mut out = Vec::new();
    let c = &r.counters;
    let used = c.used_clean().saturating_add(c.damaged_total());
    let bs = u64::try_from(r.layout.block_size).unwrap_or(8192);
    let by = |code: &str| -> Vec<&Region> { r.regions.iter().filter(|g| g.code == code).collect() };
    let all_spans = spans(&r.regions);
    // Blocks inside encrypted-dominated spans are *explained* by the
    // encryption event; they must not surface again as bit rot / torn.
    let enc_spans: Vec<&Span> = all_spans
        .iter()
        .filter(|s| s.encrypted_dominated())
        .collect();
    let absorbed = |code: &str| -> u64 {
        r.regions
            .iter()
            .filter(|g| g.code == code)
            .filter(|g| {
                enc_spans
                    .iter()
                    .any(|s| g.start >= s.start && g.end() <= s.end())
            })
            .map(|g| g.len)
            .fold(0, u64::saturating_add)
    };
    let count = |code: &str| {
        c.damaged
            .get(code)
            .copied()
            .unwrap_or(0)
            .saturating_sub(absorbed(code))
    };

    // --- Header ---------------------------------------------------------
    if r.layout.source != super::LayoutSource::FileHeader
        && r.layout.source != super::LayoutSource::Forced
    {
        let first = r.damaged_blocks.iter().find(|b| b.index == 1);
        let mut ev = vec![format!(
            "block 1 is not a datafile header (kcvfh); layout decided by {:?}",
            r.layout.source
        )];
        if let Some(b) = first {
            ev.push(format!("block 1 verdict: {}", b.damage.code()));
        }
        out.push(Finding {
            code: FindingCode::HeaderDestroyed,
            severity: Severity::Critical,
            confidence_milli: 1000,
            blocks: 1,
            evidence: ev,
            cause_en: "The file header (block 1) was overwritten, encrypted or zeroed. Ransomware that encrypts the first megabytes of every file is the most common cause; a wiped header is the second.".into(),
            cause_ar: "رأس الملف (الكتلة 1) كُتب فوقه أو شُفِّر أو صُفِّر. أشهر سبب: فدية تشفّر أول ميغابايتات من كل ملف؛ والثاني مسح الرأس.".into(),
            recovery_en: "Block size and byte order were inferred from the blocks themselves. Rows can still be extracted, but the file's identity (file#, tablespace) must be supplied with --assume and every row from it is reported as inferred, never verified.".into(),
            recovery_ar: "حجم الكتلة وترتيب البايتات استُنتجا من الكتل نفسها. يمكن استخراج الصفوف، لكن هوية الملف (رقمه ومساحته) تُعطى بـ --assume وكل صف منه يُبلَّغ مستنتَجاً لا موثَّقاً.".into(),
            oracle_errors: vec!["ORA-01122".into(), "ORA-01110".into(), "ORA-01210".into()],
        });
    }

    // --- Size -----------------------------------------------------------
    let declared = r
        .layout
        .declared_blocks_header
        .or(r.layout.declared_blocks_os)
        .map(u64::from);
    let on_disk = r.blocks_present.saturating_sub(1); // exclude OS block
    if let Some(d) = declared {
        if on_disk < d || r.last_block_partial {
            let missing = d.saturating_sub(on_disk);
            out.push(Finding {
                code: FindingCode::Truncated,
                severity: if missing > 0 && pct_milli(missing, d) >= 100 {
                    Severity::Critical
                } else {
                    Severity::High
                },
                confidence_milli: 1000,
                blocks: missing.max(u64::from(r.last_block_partial)),
                evidence: vec![
                    format!("header declares {d} blocks, {on_disk} present on disk"),
                    format!(
                        "file length {} bytes = {} full blocks{}",
                        r.file_len,
                        r.file_len.checked_div(bs).unwrap_or(0),
                        if r.last_block_partial { " + a partial block" } else { "" }
                    ),
                ],
                cause_en: "The file was cut short: an interrupted copy, a filesystem that lost its tail, or a backup that stopped early.".into(),
                cause_ar: "الملف مقطوع: نسخ متقطع، أو نظام ملفات فقد ذيله، أو نسخة احتياطية توقفت مبكراً.".into(),
                recovery_en: "Everything beyond the end is gone unless another copy exists. Blocks before the cut are judged on their own merits.".into(),
                recovery_ar: "كل ما بعد النهاية ضائع ما لم توجد نسخة أخرى. الكتل قبل القطع تُحكم كلٌّ بدليلها.".into(),
                oracle_errors: vec!["ORA-27046".into(), "ORA-01122".into(), "ORA-01110".into()],
            });
        } else if on_disk > d {
            out.push(Finding {
                code: FindingCode::SizeMismatch,
                severity: Severity::Info,
                confidence_milli: 800,
                blocks: on_disk.saturating_sub(d),
                evidence: vec![format!("header declares {d} blocks, {on_disk} present on disk")],
                cause_en: "More blocks on disk than the header declares: the file grew after this header image was written, or the header is stale.".into(),
                cause_ar: "كتل على القرص أكثر مما يعلنه الرأس: الملف نما بعد كتابة هذا الرأس، أو الرأس قديم.".into(),
                recovery_en: "Informational. Extra blocks are scanned like any other.".into(),
                recovery_ar: "معلومة فقط. الكتل الزائدة تُمسح كغيرها.".into(),
                oracle_errors: vec![],
            });
        }
    }

    // --- Encrypted ------------------------------------------------------
    let n_enc_total = c.damaged.get("encrypted").copied().unwrap_or(0);
    if n_enc_total > 0 {
        let head = enc_spans.iter().find(|s| s.start <= 1);
        if let Some(h) = head {
            out.push(Finding {
                code: FindingCode::RansomwareHead,
                severity: Severity::Critical,
                confidence_milli: if h.len >= 16 { 950 } else { 800 },
                blocks: h.len,
                evidence: vec![
                    format!(
                        "blocks {}..{} ({} bytes) form one damaged span starting at the file head; {} of {} have entropy ≥ 7.85 bits/byte and no Oracle header",
                        h.start,
                        h.end().saturating_sub(1),
                        h.len.saturating_mul(bs),
                        h.encrypted,
                        h.len
                    ),
                    format!("healthy Oracle blocks never exceed 7.68 bits/byte (LV-077); random bytes pass the header-format test ~1/16 of the time, which explains the interleaved checksum/torn verdicts; {n_enc_total} encrypted blocks in the file"),
                ],
                cause_en: "Ransomware. Most families encrypt only the first N kilobytes or megabytes of large files for speed — exactly the region that holds the OS block, the file header and the first space-management blocks.".into(),
                cause_ar: "فدية. أغلب العائلات تشفّر أول N كيلوبايت/ميغابايت من الملفات الكبيرة فقط للسرعة — وهي بالضبط منطقة رأس الملف وأولى كتل إدارة المساحة.".into(),
                recovery_en: "The encrypted region is lost without the key. Every block after it that passes its own checks is recoverable: use odr scavenge (no dictionary) or odr extract with --assume for the file identity.".into(),
                recovery_ar: "المنطقة المشفَّرة ضائعة بلا مفتاح. كل كتلة بعدها تجتاز فحوصها قابلة للاسترجاع: odr scavenge (بلا قاموس) أو odr extract مع --assume لهوية الملف.".into(),
                oracle_errors: vec!["ORA-01122".into(), "ORA-01210".into(), "ORA-27047".into()],
            });
        }
        let rest: Vec<&Span> = enc_spans.iter().copied().filter(|s| s.start > 1).collect();
        let starts: Vec<u64> = rest.iter().map(|s| s.start).collect();
        if let Some((stride, n)) = periodic(&starts) {
            let blocks: u64 = rest.iter().map(|s| s.len).fold(0, u64::saturating_add);
            out.push(Finding {
                code: FindingCode::RansomwarePeriodic,
                severity: severity_by_share(blocks, used, Severity::High),
                confidence_milli: 900,
                blocks,
                evidence: vec![
                    format!("{} encrypted spans start every {stride} blocks (±1), {n} of them in phase", rest.len()),
                    format!("span length {} blocks (first span)", rest.first().map_or(0, |s| s.len)),
                ],
                cause_en: "Intermittent (\"fast\") ransomware encryption: a fixed-size chunk every fixed stride, leaving the rest untouched.".into(),
                cause_ar: "تشفير فدية متقطّع («سريع»): قطعة ثابتة الحجم كل خطوة ثابتة، والباقي سليم.".into(),
                recovery_en: "Encrypted chunks are lost; the blocks between them are intact and extractable. Rows that span an encrypted chunk are rejected, not guessed.".into(),
                recovery_ar: "القطع المشفَّرة ضائعة؛ الكتل بينها سليمة وقابلة للاستخراج. الصف الذي يمتد إلى قطعة مشفَّرة يُرفض ولا يُخمَّن.".into(),
                oracle_errors: vec!["ORA-01578".into(), "ORA-01110".into()],
            });
        } else {
            // Encrypted blocks not inside any dominated span, plus dominated
            // spans that are neither at the head nor periodic.
            let blocks = count("encrypted")
                .saturating_add(rest.iter().map(|s| s.len).fold(0, u64::saturating_add));
            if blocks > 0 {
                out.push(Finding {
                    code: FindingCode::EncryptedScattered,
                    severity: severity_by_share(blocks, used, Severity::Medium),
                    confidence_milli: 850,
                    blocks,
                    evidence: vec![format!(
                        "{blocks} high-entropy blocks in {} spans without a fixed stride",
                        rest.len().saturating_add(by("encrypted").len())
                    )],
                    cause_en: "Random-looking bytes without an Oracle header, scattered: partial encryption, or encrypted data from another file written over this one.".into(),
                    cause_ar: "بايتات عشوائية بلا رأس أوراكل، متفرقة: تشفير جزئي، أو بيانات مشفَّرة من ملف آخر كُتبت فوق هذا.".into(),
                    recovery_en: "These blocks are lost. Neighbours are judged individually.".into(),
                    recovery_ar: "هذه الكتل ضائعة. الجيران يُحكمون فرادى.".into(),
                    oracle_errors: vec!["ORA-01578".into(), "ORA-01110".into()],
                });
            }
        }
    }

    // --- Zeroed ---------------------------------------------------------
    let n_zero = count("zeroed");
    let n_hz = count("header_zeroed");
    if n_zero.saturating_add(n_hz) > 0 {
        let z = by("zeroed");
        let mut ev = vec![
            format!("{n_zero} all-zero blocks in {} regions", z.len()),
            format!(
                "largest region {} blocks",
                z.iter().map(|g| g.len).max().unwrap_or(0)
            ),
        ];
        if n_hz > 0 {
            ev.push(format!(
                "{n_hz} blocks have a zeroed 20-byte header but a non-zero body: sub-block holes (sector-sized)"
            ));
        }
        out.push(Finding {
            code: FindingCode::ZeroedRegions,
            severity: severity_by_share(n_zero.saturating_add(n_hz), used, Severity::Medium),
            confidence_milli: 900,
            blocks: n_zero.saturating_add(n_hz),
            evidence: ev,
            cause_en: "Blocks containing only zeros inside the formatted range: a storage hole (thin provisioning / sparse copy), a wipe, or an incomplete restore. Oracle formats every block it allocates, so zeros where a header should be are not normal.".into(),
            cause_ar: "كتل كلها أصفار داخل المدى المهيّأ: ثقب تخزين (نسخ متفرق)، أو مسح، أو استرجاع ناقص. أوراكل يهيّئ كل كتلة يخصّصها، فالأصفار مكان الرأس ليست طبيعية.".into(),
            recovery_en: "Lost. Rows that live entirely in other blocks are unaffected.".into(),
            recovery_ar: "ضائعة. الصفوف الواقعة كلياً في كتل أخرى لا تتأثر.".into(),
            oracle_errors: vec!["ORA-01578".into(), "ORA-08103".into()],
        });
    }

    // --- Bit rot --------------------------------------------------------
    let n_ck = count("checksum_fail");
    if n_ck > 0 {
        let g = by("checksum_fail");
        let isolated = g.iter().filter(|x| x.len == 1).count();
        out.push(Finding {
            code: FindingCode::BitRot,
            severity: severity_by_share(n_ck, used, Severity::Low),
            confidence_milli: 950,
            blocks: n_ck,
            evidence: vec![
                format!("{n_ck} blocks carry the checksum flag and fail the 16-bit XOR fold"),
                format!("{isolated} of {} regions are single blocks (scattered, not contiguous)", g.len()),
            ],
            cause_en: "Media/physical corruption: bit rot on disk, a failing drive or controller cache, bad RAM at write time, or a faulty transfer. The header is still Oracle's, so the block was written correctly once and damaged later.".into(),
            cause_ar: "تلف مادي/وسائط: تعفّن بت على القرص، قرص أو كاش متحكم معطوب، ذاكرة سيئة وقت الكتابة، أو نقل معيب. الرأس ما زال لأوراكل، فالكتلة كُتبت سليمة ثم تلفت لاحقاً.".into(),
            recovery_en: "Each block's rows are rejected unless an independent witness exists; sibling rows in the same block are withheld if any cell fails to decode exactly (LV-064). Oracle would report ORA-01578 on first read.".into(),
            recovery_ar: "صفوف الكتلة تُرفض ما لم يوجد شاهد مستقل؛ الصفوف الشقيقة تُحجب إن فشلت أي خلية في الفكّ بدقة (LV-064). أوراكل كان سيعطي ORA-01578 عند أول قراءة.".into(),
            oracle_errors: vec!["ORA-01578".into(), "ORA-01110".into()],
        });
    }

    // --- Torn -----------------------------------------------------------
    let n_torn = count("torn");
    if n_torn > 0 {
        out.push(Finding {
            code: FindingCode::TornWrites,
            severity: severity_by_share(n_torn, used, Severity::Low),
            confidence_milli: 950,
            blocks: n_torn,
            evidence: vec![format!(
                "{n_torn} blocks whose 4-byte tail does not repeat the header's SCN/type/seq"
            )],
            cause_en: "Fractured (torn) writes: the block was being rewritten when power failed or the host crashed, so part of it is the old image and part the new. Classic after a power cut without battery-backed cache.".into(),
            cause_ar: "كتابة مقطوعة (torn): كانت الكتلة تُكتب حين انقطعت الكهرباء أو تعطل المضيف، فجزء منها النسخة القديمة وجزء الجديدة. كلاسيكي بعد انقطاع كهرباء بلا كاش ببطارية.".into(),
            recovery_en: "The header and row directory describe the version they belong to; rows whose bytes lie entirely below the damage frontier are extracted as inferred (LV-065), never verified.".into(),
            recovery_ar: "الرأس ودليل الصفوف يصفان النسخة التي ينتميان إليها؛ الصفوف الواقعة كلياً تحت حدّ التلف تُستخرج مستنتَجة (LV-065) لا موثَّقة.".into(),
            oracle_errors: vec!["ORA-01578".into(), "ORA-01110".into(), "ORA-00600 [kcbzpb_1]".into()],
        });
    }

    // --- Misplaced ------------------------------------------------------
    let n_mis = count("misplaced");
    if n_mis > 0 {
        out.push(Finding {
            code: FindingCode::MisdirectedWrites,
            severity: severity_by_share(n_mis, used, Severity::Medium),
            confidence_milli: 950,
            blocks: n_mis,
            evidence: vec![format!(
                "{n_mis} blocks pass header, tail and checksum but their rdba names a different block"
            )],
            cause_en: "Misdirected writes: the storage layer wrote a correct block to the wrong address (firmware/RAID/LVM bug), or blocks were swapped by a faulty copy tool. Oracle calls this a \"block mismatch\".".into(),
            cause_ar: "كتابات في غير مكانها: طبقة التخزين كتبت كتلة سليمة في عنوان خاطئ (خلل firmware/RAID/LVM)، أو تبدّلت الكتل بأداة نسخ معيبة. أوراكل يسمّيه «block mismatch».".into(),
            recovery_en: "The content belongs to the block named in its header. odr scavenge groups rows by their real object id, so these rows are recovered under the right table; the block that should have been here is missing.".into(),
            recovery_ar: "المحتوى يخص الكتلة المسمّاة في رأسه. odr scavenge يجمّع الصفوف بمعرّف الكائن الحقيقي فتُسترجع تحت الجدول الصحيح؛ أما الكتلة التي كان يجب أن تكون هنا فمفقودة.".into(),
            oracle_errors: vec!["ORA-01578".into(), "ORA-01110".into(), "ORA-00600 [kcbzib_1]".into()],
        });
    }

    // --- Logical --------------------------------------------------------
    let n_log = count("logical");
    if n_log > 0 {
        let mut rules: Vec<String> = r
            .logical_by_rule
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();
        rules.sort();
        out.push(Finding {
            code: FindingCode::LogicalCorruption,
            severity: severity_by_share(n_log, used, Severity::Medium),
            confidence_milli: 900,
            blocks: n_log,
            evidence: {
                let mut e = vec![format!(
                    "{n_log} data blocks pass every physical check yet violate an internal invariant"
                )];
                e.extend(rules);
                e
            },
            cause_en: "Logical (soft) corruption: the block was written with a valid checksum but inconsistent contents — an Oracle bug, memory corruption before the write, or a byte changed and the checksum recomputed (forged).".into(),
            cause_ar: "تلف منطقي (soft): كُتبت الكتلة بـ checksum صحيح ومحتوى متناقض — خلل في أوراكل، أو فساد ذاكرة قبل الكتابة، أو بايت تغيّر وأُعيد حساب checksum (تزوير).".into(),
            recovery_en: "The whole block is rejected (LV-062): no row from it is verified. Oracle's own check (DB_BLOCK_CHECKING / dbv) would raise ORA-00600 [kddummy_blkchk] with the kdbchk code listed above.".into(),
            recovery_ar: "الكتلة كلها مرفوضة (LV-062): لا صف منها موثَّق. فحص أوراكل (DB_BLOCK_CHECKING / dbv) كان سيعطي ORA-00600 [kddummy_blkchk] برمز kdbchk المذكور.".into(),
            oracle_errors: vec!["ORA-00600 [kddummy_blkchk]".into(), "ORA-00600 [kdBlkCheckError]".into(), "ORA-08102".into()],
        });
    }

    // --- Marked corrupt -------------------------------------------------
    let n_marked = count("marked_corrupt");
    if n_marked > 0 {
        out.push(Finding {
            code: FindingCode::MarkedCorrupt,
            severity: severity_by_share(n_marked, used, Severity::Medium),
            confidence_milli: 900,
            blocks: n_marked,
            evidence: vec![format!("{n_marked} blocks have seq_kcbh = 0xFF")],
            cause_en: "Oracle itself marked these blocks corrupt: DBMS_REPAIR.FIX_CORRUPT_BLOCKS, or media recovery over a NOLOGGING operation (ORA-26040).".into(),
            cause_ar: "أوراكل نفسه وسم هذه الكتل تالفة: DBMS_REPAIR.FIX_CORRUPT_BLOCKS، أو استرجاع وسائط فوق عملية NOLOGGING (ORA-26040).".into(),
            recovery_en: "The content is whatever was there when the mark was set and is not trusted; rows are reported as inferred at best.".into(),
            recovery_ar: "المحتوى هو ما كان موجوداً عند الوسم وغير موثوق؛ الصفوف تُبلَّغ مستنتَجة في أحسن الأحوال.".into(),
            oracle_errors: vec!["ORA-01578".into(), "ORA-26040".into()],
        });
    }

    // --- Garbage --------------------------------------------------------
    let n_gb = count("garbage");
    if n_gb > 0 {
        out.push(Finding {
            code: FindingCode::GarbageOverwrite,
            severity: severity_by_share(n_gb, used, Severity::Medium),
            confidence_milli: 850,
            blocks: n_gb,
            evidence: vec![format!(
                "{n_gb} blocks have no Oracle header and low entropy (< 7.85 bits/byte): structured, non-random bytes"
            )],
            cause_en: "Something else was written over these blocks: another file after a filesystem mix-up, a text/log file, a partial restore of a different image.".into(),
            cause_ar: "كُتب شيء آخر فوق هذه الكتل: ملف آخر بعد اختلاط نظام الملفات، ملف نصي/سجل، أو استرجاع جزئي لصورة مختلفة.".into(),
            recovery_en: "Lost for this file. The foreign content may be of forensic interest.".into(),
            recovery_ar: "ضائعة لهذا الملف. المحتوى الغريب قد يهم التحقيق الجنائي.".into(),
            oracle_errors: vec!["ORA-01578".into(), "ORA-01110".into()],
        });
    }

    // --- Truncated last block (no declared size) ------------------------
    if declared.is_none() && r.last_block_partial {
        out.push(Finding {
            code: FindingCode::Truncated,
            severity: Severity::High,
            confidence_milli: 1000,
            blocks: 1,
            evidence: vec![format!(
                "file length {} is not a multiple of the block size {}",
                r.file_len, r.layout.block_size
            )],
            cause_en: "The file ends inside a block.".into(),
            cause_ar: "الملف ينتهي داخل كتلة.".into(),
            recovery_en:
                "The partial block is unusable; everything before it is judged on its own.".into(),
            recovery_ar: "الكتلة الناقصة غير صالحة؛ ما قبلها يُحكم بدليله.".into(),
            oracle_errors: vec!["ORA-27046".into()],
        });
    }

    if out.is_empty() {
        out.push(Finding {
            code: FindingCode::Healthy,
            severity: Severity::Info,
            confidence_milli: 1000,
            blocks: 0,
            evidence: vec![format!(
                "{} blocks examined: {} in use, {} empty, 0 damaged",
                c.examined,
                c.used_clean(),
                c.empty
            )],
            cause_en: "No physical or logical damage detected.".into(),
            cause_ar: "لا تلف مادي ولا منطقي.".into(),
            recovery_en: "Every table can be extracted with odr extract; all rows should be verified.".into(),
            recovery_ar: "كل جدول قابل للاستخراج بـ odr extract؛ كل الصفوف ينبغي أن تكون موثَّقة.".into(),
            oracle_errors: vec![],
        });
    }
    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(b.blocks.cmp(&a.blocks)));
    out
}
