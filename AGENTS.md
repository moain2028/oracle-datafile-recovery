# AGENTS.md — Oracle Datafile Recovery
## ابدأ هنا
1. STATUS.md ثم docs/design/architecture.md ثم docs/research/01_foundational_research_ar.md.
## الفريق
وكيلان — اقرأ `docs/agents/README.md` ثم ملفك (`ASTRA.md` / `LEAD.md`). المدير (Lead) يملك العقود وصحة البيانات ويدمج (ADR-0006).

> **Latest handover (2026-10-24):** `docs/handover/2026-10-24_session_handover.md` — step g closed (LV-067), next: LOB (M5).
> **Session plan 2026-10-21 (part 2):** `docs/handover/2026-10-21_session_plan.md` — priorities from the field-expert interview (11g, Windows release, trial kit, torn partial recovery, dictionary-less mode).
> **Handover (2026-10-18, after PR #21):** `docs/handover/NEXT_AGENT_PROMPT.md` → `docs/handover/2026-10-18_final_handover.md` (state, method, numbers, limits, plan §6) → `docs/runbooks/agent-environment.md` (rebuild the sandbox, Docker, Oracle lab, golden set). Older memos (`2026-10-18_handover.md`, `2026-10-15_handover.md`) are history.
## الأوامر (Rust 1.99، مثبت عبر `rust-toolchain.toml`)
- بناء: `cargo build --workspace` · إصدار Windows: `cargo build --release -p odr-cli` ← `target/release/odr.exe`
- بوابات الجودة (نفس CI): `cargo fmt --all -- --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` · `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` · `python3 tools/gen_charsets.py --check` · `cd fuzz && cargo check --bins` · `cargo deny check`
- البوابات الذهبية (إلزامية لأي تغيير في محلّل): `ODR_GOLDEN=$PWD/lab/out/golden/20261018T080131Z cargo test -p odr-core --test golden_lab` (12) و`… cargo test -p odr-cli` (8) — المجموعة من Release `golden-26ai-20261018`.
- الاستخدام: `odr dict <system.dbf> [<files>…] [--json]` · `odr extract <files…> --table OWNER.NAME --out dir/ [--charset X] [--assume PATH=file:N,ts:N[,rfile:N],bs:N]`
- fuzz (nightly): `cd fuzz && cargo +nightly fuzz run decode_number`
- تجربة: `cargo run -p odr-testkit --example make_image -- t.img 1024` ثم `cargo run -p odr-cli -- scan t.img`
## القواعد الثابتة (لا استثناء)
- **ملف الدليل للقراءة فقط**: لا يُفتح أي ملف عميل إلا read-only؛ يُحسب SHA-256 قبل وبعد. أي كود يكتب على ملف المصدر = رفض الـ PR. (ADR-0002)
- **لا يتعطّل المحلّل على بايتات تالفة**: كل قراءة مقيّدة الحدود؛ الكتلة الفاسدة تُسجَّل وتُتجاوز ولا توقف المسح. (REQ-001.4)
- **كل رقم يُعلن** (دقة/سرعة) يأتي من `bench/` على مجموعة ملفات مرجعية مع بذور تلف متعددة وhold-out — لا أرقام من نفس بيانات الضبط. (LESSONS من cortex-memory)
- **ترخيص نظيف**: لا ننسخ من DUL أو RICO2/odbv (بلا ترخيص) أو OpenLogReplicator (AGPL). oradul (MIT) مسموح مع الإسناد. (ADR-0003)
- لا ملفات .dbf حقيقية لعملاء في المستودع أبداً؛ ملفات الاختبار تُولَّد من مختبر Oracle Free.
- معيار الجودة الكامل: ADR-0005 (يُفرض آلياً).
- قرار ← docs/adr/ · متطلب ← docs/requirements/ (EARS) · مشكلة > 30 دقيقة ← LESSONS.md.
