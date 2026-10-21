# مذكرة التسليم النهائية — 2026-10-18 (Lead، بعد دمج PR #21)

> **هذه هي نقطة البداية الوحيدة للوكيل القادم.** تحلّ محلّ `2026-10-18_handover.md` و`2026-10-15_handover.md` في كل ما يتعارض (أبقيناهما كسجل تاريخي).
> مصدر الحقيقة الحيّ دائماً: `git log` على `main` + GitHub Actions + `STATUS.md`.
> البيئة من الصفر (أوامر منفَّذة فعلاً): `docs/runbooks/agent-environment.md`.

---

## 0. الحالة في ثلاث جمل

1. **ODR يقرأ ملفات Oracle 26ai حقيقية من الرأس حتى القيم** بلا instance: `odr dict` يحلّ قاموس PDB كاملاً (71,849 كائن، 2,560 جدول، **0 مشاكل**) في 3.8 ث، و`odr extract` يُخرج أي جدول heap/cluster بثلاث طبقات (verified / inferred / rejected) في 3.7 ث.
2. **كل قيمة في المجموعة الذهبية تطابق أوراكل بايتاً ببايت** (141,581 قيمة، 15 جدولاً، ROWIDs 15/15)، وتحت التلف المتعمَّد **precision(verified) = 1.000** (أربعة أنماط) — أي لم تخرج قيمة خاطئة بوسم أخضر قط بعد LV-059.
3. **المتبقي للمرحلة M3** محدَّد في §6 أدناه بالترتيب ومعايير القبول: سدّ فجوة torn، أنماط forge/ransom/swap، مختبر 19c/11g، LOB/الضغط، ثم إغلاق g (قيم T_CL_* والعربية).

---

## 1. ما الذي بُني (الخريطة الهندسية)

```
.dbf (read-only, SHA-256 قبل/بعد)
 └─ block.rs        classify() → BlockStatus {Valid, Zeroed, Truncated, Foreign, TailMismatch, ChecksumFail}
 └─ dict/kcvfh.rs   رأس الملف: block size، byte order، file#، rfile (1024 = bigfile)، tsn، root dba @96
 └─ dict/source.rs  BlockSource / FileSet (ملفات متعددة) / ImageSet؛ FileInfo::from_header؛ absolute_by_relative(ts-scoped)  [LV-055]
 └─ dict/segment.rs ExtentMap (0x10/0x23/0x24) + walk_segment؛ حدّ MAX_SEGMENT_BLOCKS (fuzz)
 └─ dict/bootstrap.rs  صفوف bootstrap$ ← DDL ← Catalog (59 كائناً بأعمدتها بالاسم)
 └─ dict/resolver.rs   Dictionary::load(src, system_file): OBJ$ (heap) · TAB$/CLU$/COL$ (C_OBJ# tabno 1/2/5) · USER$ (C_USER#) · TS$ (C_TS#) · PROPS$ (عبر OBJ$)
 │                     tables() · table(owner,name) · cluster_of() · properties() · issues (لا تخمين)
 └─ dict/schema.rs     ColumnDef/TableDef/Cell{Verified,Inferred,Unknown}; materialize() + قاعدة ADD COLUMN DEFAULT (bit-vector)  [LV-050]
 └─ dict/rows.rs       read_heap / read_cluster → RawRow {provenance, rowid_row, columns, pieces, block_status}  [LV-046..049, 057..059]
 └─ row/{ktb,kdb,piece,cluster}.rs   ITL، kdbh/kdbt/kdbr، قطع الصفوف (chained/migrated/rowdep/cluster)  [LV-047, 049, 052, 057]
 └─ types/*, charset/*, value.rs     NUMBER/DATE/TIMESTAMP/INTERVAL/FLOAT/ROWID/CHAR/RAW + 8 ترميزات بما فيها العربية  [LV-051]
 └─ odr-cli/dict_cmd.rs  odr dict | odr extract (--charset، --assume) → verified.csv / inferred.csv(_inference) / rejected.csv(_reason) / report.json  [LV-060]
 └─ odr-corrupt          تلف حتمي بمانيفست (10 أنماط) لنسخ الملفات فقط
```

**الحجم:** 12,202 سطر Rust في 4 crates (`odr-core`, `odr-cli`, `odr-testkit`, `odr-corrupt`) + `fuzz/` (11 هدفاً) · 105 اختبار وحدة/تكامل + 13 اختباراً ذهبياً (8 core + 5 CLI) + proptest.

---

## 2. القوة التي اشتُغل بها (المنهجية — طبّقها حرفياً)

هذه ليست «أفضل ممارسات» عامة؛ هي ما أنتج 60 حقيقة مُثبتة و0 قيمة خاطئة:

| # | المبدأ | كيف يُطبَّق عملياً |
|---|---|---|
| 1 | **القيمة الخاطئة أسوأ من الناقصة** | كل خلية تخرج في طبقة واحدة من ثلاث. لا `Null` افتراضي، لا تخمين صامت. إن لم تستطع إثبات القيمة فهي `Inferred` بوسم السبب أو `Rejected` بسبب. |
| 2 | **المصادر المنشورة فرضيات لا حقائق** | 13 ادعاءً منشوراً سقط أمام الملف الحقيقي (LV-009, 047, 049, 050, 052, 053, 054, …). كل ثابت ثنائي يُثبت بمطابقة dump أوراكل لنفس الكتلة قبل اعتباره ✅. |
| 3 | **الاختبار الذهبي هو البوابة الوحيدة الصادقة** | أي تغيير في محلّل ⇒ `golden_lab` 8/8 + `golden_cli` 5/5 قبل الـ push. «الصف موجود» لا يكفي — «كل قيمة == ما يطبعه أوراكل». |
| 4 | **كل حقيقة لها رقم LV** | اكتشاف ⇒ صف `LV-xxx` في `docs/design/lab-verify.md` (الموقع، الادعاء، الدليل من الملف، الحالة) + إصلاح + اختبار يحمل الرقم في تعليقه. |
| 5 | **التلف يُقاس لا يُفترض** | `odr-corrupt` حتمي (seed) على نسخة ⇒ precision/recall ضد الاستخراج النظيف ⇒ اختبار ذهبي دائم (`damaged_copies_never_yield_a_wrong_verified_cell`). |
| 6 | **fuzz محلياً قبل CI** | 45 ث لكل هدف ملموس؛ كشف عطلين حقيقيين في أول تشغيل. `cd fuzz && cargo check --bins` إلزامي لأن `fuzz/` خارج الـ workspace. |
| 7 | **كل عائق يُحلّ في الـ sandbox ويُوثَّق بالأمر الذي نجح** | runbook §6: Docker بلا iptables، توكن منتهٍ، `ORA-01489`، قرص ممتلئ، Windows CI (`read_text` لا CRLF). |
| 8 | **قيود لغوية صارمة = أمان المحلّل** | `unsafe` ممنوع؛ indexing/unwrap/panic/arithmetic مرفوضة بـ `-D warnings` + `clippy::pedantic`؛ الاختبارات تحمل `#![allow]` موثقاً فقط. |
| 9 | **PR واحد = ميزة واحدة مكتملة التوثيق** | كل PR يحدّث: lab-verify، CHANGELOG، LESSONS (إن > 30 دقيقة)، STATUS، مذكرة التسليم. يُدمج squash بعد CI أخضر (Linux + Windows + licenses + fuzz-smoke). |

**إيقاع العمل الذي نجح (دورة 60–90 دقيقة):** فرع `lead/<feature>` → تجربة على الملف الحقيقي (بايتات) → فرضية → إصلاح أصغر ما يمكن → اختبار يحمل LV → البوابات كلها → توثيق → push → `gh pr checks` → merge → `git checkout main && git pull`.

---

## 3. سجل الـ PRs المدموجة في هذه الجلسة (ماذا، ولماذا، وبأي دليل)

| PR | المحتوى | الاكتشافات (LV) | الدليل القابل للتكرار |
|---|---|---|---|
| #18 | `schema.rs` + `rows.rs` (قارئ heap/cluster) | 046 rowid_row نسبي للـ kdbt · 047 `ktbbhict` بايت واحد · 048 كتلة مهيأة فارغة سليمة · 049 `nrid/hrid` BE دائماً | `lab_row_readers_match_oracle_rowids` 15/15 |
| #19 | مقارنة القيم بايت-ببايت؛ إصلاح Windows CI | 050 bit-vector ADD COLUMN (nullable فقط) · 051 `80 02` = 1E-130 · 052 `dscn` 7 بايت يُحسم بالرصّ | `lab_values_match_oracle_dump_byte_for_byte` 141,581 |
| #20 | `resolver.rs` + CLI `dict/extract` + cluster عبر CLU$ + 3 أهداف fuzz | 053 هوية SYSTEM من bootstrap$ لا من file# · 054 LONG آخر الصف · 055 rfile فريد داخل ts | `lab_dictionary_resolves_odr_tables_like_oracle`، `lab_end_to_end_dictionary_to_cells` 131,536، golden_cli 3/3 |
| #21 | PROPS$ charset؛ 0 مشاكل قاموس؛ طبقة سلامة الكتلة؛ `--assume` | 056 VALUE$ nullable · 057 عضو cluster مهاجر `F L`+hrid · 058 صف D مُعاد الكتابة · **059 كتلة غير Valid ⇒ rejected** · **060 `--assume` ⇒ inferred** | `damaged_copies_never_yield_a_wrong_verified_cell`، `wiped_header_requires_assume_and_downgrades_to_inferred` |

تفاصيل كل اكتشاف (البايتات، الكتلة، الـ slot): `docs/design/lab-verify.md` LV-046..060 و`2026-10-18_handover.md` §4.

---

## 4. الأرقام المقاسة (26ai، sandbox 4 أنوية، release build)

| القياس | القيمة | المصدر |
|---|---|---|
| قاموس PDB كامل (`odr dict` على SYSTEM 291 MB + 3 ملفات) | **3.76 ث**، 71,849 كائن، 2,560 جدول، 156 مستخدم، 10 ts، **0 issues** | قياس 2026-10-18 |
| `odr extract ODR.T_JOURNAL` (20,000 صف) | **3.67 ث** (يشمل تحميل القاموس) | قياس 2026-10-18 |
| مطابقة القيم مع `DUMP` أوراكل | 141,581 / 141,581 (13 جدول heap) | golden_lab |
| ROWIDs | 15/15 جدول (بما فيها cluster) | golden_lab |
| **precision(verified) تحت التلف** (seed 7، `odr_odr_assm01`) | zero_200 **1.000** / bitrot_400 **1.000** / holes_20_16 **1.000** / torn_200 **1.000** | golden_cli LV-059 |
| recall تحت التلف | 0.958 / 0.937 / 1.000 / 0.986 | golden_cli LV-059 |
| **forge/swap/ransom** (LV-062، 3 بذور على امتداد T_JOURNAL) | swap 105/105 مرفوضة · forge 39/144 كتلة + 110/54,000 صف خاطئ (0.2%) · ransom recall 0.895، 0 خطأ · 0 إيجابيات كاذبة على 18,084 كتلة | golden_cli `forged_swapped_and_encrypted_copies_are_rejected_not_believed` |
| **محاسبة كاملة تحت التلف** (LV-061) | verified + rejected + صفوف الكتل المصفَّرة = **20,000** في الأنماط الأربعة (torn: 19,721 + 279 + 0) | golden_cli + `lab_torn_block_accounts_for_every_slot` |
| wipe-header بلا `--assume` / معه | رفض (exit 1) / 0 verified · 20,000 inferred، القيم == النظيف | golden_cli LV-060 |
| حقائق lab-verify | 70 صفاً: 25 ✅ على ملف حقيقي (أول ✅ على إصدارين)، 45 ⬜ (أغلبها يحتاج 19c/11g أو big-endian أو LOB) | lab-verify.md |

> **تحذير:** أرقام الأداء من sandbox بلا ضمانات؛ أرقام الدقة فقط هي الملزِمة.

---

## 5. حدود معروفة (مكتوبة بصراحة)

| الحد | الأثر | أين موثَّق |
|---|---|---|
| ~~فجوة torn~~ **مغلقة (LV-061، 2026-10-21)**: كل خانة في كتلة غير سليمة تُحاسَب | — | lab-verify LV-061 |
| **forge داخل قيمة عمود** (checksum مُعاد + بايت واحد داخل VARCHAR2/NUMBER/DATE يبقى صالحاً) | 0.2% من الصفوف في اختبار forge تخرج verified بقيمة خاطئة؛ البنية الأحادية للكتلة لا تميّز `USD` عن `VSD`. ما يُكشف: دليل الصفوف، الأطوال، الأعلام، lock، cc، swap كاملاً. الحل الجذري عبر-الكتل (فهارس/undo) = M6 | LV-062 |
| **ransom**: الكتل المشفَّرة `integrity: Foreign/TailMismatch`؛ الرأس المشفَّر يحتاج `--assume` | مُختبَر آلياً: 0 خطأ، recall 0.895 | LV-062 |
| Hash clusters مرفوضة صراحةً | رسالة واضحة | dict_cmd.rs |
| LOB / الضغط OLTP/Basic غير مدعومة (M5) | رفض صريح لا تخمين | STATUS M5 |
| مؤكَّد على **26ai + 21c + 11g** (LE)؛ 19c وbig-endian ⬜ | LV-009 BE، LV-019/020 | §6 خطوة 3 |
| اختبار القيم لا يشمل T_CL_* ولا النصوص العربية `\XXXX` (escape في rows.psv) | g غير مغلق 100% | §6 خطوة 5 |
| `fuzz/corpus/` (1,023 ملف، 4.1 MB) متتبَّع في git منذ #20 | قرار: بذور أم `.gitignore` | §6 خطوة 0 |

---

## 6. ما الباقي — بالترتيب ومعايير القبول

| # | المهمة | التنفيذ المقترح | معيار القبول (قابل للقياس) |
|---|---|---|---|
| **0** | نظافة | قرار `fuzz/corpus/`؛ حذف الفقرات التاريخية المكررة من `2026-10-18_handover.md` إن أردت | CI أخضر |
| **1** | ~~سدّ فجوة torn~~ **✅ منجزة (LV-061، 2026-10-21)** | `dict/rows.rs`: `trusted = status == Valid`؛ في كتلة غير موثوقة تُعدَّد كل خانات `nrow` ولا يُطبَّق أي تخطٍّ (`D`/continuation/`is_deleted`/unused) — تخرج `RowError::Integrity`؛ والكتلة التي لا تُفكّ تُسجَّل `integrity:` | ✅ `verified + rejected + zeroed == 20000` للأنماط الأربعة؛ precision 1.000 ثابتة؛ اختباران ذهبيان |
| **2** | ~~forge / ransom / swap~~ **✅ منجزة (LV-062، 2026-10-21)** | `DataBlock::structural_check()` بسبع ثوابت صمدت على 18,084 كتلة حقيقية (موضع rdba، lock≤itc، خانة غير مفكوكة⇒D، lck≥مراجع، سلسلة frre، لا تداخل، سقف avsp)؛ الكتلة المخالفة ⇒ غير موثوقة كلها؛ `odr-corrupt --blocks` لاستهداف امتداد الجدول | ✅ swap 105/105 مرفوضة، precision 1.0 · forge: 39/144 كتلة مرفوضة بنيوياً، **الباقي 110/54,000 صف خاطئ (0.2%) تزوير داخل قيمة عمود — حد معلَن ومسقوف باختبار ذهبي** · ransom+`--assume`: 0 خطأ، recall 0.895 · 0 إيجابيات كاذبة |
| **3** | ~~مختبر 19c~~ **✅ مختبر 21c XE منجز (LV-063، 2026-10-21)** — 19c الرسمي ما زال يحتاج رخصة | `gvenzl/oracle-xe:21-slim` بلا رخصة (runbook §7b)؛ golden ثانية `golden-21c-20261021`؛ اختبار `lab2_second_version_reads_like_oracle` بلا أي ثابت مُشفَّر | ✅ 15/15 جدول ROWIDs == أوراكل، 140k خلية، قاموس 0 مشاكل؛ اكتشاف HWM (LV-063) أصلح 21c دون تغيير 26ai. **✅ 11g XE 11.2 غير-CDB منجز أيضاً (2026-10-21):** `golden-11g-20261021`، قاموس 0 مشاكل، `lab2` يمر **بلا أي تغيير في الكود** (runbook §7b). 19c عند توفر الرخصة |
| **4** | **LOB + الضغط** (M5) | في `10_corpus.sql`: جدول CLOB/BLOB (BasicFile ثم SecureFile) + جدول `COMPRESS BASIC`؛ `row/kdb.rs` يكشف flag 0x40 ويرفض صراحة الآن — ابدأ بـ LOB inline (< 4000) ثم LOB index/segment | recall ≥ 99% على LOB inline؛ الضغط: رفض صريح موثَّق حتى يُنفَّذ |
| **5** | **إغلاق g** | في `lab_values_match_oracle_dump_byte_for_byte`: فكّ `\XXXX` (UNISTR escape) للنصوص العربية، وأضف T_CL_DEPT/T_CL_EMP عبر `read_cluster` | 141,613 / 141,613 قيمة |
| **6** | **وضع بدون قاموس** (M4 — الميزة الفارقة) | `OBJ_<objd>` / `COL_<n>` من REQ-004، استدلال النوع من البايتات مع hold-out | دقة استدلال النوع ≥ 95% على مخططات لم تُستخدم في الضبط |
| 7 | Windows release | `cargo build --release --target x86_64-pc-windows-msvc` في CI وإرفاق `odr.exe` بـ Release | يعمل على ملف ذهبي على Windows |

**أول 30 دقيقة للوكيل القادم:** runbook §1–§2 → `gh pr list` (تأكد أن #10 الوحيد المفتوح) → تنزيل golden (runbook §4 البديل الأسرع، 62 MB؛ ملف `.sha256` يحمل مساراً مطلقاً — قارن الهاش يدوياً) → البوابات + `golden_lab` **11/11** (مع `ODR_GOLDEN2` من Release `golden-21c-20261021`) + `golden_cli` **7/7** → خطة اليوم في `docs/handover/2026-10-21_session_plan.md` (D ثم E)، وإلا الخطوة 4 (LOB).

---

## 7. البيئة الحالية (قد لا تبقى — كل شيء قابل لإعادة البناء)

| العنصر | الحالة | إعادة البناء |
|---|---|---|
| Rust 1.99.0 + nightly (cargo-fuzz) | مثبَّت | runbook §1 + §Fuzz |
| Docker + `odr-oracle` (26ai Free، حجم `odr-oradata`) | حاوية موجودة (unhealthy بعد SHUTDOWN — `docker restart odr-oracle` ثم انتظر healthy) · `/home/user/docker-data` 24 GB | runbook §3 (≈ 20 دقيقة) |
| القرص | **27/29 GB مستخدم** — احذف `target/` (1.9 GB) أو الحاوية عند الحاجة | — |
| المجموعة الذهبية | `lab/out/golden/20261018T080131Z/` محلياً + Release `golden-26ai-20261018` | runbook §4 |
| توكن GitHub | `~/.git-credentials` (المالك سيلغيه بعد الجلسة — اطلب توكناً جديداً `repo+workflow`) | runbook §2 |
| نسخ التلف اليدوية | `/tmp/dmg/*.dbf` (غير ضرورية — الاختبار يولّدها في العملية) | — |

---

## 8. مفتوح على المالك

1. **PR #10 (Astra، استنتاج حجم الكتلة بلا رأس):** لم يُحدَّث؛ `kcvfh::detect` + `--assume` يغطيان الحالة الآن. قرار: إغلاق أو إعادة بناء فوق `--assume`.
2. **رخصة Oracle 19c** في حساب المالك على container-registry (الخطوة 3).
3. **إلغاء التوكن** المستخدم في هذه الجلسة.
4. ملفات العميل `EXPORT:V11.01.00` (DMP) ليست DBF — M8 محتمل، خارج النطاق حالياً.

---

## 9. القواعد التي لا تُكسر (تذكير أخير)
قراءة فقط + SHA-256 (ADR-0002) · لا تخمين صامت · لا `.dbf`/`lab/out/` في git · كل ثابت ثنائي له LV ويُثبت على ملف حقيقي · البوابات الثماني + golden قبل كل PR · STATUS/CHANGELOG/LESSONS/lab-verify/هذه المذكرة تُحدَّث مع كل PR · الحديث مع المالك بعربية بسيطة.
