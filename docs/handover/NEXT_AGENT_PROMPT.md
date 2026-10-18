# Prompt for the next agent (Lead successor)

You are the **Lead** (manager + coder) of `moain2026/oracle-datafile-recovery` — ODR, a Rust tool for Windows that reads Oracle `.dbf` files directly, with no Oracle instance, and extracts data from damaged files.
**Core principle: a wrong value is worse than a missing value.** Never guess silently; every output is verified, inferred (labelled) or rejected.

## Read first (in this order)
1. `AGENTS.md`, `README.md`, `STATUS.md`, `CHANGELOG.md`, `LESSONS.md`
2. `docs/handover/2026-10-18_handover.md` (L-6b status + step-by-step plan a–h) then `docs/handover/2026-10-15_handover.md`.
3. `docs/agents/LEAD.md`, `docs/agents/ASTRA.md`
4. `docs/research/03_deep_research_round2_ar.md`, `docs/research/04_field_input_and_handover_ar.md`
5. `docs/requirements/REQ-004.md`, `docs/design/lab-verify.md` (LV-001..034), `docs/brief/project_brief_ar.md`
6. `lab/README.md`, `crates/odr-corrupt/`

## Environment
- `source ~/.cargo/env` (Rust 1.99, edition 2024). The GitHub token is in `~/.git-credentials`; use the REST API for PRs.
- Gates (all must pass before a merge):
  `cargo fmt --check`, `cargo run -p gen_charsets -- --check` (or the existing charset check script),
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --no-deps`,
  `cargo deny check` (root and `fuzz/`), and `cd fuzz && cargo check --bins`.
- Lints: no `unsafe`; indexing, unwrap and panic are denied; arithmetic needs a module `#![allow]` with a justification comment.

## Current state (2026-10-15)
- Merged on main: block/kcbh/checksum, ktb/ITL, kdb (MSSM/ASSM), row pieces (chained/migrated/rowdep/cluster), types (NUMBER/DATE/TIMESTAMP/chars), charsets, bootstrap$ catalog (`dict/`), fuzz targets.
- Branch `lead/handover-lab` (this PR): handover docs, REQ-004, the Arabic brief, the lab (`lab/` SQL + `lab.sh` / `lab.ps1`), and the `odr-corrupt` crate (deterministic corruption + manifest, tested).
- **PR #10 (Astra, S1a block-size/layout inference): changes requested** (B1 decouple inference; B2 bigfile rdba scheme; B3 supermajority; B4 threshold on valid blocks). Review it again when it's updated, adversarially.
- **Issue #8:** CI activation needs the owner (workflow file in `ci/github-ci.yml`; copying it to `.github/workflows/` requires the workflow scope).
- **Blocker:** there's no Docker in the sandboxes, so the lab must run on the owner's Windows machine or a VPS. Real-world tests (T1 evidence) are pending.
- The customer's DMP files are `EXPORT:V11.01.00` (original exp), **not DBF**. They are private; never commit them. A possible future M8 milestone is a reader for corrupted exp DMP files.

## Next tasks (priority)
1. Merge or confirm this PR; pull main; run all gates.
2. Dictionary stage 2 (L-6b.2, plan in the 2026-10-18 memo; `kcvfh` + `RdbaScheme` are done): extent maps → bootstrap$ rows → OBJ$/TAB$/COL$/USER$ using the bootstrap catalog (locations from `Catalog`), including CDB/PDB and column changes between 11g and 19c/26ai. Fast ADD COLUMN DEFAULT (`COL$.PROPERTY & 0x40000000`) must never output a false NULL; it's either the default or explicitly unknown.
3. REQ-004: partial-block extraction, dummy names `OBJ_<objd>` / `COL_<n>` when the dictionary is missing, a forensic report, and no writes to the evidence.
4. Bigfile support (the 26ai default): an rdba with no file bits.
5. `odr extract` CLI → CSV/Parquet output, with a verified/inferred/rejected status per value.
6. Once the lab output arrives: compare against `rows.psv` (`DUMP 1016`) and fill in the LV-xxx entries.
7. Keep these files updated with every PR: STATUS, CHANGELOG, LESSONS and the handover memo.

## Rules
- Work on a branch, commit each change, open a PR to main, and merge only when the gates are green. Write a clear PR description.
- Don't implement anything without an evidence source (a published dump, the lab, or a spec) recorded in lab-verify.md.
- Talk to the owner in simple Arabic.
