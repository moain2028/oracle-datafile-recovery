//! Triage layer on top of a [`Report`]: a single **recoverability score**
//! (0–100) a support technician can act on, a plain-language verdict, and
//! the exact next commands to run.
//!
//! The score is deterministic and explained: every deduction names the
//! fact that caused it. It is a *triage aid*, not a promise — the three
//! output layers of `odr extract` remain the only ground truth.

use super::{FindingCode, LayoutSource, Report};
use serde::{Deserialize, Serialize};

/// One deduction from the score, with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deduction {
    /// Points removed.
    pub points: u8,
    /// Why (English).
    pub reason_en: String,
    /// Why (Arabic).
    pub reason_ar: String,
}

/// Coarse verdict band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    /// 90–100: nothing or almost nothing lost.
    Excellent,
    /// 70–89: isolated damage; routine extraction.
    Good,
    /// 40–69: significant loss or lost identity; expert extraction.
    Partial,
    /// 10–39: most data gone or unreadable.
    Poor,
    /// 0–9: effectively unrecoverable from this file alone.
    Lost,
}

/// A command the operator should run next, with its purpose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextStep {
    /// Short purpose (English).
    pub purpose_en: String,
    /// Short purpose (Arabic).
    pub purpose_ar: String,
    /// Shell command, with `<SYSTEM.dbf>` / `<out>` placeholders where the
    /// operator must fill in.
    pub command: String,
}

/// Triage summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triage {
    /// 0–100.
    pub score: u8,
    /// Band.
    pub band: Band,
    /// Deductions, largest first.
    pub deductions: Vec<Deduction>,
    /// One-paragraph verdict (English).
    pub verdict_en: String,
    /// One-paragraph verdict (Arabic).
    pub verdict_ar: String,
    /// Whether the file's own identity (file#, tablespace) is readable.
    pub identity_known: bool,
    /// Whether rows from this file can ever be reported *verified*.
    pub verified_possible: bool,
    /// Estimated share (‰) of *used* blocks that are clean.
    pub clean_used_milli: u16,
    /// Live rows counted on clean data blocks (logical depth), if measured.
    pub live_rows: Option<u64>,
    /// Ordered next steps.
    pub next_steps: Vec<NextStep>,
}

fn band(score: u8) -> Band {
    match score {
        90..=100 => Band::Excellent,
        70..=89 => Band::Good,
        40..=69 => Band::Partial,
        10..=39 => Band::Poor,
        _ => Band::Lost,
    }
}

/// Compute the triage for a report. `file_name` is used in the suggested
/// commands.
#[allow(clippy::too_many_lines)] // one rule per deduction, in a fixed order
pub fn triage(r: &Report, file_name: &str) -> Triage {
    let c = &r.counters;
    let used_total = c.used_clean().saturating_add(c.damaged_total());
    let clean_used_milli = u16::try_from(
        c.used_clean()
            .saturating_mul(1000)
            .checked_div(used_total.max(1))
            .unwrap_or(1000),
    )
    .unwrap_or(1000);
    let has = |code: FindingCode| r.findings.iter().find(|f| f.code == code);
    let mut ded: Vec<Deduction> = Vec::new();
    let mut push = |points: u8, en: String, ar: String| {
        if points > 0 {
            ded.push(Deduction {
                points,
                reason_en: en,
                reason_ar: ar,
            });
        }
    };

    // 1. Share of used blocks damaged — the backbone of the score.
    let damaged_milli = 1000u64.saturating_sub(u64::from(clean_used_milli));
    // 0‰ → 0 pts; 10‰ → 5; 100‰ → 30; 500‰ → 60; 1000‰ → 70 (concave).
    let share_pts = if damaged_milli == 0 {
        0
    } else if damaged_milli < 10 {
        3
    } else if damaged_milli < 100 {
        5 + u8::try_from(
            damaged_milli
                .saturating_sub(10)
                .saturating_mul(25)
                .checked_div(90)
                .unwrap_or(0),
        )
        .unwrap_or(25)
    } else if damaged_milli < 500 {
        30 + u8::try_from(
            damaged_milli
                .saturating_sub(100)
                .saturating_mul(30)
                .checked_div(400)
                .unwrap_or(0),
        )
        .unwrap_or(30)
    } else {
        60 + u8::try_from(
            damaged_milli
                .saturating_sub(500)
                .saturating_mul(10)
                .checked_div(500)
                .unwrap_or(0),
        )
        .unwrap_or(10)
    };
    push(
        share_pts,
        format!(
            "{}‰ of used blocks are damaged ({} of {})",
            damaged_milli,
            c.damaged_total(),
            used_total
        ),
        format!(
            "{}‰ من الكتل المستخدمة تالفة ({} من {})",
            damaged_milli,
            c.damaged_total(),
            used_total
        ),
    );

    // 2. Identity lost (header destroyed): rows can never be verified.
    let identity_known = matches!(r.layout.source, LayoutSource::FileHeader);
    if !identity_known {
        push(
            15,
            "file header (kcvfh) unreadable: file identity must be supplied with --assume and no row can be reported verified".into(),
            "رأس الملف (kcvfh) غير مقروء: هوية الملف تُعطى بـ --assume ولا يمكن توثيق أي صف".into(),
        );
    }

    // 3. Ransomware at the head: space-management + first extents gone.
    if let Some(f) = has(FindingCode::RansomwareHead) {
        push(
            10,
            format!(
                "ransomware encrypted the first {} blocks (header, space bitmaps, first extents)",
                f.blocks
            ),
            format!(
                "الفدية شفّرت أول {} كتلة (الرأس، خرائط المساحة، أولى الامتدادات)",
                f.blocks
            ),
        );
    }

    // 4. Truncation: everything past the end is gone.
    if let Some(f) = has(FindingCode::Truncated)
        && f.blocks > 1
    {
        let declared = r
            .layout
            .declared_blocks_header
            .or(r.layout.declared_blocks_os)
            .map_or(0u64, u64::from);
        let share = f
            .blocks
            .saturating_mul(1000)
            .checked_div(declared.max(1))
            .unwrap_or(0);
        let pts =
            u8::try_from(share.saturating_mul(40).checked_div(1000).unwrap_or(0)).unwrap_or(40);
        push(
            pts.max(5),
            format!(
                "{} declared blocks are missing from the end of the file ({share}‰)",
                f.blocks
            ),
            format!("{} كتلة معلَنة مفقودة من نهاية الملف ({share}‰)", f.blocks),
        );
    }

    // 5. Logical corruption: whole blocks rejected even though they look fine.
    if let Some(f) = has(FindingCode::LogicalCorruption) {
        push(
            if f.blocks >= 10 { 8 } else { 4 },
            format!(
                "{} blocks pass checksum but violate internal invariants (rejected whole)",
                f.blocks
            ),
            format!(
                "{} كتلة تجتاز checksum لكنها تخالف ثوابت داخلية (تُرفض كاملة)",
                f.blocks
            ),
        );
    }

    // 6. Misdirected writes: content survives under another address.
    if let Some(f) = has(FindingCode::MisdirectedWrites) {
        push(
            3,
            format!(
                "{} blocks are misplaced; scavenge re-attributes them by object id",
                f.blocks
            ),
            format!(
                "{} كتلة في غير مكانها؛ scavenge يعيد نسبها بمعرّف الكائن",
                f.blocks
            ),
        );
    }

    ded.sort_by(|a, b| b.points.cmp(&a.points));
    let total: u32 = ded.iter().map(|d| u32::from(d.points)).sum();
    let score = u8::try_from(100u32.saturating_sub(total)).unwrap_or(0);
    let band = band(score);
    let verified_possible = identity_known;
    let live_rows = r
        .objects
        .values()
        .filter_map(|o| o.live_rows)
        .reduce(u64::saturating_add);

    let (verdict_en, verdict_ar) = match band {
        Band::Excellent => (
            format!("The file is healthy or nearly so: {}‰ of used blocks are clean. Standard extraction should verify every row.", clean_used_milli),
            format!("الملف سليم أو شبه سليم: {}‰ من الكتل المستخدمة نظيفة. الاستخراج العادي سيوثّق كل الصفوف.", clean_used_milli),
        ),
        Band::Good => (
            format!("Isolated damage: {}‰ of used blocks are clean. Most tables extract fully; rows in damaged blocks are rejected or inferred, never guessed.", clean_used_milli),
            format!("تلف معزول: {}‰ من الكتل المستخدمة نظيفة. أغلب الجداول تُستخرج كاملة؛ صفوف الكتل التالفة تُرفض أو تُستنتج ولا تُخمَّن.", clean_used_milli),
        ),
        Band::Partial => (
            format!("Significant loss or lost identity: {}‰ of used blocks are clean{}. Expert extraction is worthwhile; expect inferred rows and gaps.", clean_used_milli, if identity_known { "" } else { ", and the file header is gone" }),
            format!("خسارة كبيرة أو هوية ضائعة: {}‰ من الكتل المستخدمة نظيفة{}. الاستخراج بخبير يستحق؛ توقّع صفوفاً مستنتَجة وفجوات.", clean_used_milli, if identity_known { "" } else { "، ورأس الملف ضائع" }),
        ),
        Band::Poor => (
            format!("Most data is unreadable: only {}‰ of used blocks are clean. Salvage what remains with scavenge; look for other copies.", clean_used_milli),
            format!("أغلب البيانات غير قابلة للقراءة: {}‰ فقط من الكتل المستخدمة نظيفة. أنقذ ما بقي بـ scavenge؛ ابحث عن نسخ أخرى.", clean_used_milli),
        ),
        Band::Lost => (
            "Effectively unrecoverable from this file alone. Do not spend extraction effort here before locating another copy, a backup or an export.".into(),
            "غير قابل للاسترجاع فعلياً من هذا الملف وحده. لا تُنفق جهد الاستخراج قبل إيجاد نسخة أخرى أو نسخة احتياطية أو تصدير.".into(),
        ),
    };

    // Next steps, in order.
    let mut steps = Vec::new();
    let is_system = r.objects.keys().any(|&o| o < 100); // bootstrap objects live below 100
    if band == Band::Lost {
        steps.push(NextStep {
            purpose_en: "Look for any other copy of this datafile (backup, standby, exported DMP) before anything else".into(),
            purpose_ar: "ابحث عن أي نسخة أخرى من هذا الملف (نسخة احتياطية، standby، تصدير DMP) قبل أي شيء".into(),
            command: String::new(),
        });
    }
    if identity_known {
        steps.push(NextStep {
            purpose_en: "List tables and columns from the dictionary (needs the SYSTEM datafile of the same database)".into(),
            purpose_ar: "اعرض الجداول والأعمدة من القاموس (يحتاج ملف SYSTEM لنفس القاعدة)".into(),
            command: if is_system {
                format!("odr dict \"{file_name}\" --json")
            } else {
                format!("odr dict <SYSTEM.dbf> \"{file_name}\" --json")
            },
        });
        steps.push(NextStep {
            purpose_en: "Extract a table into verified / inferred / rejected layers".into(),
            purpose_ar: "استخرج جدولاً إلى طبقات موثَّق / مستنتَج / مرفوض".into(),
            command: format!(
                "odr extract <SYSTEM.dbf> \"{file_name}\" --table OWNER.TABLE --out <out>/"
            ),
        });
    } else {
        steps.push(NextStep {
            purpose_en: "Header destroyed: extract with the file identity supplied by the operator (rows will be inferred, never verified)".into(),
            purpose_ar: "الرأس ضائع: استخرج مع إعطاء هوية الملف (الصفوف مستنتَجة لا موثَّقة)".into(),
            command: format!(
                "odr extract <SYSTEM.dbf> \"{file_name}\" --table OWNER.TABLE --out <out>/ --assume \"{file_name}=file:<N>,ts:<N>,rfile:<N>,bs:{}\"",
                r.layout.block_size
            ),
        });
    }
    if band != Band::Excellent {
        steps.push(NextStep {
            purpose_en: "No SYSTEM available? Recover every object by data object id with inferred column types".into(),
            purpose_ar: "لا يوجد SYSTEM؟ استرجع كل كائن بمعرّفه مع أنواع أعمدة مستنتَجة".into(),
            command: format!("odr scavenge \"{file_name}\" --out <out>/"),
        });
    }
    if has(FindingCode::RansomwareHead).is_some() || has(FindingCode::RansomwarePeriodic).is_some()
    {
        steps.push(NextStep {
            purpose_en: "Ransomware: preserve this file read-only as evidence; the encrypted region is lost without the key — do not pay before measuring what survived".into(),
            purpose_ar: "فدية: احفظ هذا الملف للقراءة فقط كدليل؛ المنطقة المشفَّرة ضائعة بلا مفتاح — لا تدفع قبل قياس ما نجا".into(),
            command: String::new(),
        });
    }
    if has(FindingCode::BitRot).is_some() || has(FindingCode::TornWrites).is_some() {
        steps.push(NextStep {
            purpose_en: "Physical damage: check the disk (SMART), controller cache battery and power protection before restoring onto the same hardware".into(),
            purpose_ar: "تلف مادي: افحص القرص (SMART) وبطارية كاش المتحكم والحماية الكهربائية قبل الاسترجاع على نفس العتاد".into(),
            command: String::new(),
        });
    }

    Triage {
        score,
        band,
        deductions: ded,
        verdict_en,
        verdict_ar,
        identity_known,
        verified_possible,
        clean_used_milli,
        live_rows,
        next_steps: steps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_cover_the_range() {
        assert_eq!(band(100), Band::Excellent);
        assert_eq!(band(90), Band::Excellent);
        assert_eq!(band(89), Band::Good);
        assert_eq!(band(70), Band::Good);
        assert_eq!(band(69), Band::Partial);
        assert_eq!(band(40), Band::Partial);
        assert_eq!(band(39), Band::Poor);
        assert_eq!(band(10), Band::Poor);
        assert_eq!(band(9), Band::Lost);
        assert_eq!(band(0), Band::Lost);
    }
}
