# ODR — جرّبها في 10 دقائق (Windows)

> صفحة واحدة. ثلاثة أوامر. كل رقم هنا مقاس فعلاً على الملفات المذكورة (26ai، مجموعة `golden-26ai-20261024`).
> الأداة **لا تكتب أبداً** على ملف المصدر. العُدّة الكاملة في `TRIAL_KIT_ar.md`.

## 0. حمّل (مرة واحدة)

من صفحة **Releases** في المستودع:

| ماذا | الملف | الحجم |
|---|---|---|
| الأداة | `odr-v0.2.0-alpha-x86_64-pc-windows-msvc.zip` (فيه `odr.exe` + `odr-corrupt.exe`) | ≈ 3 MB |
| ملفات التجربة | Release `golden-26ai-20261024` → `odr_golden_26ai_v2_20261024T085903Z.tar.gz` | 62 MB (≈ 480 MB مفكوكة) |

فكّ الاثنين. على Windows يفك `tar -xzf` من PowerShell مباشرة. تحقق من `.sha256` بجانب كل ملف إن أردت.

في `datafiles/` أربعة ملفات أوراكل حقيقية سليمة (أُخذت بعد `SHUTDOWN IMMEDIATE`، وفحصها `dbv` نظيف):
`FREE_FREEPDB1_system01.dbf` (القاموس) · `odr_odr_assm01.dbf` · `odr_odr_mssm01.dbf` · `odr_odr_big01.dbf`.
وفيها `rows.psv`: **147,661 قيمة** كما طبعها أوراكل نفسه — هذه «الحقيقة» التي نقارن بها.

## 1. ثلاثة أوامر

```powershell
# داخل مجلد الـ zip المفكوك. عدّل المسار:
$G = "G:\golden\20261024T085903Z\datafiles"
$S = "$G\FREE_FREEPDB1_system01.dbf"

# (أ) القاموس — أي جداول في هذه الملفات؟
.\odr.exe dict $S $G\odr_odr_assm01.dbf $G\odr_odr_mssm01.dbf $G\odr_odr_big01.dbf
#   تتوقع: "71876 objects · 2563 tables · 0 issues" ثم 18 جدولاً في مخطط ODR
#   (T_JOURNAL, T_LOB_BASIC, T_LOB_SECURE, T_COMPRESS, T_CL_EMP cluster, ...)

# (ب) استخراج جدول نظيف — 20,000 قيد يومية بالعربية
.\odr.exe extract $S $G\odr_odr_assm01.dbf --table ODR.T_JOURNAL --out out\journal
#   تتوقع السطر الأخير: verified 20000 · inferred 0 · rejected 0   ورمز خروج 0

# (ج) جدول فيه CLOB/BLOB؛ الصف 6 مخزَّن خارج الصف (CLOB 6,000 حرف = 12,000 بايت UCS-2 يُجمَّع من كتلتين، BLOB 6,000 بايت من كتلة واحدة)
.\odr.exe extract $S $G\odr_odr_assm01.dbf --table ODR.T_LOB_BASIC --out out\lob
#   تتوقع: verified 6 · inferred 0 · rejected 0
#   افتح out\lob\verified.csv: الصف 2 فيه "فاتورة رقم 1 - مرفق"؛ الصف 6: عمود C = 6000 حرف L، وعمود B = 6000 بايت 4D (يُعرض hex: 12,000 خانة)
#   نفس الشيء لـ --table ODR.T_LOB_SECURE (تخزين SecureFile): verified 6
```

على Linux نفس الأوامر بـ `./odr`.

## 2. اصنع تلفاً وقِس بنفسك (دقيقتان)

```powershell
mkdir dmg
# 400 كتلة عشوائية فيها bit rot — حتمي: البذرة 7 تعطي نفس التلف دائماً. الأصل لا يُمسّ.
.\odr-corrupt.exe --input $G\odr_odr_assm01.dbf --output dmg\bitrot.dbf --block-size 8192 --seed 7 --op bitrot:400
.\odr.exe extract $S dmg\bitrot.dbf --table ODR.T_JOURNAL --out out\bitrot
#   تتوقع: verified 18744 · rejected 1256   ورمز خروج 2 (= فيه مرفوض، وهذا طبيعي)
#   كل سطر في rejected.csv ينتهي بالسبب: block integrity: ChecksumFail

# بِت واحد مقلوب داخل الكتلة 3935 (أولى كتلتي الـ CLOB الكبير في الصف 6)
.\odr-corrupt.exe --input $G\odr_odr_assm01.dbf --output dmg\lobchunk.dbf --block-size 8192 --seed 7 --op bitrot:1 --blocks 3935:3935
.\odr.exe extract $S dmg\lobchunk.dbf --table ODR.T_LOB_BASIC --out out\lobchunk
#   تتوقع: verified 5 · rejected 1 — الصف 6 وحده مرفوض وسببه:
#   column 2: LOB out of line could not be assembled: Integrity { rdba: 67112799, status: ChecksumFail } (LV-070)
#   (لا نُخرج LOB ناقصاً أبداً؛ باقي أعمدة الصف تبقى في السطر المرفوض لتراها)
```

**الاختبار الحاسم:** قارن `out\bitrot\verified.csv` مع `out\journal\verified.csv`. كل سطر في التالف **موجود حرفياً** في النظيف — أي سطر زائد = قيمة خاطئة بوسم verified. المقاس على هذه البذرة: **صفر**.

```powershell
Compare-Object (Get-Content out\journal\verified.csv) (Get-Content out\bitrot\verified.csv) | Where-Object SideIndicator -eq "=>"
#   تتوقع: لا شيء
```

## 3. كيف تقرأ ما خرج

| الملف | معناه |
|---|---|
| `verified.csv` | الكتلة سليمة والقيمة مطابقة لأوراكل بايتاً ببايت. الأعمدة الثلاثة الأولى `_file,_block,_slot` = موضع الصف الفيزيائي (ROWID). |
| `inferred.csv` | قيمة مستنتجة بقاعدة معلنة، والسبب في عمود `_inference` (مثل `column 6: Default` لعمود أُضيف بـ ADD COLUMN DEFAULT، أو صف تحت حدّ تمزّق الكتلة). |
| `rejected.csv` | لا نستطيع إثباتها، والسبب في `_reason`. |
| `report.json` | SHA-256 لكل ملف مدخل، المقطع وامتداداته، عدّ الطبقات، الكتل المتخطاة وأسبابها، الترميز. |

رموز الخروج: `0` كل شيء verified/inferred · `2` يوجد مرفوض · `1` رُفض الطلب كله (مثلاً جدول مضغوط).

## 4. ما ترفضه الأداة اليوم صراحةً (جرّبه لترى الرفض لا التخمين)

```powershell
.\odr.exe extract $S $G\odr_odr_assm01.dbf --table ODR.T_COMPRESS --out out\compress
#   تتوقع رمز خروج 1 و:
#   ODR.T_COMPRESS: 3 data block(s) are BASIC/OLTP compressed (kdbh flag 0x40) — table compression is not supported yet (M5); refusing rather than guessing (LV-069)
```

كذلك: TDE (يحتاج wallet)، Hash cluster/IOT، وتزوير قيمة داخل كتلة مع checksum صحيح (حدّ معلَن، يحتاج فهارس — M6). بلا ملف SYSTEM استخدم `odr scavenge` (كل شيء يخرج inferred، لا verified).

## 5. ما نرجوه منك

1. ملف `.dbf` تالف **حقيقي** (أي إصدار) مع ما تعرفه عن تلفه — نُحوّله إلى اختبار ذهبي.
2. إصدارات أوراكل وترميزاتها عند عملائك (AR8MSWIN1256؟ 11gR2؟ 19c؟).
3. الشكل النهائي الذي تريده للمخرجات (CSV؟ SQL*Loader؟ إدخال مباشر؟).
