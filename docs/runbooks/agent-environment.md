# Runbook — إعداد بيئة الوكيل من الصفر (مجرَّب 2026-10-18؛ آخر تحديث بعد PR #21)

> لأي وكيل جديد في sandbox جديد (Debian 13، 4 أنوية، 8 GB RAM، 29 GB قرص، لا Docker مثبّت، `sudo` بلا كلمة سر).
> كل أمر هنا نُفِّذ فعلاً ونجح. المدة الكلية من الصفر إلى أول مجموعة ذهبية ≈ 35 دقيقة.

## 0. الحقائق التي ستوفّر عليك ساعة
- `docker: command not found` **لا يعني** أن Docker مستحيل. `apt-get install docker.io` يعمل، لكن **لا يوجد iptables/bridge** في الـ sandbox ← شغّل الـ daemon بـ `"iptables": false, "bridge": "none"` واستخدم `--network host` لكل حاوية.
- `/mnt/aidrive` **للقراءة فقط** في هذه البيئات ← النسخ الدائمة تذهب إلى **GitHub Releases** (`gh release create`).
- القرص 29 GB: صورة Oracle 10 GB + حجم البيانات 4.5 GB + `target/` 1.3 GB. **لا تنسخ كل datafiles** (3.6 GB)؛ انسخ فقط `FREE_FREEPDB1_system01.dbf` + `odr_*.dbf` (≈ 500 MB).
- توكن GitHub: يحتاج `repo` + `workflow`. التوكن في `~/.git-credentials` قد **ينتهي** أثناء الجلسة ← أعد `setup_github_environment` أو اكتب التوكن يدوياً (القسم 2).
- التحذيرات المرفوضة بـ `-D warnings` تشمل `clippy::pedantic` كاملاً: ضع الاختبارات في ملفات `tests/` مع `#![allow(...)]` موثّق في أعلى الملف.

## 1. Rust (3 دقائق)
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh
sh /tmp/rustup.sh -y --default-toolchain 1.99.0 --profile minimal -c rustfmt -c clippy
source ~/.cargo/env
cd /home/user/webapp && cargo build --workspace     # ≈ 15 s
```
البوابات (كلها قبل أي PR):
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
python3 tools/gen_charsets.py --check
# مع المجموعة الذهبية (القسم 4):
ODR_GOLDEN=$PWD/lab/out/golden/<run> cargo test -p odr-core --test golden_lab -- --nocapture
```

## 2. GitHub
```bash
# كل أمر Bash يبدأ من /home/user — ابدأ دائماً بـ cd /home/user/webapp
T=<token>            # repo + workflow
printf 'https://moain2026:%s@github.com\n' "$T" > ~/.git-credentials && chmod 600 ~/.git-credentials
git config --global credential.helper store
echo "$T" | gh auth login --with-token     # تحذير read:org غير مهم
git fetch origin && git checkout main && git pull
```
سير العمل: فرع `lead/<feature>` ← commit ← `gh pr create --base main` ← بوابات خضراء ← `gh pr merge N --merge`. CI مفعّل (`.github/workflows/ci.yml`) على Windows + Linux ويعمل على `main`، `lead/**`، `agent/**` وكل PR.

## 3. Docker + Oracle (15–20 دقيقة أول مرة)
```bash
sudo apt-get update -qq && DEBIAN_FRONTEND=noninteractive sudo apt-get install -y -qq docker.io iptables
sudo mkdir -p /etc/docker
echo '{"storage-driver":"overlay2","iptables":false,"bridge":"none","data-root":"/home/user/docker-data"}' | sudo tee /etc/docker/daemon.json
(sudo dockerd > /tmp/dockerd.log 2>&1 &) ; sleep 8 ; sudo docker info | grep "Server Version"
# غلاف حتى تعمل سكربتات lab.sh التي تستدعي docker بلا sudo:
printf '#!/bin/sh\nexec sudo /usr/bin/docker "$@"\n' | sudo tee /usr/local/bin/docker >/dev/null && sudo chmod +x /usr/local/bin/docker
export PATH=/usr/local/bin:$PATH
# الصورة الرسمية (بلا تسجيل دخول؛ ≈ 10 GB، 5–10 دقائق):
nohup docker pull container-registry.oracle.com/database/free:23.26.0.0 > /home/user/pull.log 2>&1 &
```
تشغيل القاعدة (**--network host** إلزامي هنا؛ الصورة تحوي قاعدة جاهزة فتصبح healthy خلال ≈ 3 دقائق):
```bash
docker volume create odr-oradata
docker run -d --name odr-oracle --network host --shm-size=2g \
  -e ORACLE_PWD=OdrLab2026x -e ORACLE_CHARACTERSET=AL32UTF8 \
  -v odr-oradata:/opt/oracle/oradata container-registry.oracle.com/database/free:23.26.0.0
until [ "$(docker inspect -f '{{.State.Health.Status}}' odr-oracle)" = healthy ]; do sleep 10; done
```
> إن انتهت الجلسة: الحاوية والحجم يبقيان على القرص إن بقيت البيئة؛ وإلا أعد الخطوات (الصورة تُسحب من جديد).

## 4. المختبر (lab/) — الأوامر الفعلية التي نجحت
```bash
cd /home/user/webapp/lab && export PATH=/usr/local/bin:$PATH
./scripts/lab.sh corpus          # tablespaces + 15 جدول + cluster (≈ 10 s)
```
إن فشل `corpus` في منتصفه (`ORA-00955 name is already used`) احذف المخطط ثم أعد:
```bash
docker exec -i odr-oracle bash -lc "sqlplus -s / as sysdba" <<'EOF'
ALTER SESSION SET CONTAINER = FREEPDB1;
BEGIN
  FOR t IN (SELECT cluster_name FROM dba_clusters WHERE owner='ODR') LOOP EXECUTE IMMEDIATE 'DROP CLUSTER ODR.'||t.cluster_name||' INCLUDING TABLES CASCADE CONSTRAINTS'; END LOOP;
  FOR t IN (SELECT table_name FROM dba_tables WHERE owner='ODR') LOOP EXECUTE IMMEDIATE 'DROP TABLE ODR.'||t.table_name||' CASCADE CONSTRAINTS PURGE'; END LOOP;
END;
/
EXIT
EOF
```
التصدير الذهبي — **لا تستخدم `lab.sh golden` كما هو** (ينسخ 3.6 GB ويملأ القرص). نفّذ خطواته يدوياً:
```bash
docker cp sql/. odr-oracle:/tmp/odr/
docker exec -i odr-oracle bash -lc 'rm -rf /tmp/odr/out && mkdir -p /tmp/odr/out/meta && cd /tmp/odr && sqlplus -s / as sysdba @20_export_golden.sql FREEPDB1 /tmp/odr/out'   # ≈ 55 s، rows.psv ≈ 141k سطر
docker exec -i odr-oracle bash -lc 'cd /tmp/odr && sqlplus -s / as sysdba @30_block_dumps.sql FREEPDB1'
docker exec -i odr-oracle bash -lc 'mkdir -p /tmp/odr/out/traces && cp $ORACLE_BASE/diag/rdbms/*/*/trace/*odr_dump*.trc /tmp/odr/out/traces/'
echo "SHUTDOWN IMMEDIATE" | docker exec -i odr-oracle bash -lc "sqlplus -s / as sysdba"
docker exec odr-oracle bash -lc "bash /tmp/odr/collect.sh /tmp/odr/out"
docker exec odr-oracle bash -lc 'cd /tmp/odr/out/datafiles && ls | grep -vE "FREE_FREEPDB1_system01|odr_odr_" | xargs rm -f'   # أبقِ 4 ملفات فقط
run=$(date -u +%Y%m%dT%H%M%SZ); out=out/golden/$run; mkdir -p $out
docker cp odr-oracle:/tmp/odr/out/. $out/ && sudo chown -R user:user $out
docker image inspect --format '{{.Id}} {{index .RepoDigests 0}}' container-registry.oracle.com/database/free:23.26.0.0 > $out/meta/image.txt
( cd $out && find . -type f -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS )
docker start odr-oracle    # لإعادة الاستخدام
```
**بديل أسرع:** حمّل المجموعة الجاهزة من GitHub Release (62 MB):
```bash
cd /home/user/webapp/lab && mkdir -p out/golden && cd out/golden
gh release download golden-26ai-20261018 --repo moain2026/oracle-datafile-recovery -p '*.tar.gz' -p '*.sha256'
sha256sum -c odr_golden.sha256 && tar -xzf odr_golden_26ai_*.tar.gz && ls
```

## 5. حقائق هذه المجموعة الذهبية (run `20261018T080131Z`)
| | |
|---|---|
| الإصدار | Oracle AI Database 26ai Free 23.26.0.0، AL32UTF8، PDB `FREEPDB1` |
| SYSTEM الخاص بالـ PDB | ملف **12**، **bigfile** (`rfile# 1024`)، MSSM، 37,120 كتلة، `kcvfhrdb` = 520 |
| ODR_ASSM / ODR_MSSM | ملفان 16 / 17، smallfile (rfile 16/17)، 8,192 كتلة |
| ODR_BIG | ملف 18، bigfile |
| bootstrap$ | obj# 59، رأس المقطع 12/520، البيانات 521–527، 60 صفاً |
| OBJ$ / C_OBJ# / C_USER# / C_TS# | رؤوس المقاطع 240 / 144 / 208 / 176 (نص bootstrap يقول `FILE 1024`) |
| أعمدة 26ai | OBJ$ 37، TAB$ 45، COL$ 37، USER$ 35 |
| الملفات | `meta/{database,datafiles,segments,extents,columns,rowcounts}.txt`، `meta/rowids.psv`، `rows.psv` (`TABLE|ID|COL|ROWID|Typ= Len=|CharacterSet|hex,bytes|canonical`)، `traces/*.trc`، `dbv/*.txt`، `datafiles/*.dbf`، `SHA256SUMS` |

## 6. مشاكل واجهتها وحلولها (حتى لا تُعاد)
| العَرَض | السبب | الحل |
|---|---|---|
| `docker pull` يتجاوز مهلة أداة Bash (120 s) | الصورة 10 GB | `nohup … &` ثم راقب `pull.log` |
| `ORA-03405 End of query reached` في corpus | تعليق `--` بعد `;` في سطر عبارة متعددة الأسطر | أزلت التعليقات الداخلية (في `10_corpus.sql`) |
| `ORA-12704 character set mismatch` | `UNISTR()` (NCHAR) مع `'Payment'` (CHAR) في `CASE` | `TO_CHAR(UNISTR(...))` |
| `ORA-01489` في التصدير | `DUMP(col,1016)` لعمود 4000 بايت يتجاوز 4000 حرف | التصدير الآن PL/SQL بـ CLOB وقطع `DUMP(col,1016,pos,1000)` |
| `no space left on device` أثناء `docker cp` | نسخ 3.6 GB datafiles | أبقِ 4 ملفات فقط (أعلاه) |
| `git push` يفشل `Invalid username or token` | انتهاء توكن المنصة المؤقت | اكتب توكن المالك في `~/.git-credentials` |
| `golden_lab` يفشل على bootstrap$ | **اكتشاف حقيقي**: طول `0xFE` بترتيب الملف لا BE | أُصلح في `row/piece.rs` (LV-009) |

## 7. تجارب التلف (odr-corrupt) — كيف قيست precision/recall في LV-059/060
```bash
cargo build --release -p odr-cli -p odr-corrupt
G=$PWD/lab/out/golden/20261018T080131Z; SYS=$G/datafiles/FREE_FREEPDB1_system01.dbf; A=$G/datafiles/odr_odr_assm01.dbf
mkdir -p /tmp/dmg
# --blocks FIRST:LAST يستهدف امتداد الجدول (T_JOURNAL في assm: 3204:3327) حتى تصيب كل عملية الجدول فعلاً
for op in zero:200 bitrot:400 holes:20:16 torn:200 wipe-header "ransom:16384:8192:10" forge:50 swap:20; do
  n=$(echo "$op" | tr ':' '_'); ./target/release/odr-corrupt --input $A --output /tmp/dmg/$n.dbf --block-size 8192 --seed 7 --op "$op"
done
./target/release/odr extract $SYS $A --table ODR.T_JOURNAL --out /tmp/dmg/out_clean                 # الحقيقة المرجعية
./target/release/odr extract $SYS /tmp/dmg/bitrot_400.dbf --table ODR.T_JOURNAL --out /tmp/dmg/out_bitrot   # exit 2 = صفوف مرفوضة
# precision(verified) = |verified ∩ clean| / |verified| ؛ recall = |verified| / 20000
python3 -c "
clean=set(open('/tmp/dmg/out_clean/verified.csv').read().splitlines()[1:])
v=set(open('/tmp/dmg/out_bitrot/verified.csv').read().splitlines()[1:])
print('precision', len(v&clean)/len(v), 'recall', len(v)/len(clean), 'wrong', len(v-clean))"
# رأس ممسوح: بلا --assume ⇒ exit 1؛ معه ⇒ كل الصفوف inferred
./target/release/odr extract $SYS /tmp/dmg/wipe-header.dbf --table ODR.T_JOURNAL --out /tmp/dmg/out_wh \
  --assume "/tmp/dmg/wipe-header.dbf=file:16,ts:7,rfile:16,bs:8192"
```
الاختباران الذهبيان `damaged_copies_never_yield_a_wrong_verified_cell` و`wiped_header_requires_assume_and_downgrades_to_inferred` يفعلان ذلك آلياً داخل العملية (`odr_corrupt::apply`) — أي نمط جديد يُضاف هناك، لا كتجربة يدوية فقط.

## 7b. مختبر إصدار ثانٍ بلا رخصة (2026-10-21)
`gvenzl/oracle-xe:21-slim` (Docker Hub، لا يحتاج قبول رخصة؛ ≈ 2 GB) — ونفس الصورة متوفرة لـ `11-slim` و`18-slim`:
```bash
docker volume create odr-xe21
docker run -d --name odr-xe21 --network host --shm-size=1g -e ORACLE_PASSWORD=OdrLab2026x \
  -e ORACLE_CHARACTERSET=AL32UTF8 -v odr-xe21:/opt/oracle/oradata gvenzl/oracle-xe:21-slim
until docker logs odr-xe21 2>&1 | grep -q "DATABASE IS READY TO USE"; do sleep 10; done
# PDB الافتراضية XEPDB1؛ sqlplus داخل الحاوية: docker exec -i odr-xe21 sqlplus -s / as sysdba
```
19c الرسمية (`container-registry.oracle.com/database/enterprise:19.3.0.0`) تبقى تحتاج رخصة المالك.

**11g XE (11.2.0.2، غير-CDB) — نُفِّذ 2026-10-21، golden `golden-11g-20261021` (run `20261021T102638Z`):**
```bash
docker volume create odr-xe11
docker run -d --name odr-xe11 --network host --shm-size=1g -e ORACLE_PASSWORD=OdrLab2026x \
  -v odr-xe11:/u01/app/oracle/oradata gvenzl/oracle-xe:11-slim
until docker logs odr-xe11 2>&1 | grep -q "DATABASE IS READY TO USE"; do sleep 10; done
```
- لا PDB: شغّل نسخاً معدَّلة من `lab/sql/*.sql` (لا تعدّل الأصل) بأربعة تغييرات فقط: احذف كل `ALTER SESSION SET CONTAINER = &1;`، المسار `/u01/app/oracle/oradata/odr/` بدل `/opt/oracle/oradata/odr/`، `banner` بدل `banner_full` في `v$version` (مع `ROWNUM = 1`)، وسطر الـ pdb يصبح `SELECT 'pdb|NONCDB|0|' || dbid FROM v$database;`. `ORACLE_CHARACTERSET` غير مدعوم في صورة 11 (الافتراضي AL32UTF8).
- ملف SYSTEM اسمه `XE_system.dbf` (file#1 حقيقي؛ ليس PDB) — اختبار `lab2` يتعرف عليه من `kcvfh` + bootstrap لا من الاسم.
- بعد `SHUTDOWN IMMEDIATE` للتصدير لا تعيد صور gvenzl تشغيل القاعدة: `docker restart odr-xe11`.
- النتيجة: نفس الكود بلا تغيير — `odr dict` 0 مشاكل، `lab2_second_version_reads_like_oracle` يمر مع `ODR_GOLDEN2=<11g>`.

## 8. ملاحظات القرص والحاوية (حالة نهاية الجلسة 2026-10-18)
- القرص 27/29 GB: `/home/user/docker-data` 24 GB (صورة 10 GB + حجم oradata) · `target/` 1.9 GB. عند الضيق: `cargo clean` أولاً، ثم `docker rm -f odr-oracle && docker volume rm odr-oradata` (المجموعة الذهبية محفوظة في Release).
- بعد `SHUTDOWN IMMEDIATE` للتصدير تبقى الحاوية `unhealthy`: `docker restart odr-oracle` ثم انتظر `healthy` (≈ 3 دقائق).
- `/usr/bin/time` و`bc` غير مثبتين؛ للقياس استخدم بايثون (`time.time()` حول `subprocess.run`).
- `odr-corrupt` أصبح dev-dependency في `odr-cli` (PR #21) — اختبارات التلف لا تحتاج الملف التنفيذي.

## Fuzz محلياً (مطلوب قبل كل PR يلمس محلّلاً)
```bash
rustup toolchain install nightly --profile minimal -c rust-src
cargo install cargo-fuzz --locked
cd fuzz && cargo check --bins            # يجب أن يمر مع كل تغيير API عام
for t in kcvfh segment bootstrap_columns row_piece data_block; do
  cargo +nightly fuzz run "$t" -- -max_total_time=45
done
# عند crash: cargo +nightly fuzz run <t> artifacts/<t>/crash-*  → أصلح → أعد التشغيل → احذف artifacts/
```
