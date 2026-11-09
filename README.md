# Oracle Datafile Recovery Platform

منصة مؤسسية لقراءة ملفات بيانات أوراكل (.dbf) التالفة أو غير القابلة للفتح واستخراج البيانات منها دون الحاجة إلى instance أوراكل — بوضع القاموس وبدونه، مع دعم ASM وLOB والضغط والصفوف المتسلسلة، وتقرير شرعي قياسي.

Enterprise-grade, DUL-class tool for extracting data from corrupted / unopenable Oracle datafiles without a running Oracle instance.

## Try it (Windows / Linux, no Oracle needed)
> **Private repository** (ADR-0007): code, lab notes and releases are not public. External evaluators receive the release archive and `docs/field/*` only.

Download `odr-<ver>-x86_64-pc-windows-msvc.zip` or the Linux tarball from **Releases**, plus a golden set (`golden-26ai-20261018`, `golden-21c-20261021`). Then follow **`docs/field/TRIAL_KIT_ar.md`** (Arabic): scan → dict → extract, make deterministic damage with `odr-corrupt`, and compare the three layers against the clean extraction.


## Windows app (0.7.0) — download, structure, remote lock
**Customer download page (permanent):** https://4c963f49-7f97-497d-ae4a-0ba8a209171a.vip.gensparksite.com — source `odr-apps/gateway/`. Direct Release: https://github.com/moain2028/odr-apps/releases/latest/download/ODR-Setup.exe — signed, no admin rights, Arabic RTL. Six pages in a sidebar, each with an ℹ️ that explains it: الرئيسية · الاسترجاع · **التشخيص** · **الفرز** · **البصمة** · الإعدادات. Source `app/windows/` (`docs/design/windows-app.md`). CI installs it on a real Windows runner and runs `--autotest` (18 checks) + `--selftest-license` (28) before publishing.

**Remote lock:** the public repo `moain2028/odr-apps` holds only `odr/license.json` (`"active": true|false` + `"code"`) and `odr/update.json`. `active:false` locks every installed copy within ≤ 10 min and shows the activation-code screen; first run never asks for a code; offline keeps the last known state (`docs/design/remote-lock.md`, `tools/odr_control.py`).

## `odr diagnose` — what happened to this file?
```
odr diagnose FILE.dbf [--lang ar] [--html report.html] [--db3f file.db3f] [--json] [--fast]
odr triage DIR…  [--json]            # many files, worst first
odr fingerprint learn DICT.json --out BOOK   ·   odr fingerprint match report.json --book BOOK
```
Every block is judged (kind × damage: zeroed / encrypted / **payload_scrambled** / torn / checksum / misplaced / logical / **stale_scn** / **duplicate_rdba** …), regions and per-object summary are built in one streaming pass with SHA-256 of the scanned bytes, and findings explain **cause · evidence · recovery · ORA errors** in Arabic and English with a 0–100 recoverability score and expected loss. Design: `docs/design/diagnose.md`; facts LV-075..084; seven field use cases: `docs/field/USE_CASES_ar.md`.

## Repository layout

| Path | Content |
|---|---|
| `STATUS.md` | الحالة وخارطة الطريق M0–M7 بمعايير إنجاز قابلة للقياس — **ابدأ هنا** |
| `AGENTS.md` / `CLAUDE.md` | قواعد العمل للمطورين والوكلاء |
| `docs/design/architecture.md` | المعمارية (C4): خط Scan → Extract، المكوّنات، المخاطر |
| `docs/adr/` | القرارات: لغة النواة، الدليل للقراءة فقط، clean-room، المجموعة الذهبية |
| `docs/requirements/` | المتطلبات بصيغة EARS (REQ-001 المسح، REQ-002 القاموس، REQ-003 الأنواع، REQ-004 الاستخراج الجزئي بثلاث طبقات) |
| `docs/research/01_foundational_research_ar.md` | البحث التأسيسي: الأدوات القائمة، الأبحاث، بنية الكتل، الفجوات، خارطة الطريق (كل مرجع تم التحقق منه) |
| `docs/research/02_expert_interview_questions_ar.md` | أسئلة مقابلة خبير الإنتاج |
| `crates/` | النواة Rust: `odr-core` · `odr-cli` (`odr.exe`) · `odr-testkit` |
| `docs/agents/` | تقسيم العمل على وكيلين (Lead + Astra) وملف مهمة لكل منهما |
| `docs/runbooks/agent-environment.md` | **إعادة بناء بيئة الوكيل من الصفر**: Rust، GitHub، Docker، Oracle 26ai، المختبر، المجموعة الذهبية (أوامر منفَّذة) |
| `docs/handover/` | مذكرات التسليم + prompt الوكيل التالي — **ابدأ هنا إن كنت وكيلاً جديداً** (`NEXT_AGENT_PROMPT.md` ← `2026-10-18_final_handover.md`) |
| `docs/design/lab-verify.md` | سجل الحقائق الثنائية LV-001..060: كل ثابت، مصدره، ودليله من الملف الحقيقي |
| `docs/runbooks/` `docs/postmortems/` | قوالب التشغيل وتحليل الحوادث |
| `LESSONS.md` `CHANGELOG.md` `CONTRIBUTING.md` `SECURITY.md` | الدروس، السجل، المساهمة، الأمان وبيانات العملاء |
| `crates/odr-corrupt/` | Deterministic, manifest-backed damage for **copies** of datafiles (10 patterns) — used by the golden precision tests |
| `lab/` | Oracle 26ai Free lab: corpus SQL, golden export, `odr-corrupt` recipes (`lab/out/` is never committed) |
| `fuzz/` | 12 cargo-fuzz targets (nightly); CI runs a smoke pass |

## Status (2026-11-03, v0.4.0-alpha)

Repository moved to **`moain2028/oracle-datafile-recovery`** (private, ADR-0007) with its full original history. ODR reads real Oracle **26ai, 21c and 11g** datafiles end-to-end with no instance. The three golden sets were rebuilt from `lab/` on 2026-11-03 and live in private Releases (`golden-26ai-20261103`, `golden-21c-20261103`, `golden-11g-20261103`); the CI job `golden` runs every golden test against all three on each PR. Measured: 26ai **147,661/147,661** values byte-identical to Oracle; dictionary 0 issues on all three versions; dictionary-less type inference 0 wrong on the 21c/11g hold-outs; **0 wrong verified rows** under bitrot / torn / zero / swap / ransom / wiped header (forge inside a value with recomputed checksum: 0.16 %, declared limit).

**Evaluators:** from Releases download `odr-<tag>-windows-trial-bundle.zip`, unzip, double-click `START_HERE.bat` — offline, 33 self-checks; the same bundle is run on a real Windows runner by `trial.yml` before every hand-over.

**New agent:** `docs/handover/2026-11-03_session_handover.md` → `docs/handover/NEXT_AGENT_PROMPT.md` → `docs/runbooks/agent-environment.md`.

```
cargo build --release -p odr-cli
odr scan USERS01.DBF --json                              # classify every block (read-only; SHA-256 before/after; block size + byte order from the header)
odr dict SYSTEM01.DBF USERS01.DBF [--json]               # dictionary: tables, columns, NLS charset, issues
odr extract SYSTEM01.DBF USERS01.DBF --table HR.EMP --out out/   # verified / inferred / rejected + report.json
odr extract SYSTEM01.DBF DAMAGED.DBF --table HR.EMP --out out/ \
    --assume "DAMAGED.DBF=file:5,ts:4,rfile:5,bs:8192"  # header destroyed: operator identity => rows are *inferred*
```
Exit codes: 0 = no rejected rows · 1 = refused (no SYSTEM, unknown table, hash cluster, objd mismatch) · 2 = some rows rejected (see `rejected.csv`).
