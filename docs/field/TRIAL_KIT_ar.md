# عدة التجربة الميدانية — ODR (Oracle Datafile Recovery)

> لمن يريد أن **يجرّب بنفسه** على ملفات أوراكل حقيقية سليمة وتالفة، على Windows أو Linux، بلا تثبيت أوراكل.
> كل أمر هنا نُفِّذ فعلاً. الأداة **لا تكتب أبداً** على ملف المصدر (تحسب SHA-256 قبل وبعد).
> **مستعجل؟** ابدأ بـ `QUICKSTART_ar.md` (صفحة واحدة، ثلاثة أوامر، 10 دقائق).

## 1. الفكرة في دقيقتين (نص يصلح أن يُقرأ صوتياً)

أوراكل يخزّن الجداول في ملفات `.dbf` على القرص، مقسّمة إلى كتل (8 KB عادة). كل كتلة فيها رأس، وقائمة صفوف، وقيم مكتوبة بصيغة أوراكل الداخلية (الأرقام والتواريخ والنصوص لها ترميز خاص). القاعدة نفسها تعرف أسماء الجداول والأعمدة من «القاموس» المخزَّن في ملف `SYSTEM`.

عندما تتلف كتلة واحدة، أوراكل يرفض الجدول كله (`ORA-01578`) أو لا يفتح القاعدة أصلاً. والعميل غالباً بلا نسخة احتياطية صالحة.

**ODR** يقرأ الملفات مباشرة بلا أوراكل: يفكّ القاموس من `SYSTEM`، يمشي على امتدادات كل جدول، يفحص كل كتلة (checksum + ذيل + سبعة ثوابت بنيوية)، ويخرج كل صف في **طبقة واحدة من ثلاث**:
- **verified** — الكتلة سليمة والقيمة مطابقة لما يطبعه أوراكل بايتاً ببايت (أثبتناه على 141,581 قيمة).
- **inferred** — القيمة مستنتجة بقاعدة معلنة (مثلاً عمود أُضيف بـ `DEFAULT` بعد إنشاء الصف، أو ملف رأسه ممسوح وأُعطيت هويته يدوياً)؛ كل سطر يحمل سبب الاستنتاج.
- **rejected** — لا نستطيع إثبات القيمة (كتلة تالفة، بنية مكسورة)؛ كل سطر يحمل السبب.

**القاعدة الواحدة:** القيمة الخاطئة أسوأ من الناقصة. لا تخمين صامت، لا NULL افتراضي.

ما يختلف عن أوراكل: أوراكل يتوقف عند أول كتلة تالفة؛ ODR يتخطاها ويخرج ما حولها، ويسجّل بالضبط ما خسرناه ولماذا.

## 2. التنزيل

من صفحة Releases في المستودع:
- `odr-<version>-x86_64-pc-windows-msvc.zip` (فيه `odr.exe` و`odr-corrupt.exe`)
- `odr-<version>-x86_64-unknown-linux-gnu.tar.gz`
- ملف `.sha256` بجانب كل أرشيف — تحقق منه.

**ملفات التجربة** (سليمة + حقيقة أوراكل عنها)، كل مجموعة ≈ 60 MB مضغوطة / 480 MB مفكوكة:

| Release | الإصدار | ملف SYSTEM | ملفات الجداول |
|---|---|---|---|
| `golden-26ai-20261018` | Oracle AI Database 26ai Free (23.26) | `FREE_FREEPDB1_system01.dbf` (ملف 12، bigfile) | `odr_odr_assm01/mssm01/big01.dbf` (16/17/18) |
| `golden-21c-20261021` | Oracle 21c XE (21.3) | `XE_XEPDB1_system01.dbf` (ملف 9) | `odr_odr_assm01/mssm01/big01.dbf` (20/21/22) |
| `golden-11g-20261021` | Oracle 11g XE (11.2, non-CDB) | `XE_system.dbf` (ملف 1) | `odr_odr_assm01/mssm01/big01.dbf` |
| **`golden-26ai-20261024`** (الأحدث، v2) | 26ai Free | `FREE_FREEPDB1_system01.dbf` (ملف 12) | نفس الثلاثة (16/17/18) **+ جداول LOB وضغط**: `T_LOB_BASIC`، `T_LOB_SECURE`، `T_COMPRESS` — 147,661 قيمة |

داخل كل مجموعة: `datafiles/` (النسخ السليمة بعد `SHUTDOWN IMMEDIATE`، `dbv` نظيف)، `rows.psv` (كل قيمة كما طبعها أوراكل بـ `DUMP`)، `meta/rowids.psv` (ROWID كل صف)، `meta/rowcounts.txt`، تفريغات كتل (`traces/`).

الجداول: 15 جدولاً في مخطط `ODR` تغطي NUMBER/DATE/TIMESTAMP/نصوص عربية/RAW/NULLs/صفوف متسلسلة ومهاجرة/ROWDEPENDENCIES/cluster/ADD COLUMN DEFAULT/MSSM/bigfile، وأكبرها `T_JOURNAL` (20,000 قيد يومية بالعربية).

## 3. أول تجربة (5 دقائق)

```powershell
# Windows PowerShell — داخل مجلد الـ zip المفكوك، ومجلد golden مفكوك في G:\golden\20261018T080131Z
$G = "G:\golden\20261018T080131Z\datafiles"

# أ) ماذا في الملف؟ (قراءة فقط)
.\odr.exe scan $G\odr_odr_assm01.dbf

# ب) القاموس: أي جداول وأعمدة
.\odr.exe dict $G\FREE_FREEPDB1_system01.dbf $G\odr_odr_assm01.dbf $G\odr_odr_mssm01.dbf $G\odr_odr_big01.dbf

# ج) استخراج جدول
.\odr.exe extract $G\FREE_FREEPDB1_system01.dbf $G\odr_odr_assm01.dbf --table ODR.T_JOURNAL --out out\journal
# → out\journal\verified.csv (20,000 صف) · inferred.csv · rejected.csv · report.json
```

على Linux نفس الأوامر بـ `./odr`.

## 4. اصنع تلفاً بنفسك وقِس

`odr-corrupt` يصنع نسخة تالفة **حتمية** (نفس البذرة = نفس التلف) ويكتب مانيفست بكل بايت غُيِّر. لا يلمس الأصل.

```powershell
# bit rot في 400 كتلة عشوائية
.\odr-corrupt.exe --input $G\odr_odr_assm01.dbf --output dmg\bitrot.dbf --block-size 8192 --seed 7 --op bitrot:400
# كتابة ممزقة (نصف الكتلة قديم ونصفها جديد) في 200 كتلة — داخل امتداد T_JOURNAL فقط
.\odr-corrupt.exe --input $G\odr_odr_assm01.dbf --output dmg\torn.dbf --block-size 8192 --seed 7 --op torn:200 --blocks 3204:3327
# فدية: أول 16 KB + كل عاشر 8 KB مشفَّر
.\odr-corrupt.exe --input $G\odr_odr_assm01.dbf --output dmg\ransom.dbf --block-size 8192 --seed 7 --op ransom:16384:8192:10
# تزوير: بايت واحد يتغير و checksum يُعاد حسابه (الأصعب)
.\odr-corrupt.exe --input $G\odr_odr_assm01.dbf --output dmg\forge.dbf --block-size 8192 --seed 7 --op forge:60 --blocks 3204:3327
# كتل متبادلة (كتابة في المكان الخطأ)
.\odr-corrupt.exe --input $G\odr_odr_assm01.dbf --output dmg\swap.dbf --block-size 8192 --seed 7 --op swap:20 --blocks 3204:3327
# رأس الملف ممسوح
.\odr-corrupt.exe --input $G\odr_odr_assm01.dbf --output dmg\wiped.dbf --block-size 8192 --seed 7 --op wipe-header
```

ثم استخرج من النسخة التالفة وقارن بالنظيفة:

```powershell
.\odr.exe extract $G\FREE_FREEPDB1_system01.dbf dmg\bitrot.dbf --table ODR.T_JOURNAL --out out\bitrot
# رمز الخروج 2 = هناك صفوف مرفوضة (طبيعي). 1 = رفض الملف كله (مثلاً رأس ممسوح بلا --assume).

# الرأس الممسوح: الأداة ترفض حتى تعطيها هوية الملف صراحة، وعندها كل الصفوف inferred لا verified
.\odr.exe extract $G\FREE_FREEPDB1_system01.dbf dmg\wiped.dbf --table ODR.T_JOURNAL --out out\wiped --assume "dmg\wiped.dbf=file:16,ts:7,rfile:16,bs:8192"
```

**ما تتوقعه (مقاس على 26ai، بذرة 7):**

| التلف | verified صحيحة | خاطئة بوسم verified | ماذا يحدث للباقي |
|---|---|---|---|
| bitrot 400 | 18,735 | **0** | 1,265 rejected بسبب `block integrity: ChecksumFail` |
| torn 200 | 19,721 | **0** | 279 rejected (`TailMismatch`) |
| zero 200 | 19,166 | **0** | 834 في كتل مصفَّرة (في `report.json` → `skipped_blocks`) |
| swap 20 | ~15,000 | **0** | الكتل المتبادلة كلها مرفوضة: `header says block X but read at Y` |
| ransom | 0 verified، 17,906 inferred | **0** | الكتل المشفَّرة `integrity: Foreign` |
| forge 60 | ~18,000 | **≈ 0.2%** | التزوير داخل قيمة عمود مع checksum صحيح **لا يُكشف من الكتلة وحدها** — هذا حدّ معلَن (انظر §6) |

للمقارنة الآلية: اطرح `verified.csv` التالف من النظيف — أي سطر زائد = قيمة خاطئة. (في المستودع سكربت الاختبار الذهبي يفعل ذلك.)

## 5. كيف تقرأ المخرجات

- `verified.csv`: `_file,_block,_slot,<الأعمدة…>` — الثلاثة الأولى هي موضع الصف الفيزيائي (يعادل ROWID).
- `inferred.csv`: نفسه + عمود `_inference` بالسبب (مثلاً `column 6: Default` أو `file identity from --assume`).
- `rejected.csv`: نفسه + `_reason` (`block integrity: …`، `block structure: …`، `row did not decode: …`، `column 4 not exact: …`).
- `report.json`: SHA-256 لكل ملف مدخل، المقطع (رأسه، امتداداته)، عدّ الطبقات، الكتل المتخطاة وأسبابها، مشاكل القاموس، الترميز المستخدم.

## 6. ما لا تفعله الأداة اليوم (بصراحة)

- **LOB** (CLOB/BLOB): مدعوم — داخل الصف وخارجه (BasicFile وSecureFile)؛ يُجمَّع من كتل مقطع الـ LOB ولا يُخرَج ناقصاً أبداً: أي كتلة تالفة أو طولٌ لا يطابق المحدِّد ⇒ الصف مرفوض بالسبب (LV-068/070). مقاس على 26ai فقط.
- **الضغط** (BASIC/OLTP): يُرفض الجدول كله صراحةً (رمز خروج 1، LV-069)، لا يُخمَّن. (قيد العمل — M5 ج)
- **بلا SYSTEM** (القاموس مفقود): `odr scavenge <dbf> --out out\scav` يُخرج `OBJ_<id>/inferred.csv` بأعمدة `COL_<n>` ونوع مستدل موسوم (LV-066)؛ **لا شيء فيه verified** لأن لا قاموس يشهد. أنواع INTERVAL/TSTZ/BINARY_*/NCHAR لم تُضف بعد للمرشّحين.
- **TDE** (التشفير الحقيقي): يحتاج wallet العميل؛ خارج النطاق.
- **تزوير داخل قيمة مع checksum صحيح**: غير قابل للكشف من الكتلة وحدها؛ يحتاج تحققاً عبر الفهارس (M6).
- **مؤكَّد على**: 26ai و21c و11g (little-endian). LOB والضغط مقاسان على 26ai فقط؛ 19c يحتاج رخصة.
- **Hash clusters وIOT**: رفض صريح.

## 7. ما نحتاجه من المجرّب

1. ملفات `.dbf` تالفة **حقيقية** (من بيئة اختبار أو عميل بموافقته) — أي إصدار؛ مع ما يعرفه عن التلف.
2. أسماء الإصدارات والترميزات الشائعة عند عملائه (AR8MSWIN1256؟ 11gR2؟ Windows Server؟).
3. ماذا يريد كمخرج نهائي: CSV؟ ملفات SQL*Loader؟ إدخال مباشر في قاعدة جديدة؟

كل ملاحظة تُسجَّل في `docs/research/` وتتحول إلى اختبار ذهبي.
