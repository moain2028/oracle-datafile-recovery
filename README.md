# Oracle Datafile Recovery Platform

منصة مؤسسية لقراءة ملفات بيانات أوراكل (.dbf) التالفة أو غير القابلة للفتح واستخراج البيانات منها دون الحاجة إلى instance أوراكل — بوضع القاموس وبدونه، مع دعم ASM وLOB والضغط والصفوف المتسلسلة، وتقرير شرعي قياسي.

Enterprise-grade, DUL-class tool for extracting data from corrupted / unopenable Oracle datafiles without a running Oracle instance.

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
| `fuzz/` | 11 cargo-fuzz targets (nightly); CI runs a smoke pass |

## Status (2026-10-18, after PR #21)

ODR reads **real Oracle 26ai datafiles end-to-end** with no instance: `odr dict` resolves a full PDB dictionary (71,849 objects, 2,560 tables, **0 issues**) in 3.8 s; `odr extract` writes any heap/cluster table in three layers — `verified.csv` / `inferred.csv` / `rejected.csv` + `report.json` (SHA-256 of every input). Golden proof: 141,581 values byte-identical to Oracle `DUMP`, 15/15 tables match Oracle ROWIDs, and under deterministic damage (`odr-corrupt`: zeroed, bit-rot, holes, torn) **precision of the verified layer is 1.000**. 67 binary facts tracked in `docs/design/lab-verify.md` (22 confirmed on real files). CI green on Windows + Linux + licences + fuzz.

**Start here if you are a new agent:** `docs/handover/NEXT_AGENT_PROMPT.md` → `docs/handover/2026-10-18_final_handover.md` → `docs/runbooks/agent-environment.md`.

```
cargo build --release -p odr-cli
odr scan USERS01.DBF --json                              # classify every block (read-only; SHA-256 before/after)
odr dict SYSTEM01.DBF USERS01.DBF [--json]               # dictionary: tables, columns, NLS charset, issues
odr extract SYSTEM01.DBF USERS01.DBF --table HR.EMP --out out/   # verified / inferred / rejected + report.json
odr extract SYSTEM01.DBF DAMAGED.DBF --table HR.EMP --out out/ \
    --assume "DAMAGED.DBF=file:5,ts:4,rfile:5,bs:8192"  # header destroyed: operator identity => rows are *inferred*
```
Exit codes: 0 = no rejected rows · 1 = refused (no SYSTEM, unknown table, hash cluster, objd mismatch) · 2 = some rows rejected (see `rejected.csv`).
