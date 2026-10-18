# البحث 05 — مصادر مخطط القاموس وخرائط المقاطع (2026-10-18)

**الغرض:** الأدلة الثنائية لمرحلة L-6b (القاموس). كل بند مربوط بـ `lab-verify`.

## 1. `kcvfh` — رأس ملف البيانات (LV-035) — مُنفَّذ في `dict/kcvfh.rs`
- المصدر T3: BBED `map /v` + `dump` كامل لـ 11.2.0.4 `system01.dbf` كتلة 1 (cnblogs yhq1314 2019) مع تحقق المؤلف نفسه من `x$kcvfh.fhrdb`، `v$datafile.creation_change#`، `checkpoint_change#`، `ts#`، `rfile#`.
- الحقول المستخدمة ثابتة 10g–19c (V10 STYLE): cvn@24، dbid@28، dbname@32، fsz@44، bsz@48، fno@52، typ@54، **rdb@96**، crs@100، crt@108، sta@138، cpc@140. ما بعد @332 (tsn/tln/tnm/rfn/ckp@484) اختياري.
- 9i (LV-036): `root dba` يشير إلى مقطع التوافق `0x0D`، وعنوان bootstrap$ في مدخلة `BOOTSTRP` (eygle 2007). خارج النطاق.

## 2. مخطط أعمدة الجداول الأساسية — `sql.bsq` 9.2 (T1، نسخة منشورة كاملة، blog.naver hjc1108)
ترتيب الأعمدة (col#) هو ترتيب التخزين في المقطع لهذه الجداول (لا أعمدة افتراضية/محذوفة فيها). **القاعدة (LV-038): يُقرأ المخطط من نص `bootstrap$` في الملف نفسه؛ القائمة أدناه مرجع للاختبار فقط.**

| الجدول | الأعمدة بالترتيب (9iR2) | التخزين |
|---|---|---|
| `OBJ$` (obj# 18) | obj#, dataobj#, owner#, name, namespace, subname, type#, ctime, mtime, stime, status, remoteowner, linkname, flags, oid$, spare1..3, spare4(vc), spare5(vc), spare6(date) | مقطع مستقل (1,121) |
| `TAB$` (obj# 4) | obj#, dataobj#, ts#, file#, block#, bobj#, tab#, cols, clucols, pctfree$, pctused$, initrans, maxtrans, flags, audit$, rowcnt, blkcnt, empcnt, avgspc, chncnt, avgrln, avgspc_flb, flbcnt, analyzetime, samplesize, degree, instances, intcols, kernelcols, property, trigflag, spare1..3, spare4(vc), spare5(vc), spare6(date) | `C_OBJ#` tabno 1 |
| `COL$` (obj# 21) | obj#, col#, segcol#, segcollength, offset, name, type#, length, fixedstorage, precision#, scale, null$, deflength, default$(LONG), intcol#, property, charsetid, charsetform, spare1..3, spare4(vc), spare5(vc), spare6(date) | `C_OBJ#` tabno 5 |
| `CLU$` (obj# 5) | obj#, dataobj#, ts#, file#, block#, cols, pctfree$, pctused$, initrans, maxtrans, size$, hashfunc, hashkeys, func, extind, flags, degree, instances, avgchn, spare1..4, spare5(vc), spare6(vc), spare7(date) | `C_OBJ#` tabno 2 |
| `USER$` (obj# 22) | user#, name, type#, password, datats#, tempts#, ctime, ptime, exptime, ltime, resource$, audit$, defrole, defgrp#, defgrp_seq#, spare, astatus, lcount, defschclass, ext_username, spare1..3, spare4(vc), spare5(vc), spare6(date) | `C_USER#` tabno 1 |
| `TS$` (obj# 16) | ts#, name, owner#, online$, contents$, undofile#, undoblock#, blocksize, inc#, scnwrp, scnbas, dflminext, dflmaxext, dflinit, dflincr, dflminlen, dflextpct, dflogging, affstrength, bitmapped, plugged, directallowed, **flags** (0x20 = ASSM)، pitrscnwrp, pitrscnbas, ownerinstance, backupowner, groupname, spare1, spare2, spare3(vc), spare4(date) | `C_TS#` |
| `SEG$` (obj# 14) | file#, block#, type#, ts#, blocks, extents, iniexts, minexts, maxexts, extsize, extpct, user#, lists, groups, bitmapranges, cachehint, scanhint, hwmincr, **spare1** (0x800 مضغوط…), spare2 | `C_FILE#_BLOCK#` |

أعلام حرجة: `TAB$.PROPERTY 0x400` = جدول cluster؛ `TAB$.FLAGS 0x00800000` = ROWDEPENDENCIES؛ `COL$.PROPERTY 0x4000` dropped، `0x8000` unused (البيانات باقية ← تُحتسب في segcol#)، `0x10000` virtual (segcol# = 0)؛ `0x40000000` = ADD COLUMN DEFAULT دون كتابة الصفوف (11g+؛ LV-023) ← **ممنوع إخراج NULL؛ إمّا القيمة من `ECOL$`/`DEFAULT$` موسومة `inferred` أو `unknown` صريح**.

## 3. خريطة كائنات bootstrap (10g، eygle 2007) — LV-027/033
SYSTEM rbs (1,9) · C_OBJ# obj 2 (1,25) · C_TS# 6 (1,57) · C_FILE#_BLOCK# 8 (1,73) · C_USER# 10 (1,89) · UNDO$ 15 (1,105) · FILE$ 17 (1,113) · OBJ$ 18 (1,121) · BOOTSTRAP$ 56 (1,377). داخل C_OBJ#: TAB$=tabno 1، CLU$=2، IND$=3، ICOL$=4، COL$=5، LOB$=6، COLTYPE$=7. (11g: bootstrap$ obj 59، (1,520) — يُؤخذ من `kcvfhrdb` لا يُثبّت.)

## 4. رأس المقطع وخرائط الـ extents (للمرحلة L-6b.2)
- **MSSM (نوع 0x10)** — BBED `map /v` (eygle، minniebaby، cnblogs): `ktech` @20 (72 بايت): spare1@20، tsn@24، lastmap@28، mapcount@32، extents@36، blocks@40، mapend u16@44، hwmark@48 (32)، locker@80، flag@88؛ `ktemh` @92: count@92، next@96، obj@100، flag@104؛ `ktetb[]` @108: (dba u32، nbk u32) × extents. T2 (أربع تفريغات متطابقة).
- **ASSM (نوع 0x23 Pagetable Segment Header)** — modb.pro 44559 + CSDN renyanjie123 (12.2): `Map Header` (next/#extents/obj#/flag) عند @256..@271 (**obj# @272**، تأكّد: تعديل البايت @272 غيّر `obj#` في التفريغ)؛ **Extent Map من @280** (dba u32، len u32) حتى 307 مدخلة؛ `L2 Array start offset 0x1434`؛ عند > 307 extents يشير `next` إلى كتلة **0x24 (EMB)**: رأس: exts@20، next@24، blks@28، flag@32، ثم Extent Map من @36 (dba،len) حتى 508 مدخلة، وAuxillary Map من @4100. T2 (تفريغتان بالبايت).
- الحالة: ⬜ كلها `lab-verify`؛ الاختبار الذهبي للمرحلة القادمة يُبنى من بايتات التفريغين أعلاه.

## 5. المصادر
- sql.bsq 9.2 (نسخة كاملة): https://blog.naver.com/hjc1108/60019581749
- eygle 2007 *Oracle中独一无二的Cache对象*: https://www.eygle.com/archives/2007/04/cache_bootstrap.html
- cnblogs yhq1314 2019 *bbed修复system文件头损坏*: https://www.cnblogs.com/yhq1314/p/10870998.html
- eygle 2010 V$TYPE_SIZE/ktech map: https://www.eygle.com/archives/2010/08/vtype_size_ktuxc.html
- modb.pro 2021 *PAGETABLE EXTENT MAP BLOCK*: https://www.modb.pro/db/44559
- CSDN renyanjie123 2022 *bbed truncate 恢复*: https://blog.csdn.net/renyanjie123/article/details/122929522
- renenyffenegger: COL$ / bootstrap tables / dcore.bsq notes.
