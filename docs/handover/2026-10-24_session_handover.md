# تسليم جلسة 2026-10-24 — Lead

> اقرأ هذا أولاً، ثم `2026-10-21_session_handover.md` (LV-061..066، الحدود المعلَنة)، ثم `2026-10-18_final_handover.md` (المنهجية والخطة §6).

## 1. الحالة في جمل
- **PR #35 (`lead/lob-out-of-line`)**: M5 جزء 2 — LOB خارج الصف مجمَّع من مقطع LOB (LV-070)؛ `odr extract` على T_LOB_* **6/6 verified**؛ 3 أنماط تلف للقطع مرفوضة بسببها. **M5 LOB مكتمل على المختبر** (BasicFile + SecureFile، inline + out-of-line). الباقي من M5: فكّ الضغط.
- **PR #34 (`lead/lob-inline`)**: M5 جزء 1 — LOB inline يُقرأ (LV-068)، الضغط يُرفض صراحة على ملف حقيقي (LV-069)، golden v2 `golden-26ai-20261024`. golden_lab 12/12 (v1 وv2)، golden_cli **10/10** (`ODR_GOLDEN_V2`)، 117 اختباراً.
- **main** بعد دمج **PR #33** (`lead/close-step-g`). المفتوح الوحيد: #10 (Astra، مسودة قديمة — لا تلمسه).
- **الخطوة g مغلقة (LV-067):** `lab_values_match_oracle_dump_byte_for_byte` يقارن **141,613/141,613** قيمة (كانت 141,581): T_CL_DEPT/T_CL_EMP عبر `read_cluster`، و**10,009 نصاً** عربياً/لاتينياً كان الاختبار يتخطاها (`\XXXX` من `ASCIISTR`) صارت تُفكّ وتُقارَن نصياً — في الاختبارات الذهبية الثلاثة (26ai، 11g، 21c). لم يُلمس أي كود مكتبة؛ الاختبار والتوثيق فقط.
- LV-020/LV-021 (تخطيط صف مفتاح/عضو cluster) انتقلتا من ⬜ إلى ✅ 26ai على مستوى **القيم** (كانتا ROWID فقط).
- الاختبارات: `cargo test --workspace` **114**؛ `golden_cli` **8/8**؛ `golden_lab` **12/12** (اختبار وحدة جديد `unasciistr_decodes_the_export_grammar_exactly` يعمل بلا `ODR_GOLDEN`).
- البيئة: Rust 1.99 مثبّت؛ المجموعات الذهبية الثلاث في `lab/out/golden/{20261018T080131Z,20261021T102638Z,20261021T093052Z}` (SHA-256 مطابق)؛ Docker مثبّت وصورة 26ai **مسحوبة** (`container-registry.oracle.com/database/free:23.26.0.0`) — لم تُشغَّل حاوية بعد.

## 2. ما قيس (أرقام حقيقية من هذه الجلسة)
| القياس | القيمة |
|---|---|
| قيم بايت-ببايت == `DUMP` أوراكل | 141,601 مخزَّنة + 12 (T_ADDCOL NN/NL عبر `materialize`) = **141,613** |
| نصوص قانونية مقارنة | 141,551 (+10,027 عن السابق) |
| نصوص `\XXXX` مُفكَّكة ومطابقة | 10,009 (step g) · 10,007 (end-to-end) · 10,000 (lab2 على 11g و21c) |
| أحمر/أخضر | تعطيل الفكّ ⇒ فشل `T_CL_DEPT.DNAME`؛ حرف واحد مُبدَّل (ا→أ) ⇒ فشل؛ `CLUCOLS=1` ⇒ فشل؛ تبديل tab# ⇒ فشل |
| بايتات C_ODR كتلة 16/2212 | `ntab=3`, `kdbt=[(0,2),(2,2),(4,4)]`؛ مفتاحان `0xac`؛ 6 أعضاء `0x6c`؛ عضو محذوف slot 7 `0x7c`؛ K2 NULL مخزَّن `ff` لأن `cc=2` |

## 3. ما لم يُنجَز (بالترتيب للوكيل القادم)
1. ~~LOB خارج الصف~~ **✅ منجز (LV-070، PR #35)**. **الباقي من M5: فكّ الضغط BASIC** (جدول الرموز في kdbt[0]، حقول 9ir2 — كتلة T_COMPRESS 16/5124 في golden v2؛ ابدأ بفحص البايتات: `40 02 e2 02 ff ff e6 05 24 09 15 00 15 00 00 00 23 00 …`). حدود LOB المعلَنة: LOB > حجم extent واحد في SecureFile (خريطة extents بأكثر من مدخل) وLOB index لم يُختبرا بملف؛ CLOB في قاعدة أحادية البايت بلا ملف.
   - (قديم) الخطة السابقة: قراءة مقطع LOB (BasicFile: فهرس LOB + chunks بعنوان في المحدِّد `05 …` — انظر بايتات id 6 في LV-068؛ SecureFile: inode `40 90` يحمل DBA القطعة `04 00 12 50`). ابدأ بـ BasicFile. معيار القبول: قيمة 6000 في T_LOB_* == `DBMS_LOB.SUBSTR`. **ثم الضغط** (فكّ جدول الرموز 9ir2) — مرفوض صراحة الآن (LV-069).
   - (قديم، منجز) LOB inline: المطلوب: (أ) أضف إلى `lab/sql/10_corpus.sql` جدول `T_LOB (ID, C CLOB, B BLOB)` بقيم inline (< 4000 بايت، منها عربية) وقيمة واحدة > 4000 (out-of-line) + جدول `T_COMPRESS … COMPRESS BASIC`؛ (ب) أعد بناء golden 26ai (runbook §3–§4؛ الصورة مسحوبة بالفعل — `docker volume create odr-oradata` ثم `docker run …`)؛ (ج) ارفعها كـ Release `golden-26ai-<date>` و**حدّث ODR_GOLDEN في CI/الوثائق**؛ (د) افحص بايتات locator الـ LOB (type# 112/113) في الكتلة الحقيقية قبل أي كود: الـ inline LOB يحمل الـ locator (رأس `00 54 00 01 …`) ثم البيانات؛ (هـ) `value.rs`: `ColumnType::Clob/Blob`، فكّ inline فقط؛ out-of-line ⇒ `Unknown("LOB out of line: segment not read yet")` لا NULL؛ المضغوط يبقى `Unsupported` صريحاً. معيار القبول: كل قيمة LOB inline == `DUMP`؛ صف LV لكل ثابت.
   - ملاحظة لتصدير الحقيقة: `20_export_golden.sql` يستخدم `DUMP(col,1016,pos,n)` — على CLOB يعطي `DUMP` بايتات الـ **locator** لا البيانات؛ أضف فرعاً لـ `data_type IN ('CLOB','BLOB')` يصدّر `DBMS_LOB.SUBSTR` (hex عبر `UTL_RAW.CAST_TO_RAW`) **و**الـ locator معاً.
2. **توسيع `odr scavenge`** (INTERVAL/TSTZ/BINARY_*/NCHAR) بشرط hold-out: اضبط على 26ai فقط، ثم قِس على 11g و21c بلا تعديل بعد النظر. الكود في `crates/odr-cli/src/scavenge_cmd.rs` (`Guess`, `fits`, `infer`).
3. تجربة Windows حقيقية (`docs/field/TRIAL_KIT_ar.md`) — ينتظر المالك/الخبير.
4. 19c عند الرخصة؛ قرار `fuzz/corpus/*` غير المتتبَّع — لا يُرفع.

## 4. حدود معلَنة (بلا تغيير عن 2026-10-21)
تصادم checksum 16-بت بلا صف غير دقيق · forge داخل قيمة (0.2%) · torn ⇒ inferred لا verified · scavenge يرفض المسلسل/المهاجر/العنقودي · LOB والضغط غير مدعومين (رفض صريح).

## 5. دروس هذه الجلسة
- `ASCIISTR` يكتب الشرطة المائلة الخلفية `\005C` (أربعة أرقام) لا `\5C` — اختبار الوحدة الأول افترض الخطأ وفشل؛ القاعدة: اقرأ وثيقة الدالة قبل كتابة فاكّها.
- `sed` مع `|` كفاصل يفشل على أسطر Rust فيها `||` — استخدم بايثون للتعديلات المؤقتة في إثبات أحمر/أخضر.

## 6. القاعدة التي لا تُكسر
قيمة خاطئة أسوأ من قيمة مفقودة. كل اختبار LV **يجب أن يفشل بدون الإصلاح** (أثبته فعلياً — 4 طرق هذه الجلسة). لا أرقام بلا قياس على ملف حقيقي.
