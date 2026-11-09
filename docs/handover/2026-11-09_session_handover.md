# Session handover — 2026-11-09 (Lead) — diagnose engine · remote lock · Windows app v0.7

> **Start here if you are a new agent on a new machine.** Then `NEXT_AGENT_PROMPT.md` for the rules, then
> `docs/runbooks/agent-environment.md` for the environment. Everything below is on `main` and tagged
> `v0.7.0-alpha`; nothing is uncommitted.

## 0. The state in one table

| What | Where | Proof |
|---|---|---|
| Repo (private, everything) | `moain2028/oracle-datafile-recovery`, `main` = `0d96c8c`, branch `genspark_ai_developer` = main | PRs #11–#16 merged (squash) |
| Public download + remote switch | `moain2028/odr-apps`: `odr/license.json`, `odr/update.json`, Releases | https://github.com/moain2028/odr-apps/releases/latest |
| Installer | `ODR-Setup-0.7.0.exe` (35.6 MB) sha256 `59eba79b3c242b27ec515ed3f46f92aec04cdda52bea47c431e73e16d0e9fd04` | built, signed, installed, autotested, uninstalled on a real Windows runner (`app.yml` run 42582932029) |
| Engine gates | `cargo fmt/clippy -D warnings/doc/test --workspace` | green at `0d96c8c` |
| App self-tests | `ODR.exe --selftest-license` 28/28 · `--autotest` 18/18 | in the CI log + `odr-app-proof` artifact (screenshots of all pages) |

## 1. What was built this session (chronological)

### 1.1 `odr diagnose` — corruption detection & explanation engine (PR #11, #12)
`crates/odr-core/src/diagnose/` — design: `docs/design/diagnose.md`; facts: LV-075..084.
- `layout.rs`: block size / byte order from three witnesses: `kcvfh` → OS block 0 magic (`0x7A7B7C7D` @28) → self-addressed `kcbh` vote.
- `verdict.rs`: per block **kind × damage**. Damage codes: `truncated zeroed header_zeroed encrypted payload_scrambled garbage marked_corrupt checksum_fail torn misplaced logical stale_scn duplicate_rdba`. Randomness = entropy + χ² + serial correlation (`looks_random()`); a Weyl sequence was correctly *rejected* (LV-080).
- `mod.rs`: streaming 4 MiB, O(regions) memory, SHA-256 of scanned bytes, `Counters` (dbv-parity, LV-078; `checksum_flagged`), regions, per-object summary, **post-pass** (`consistency_post_pass`): per-object median/MAD SCN → `stale_scn`; rdba map → `duplicate_rdba`; bounded by `max_witnesses` and *disclosed* (`consistency_pass: skipped`). `tail_excerpt` (hex/ASCII of a foreign file tail).
- `pattern.rs`: findings with severity, confidence‰, evidence, cause/recovery (EN/AR), ORA errors, **`parameters`** (ransomware geometry: `head_bytes/stride_blocks/span_blocks` — never a family name). Codes: `HEALTHY HEADER_DESTROYED TRUNCATED RANSOMWARE_HEAD RANSOMWARE_PERIODIC ENCRYPTED_SCATTERED PAYLOAD_SCRAMBLED ZEROED_REGIONS BIT_ROT TORN_WRITES MISDIRECTED_WRITES LOGICAL_CORRUPTION MARKED_CORRUPT GARBAGE_OVERWRITE SIZE_MISMATCH STALE_BLOCKS DUPLICATE_BLOCKS HEADER_BLOCKSIZE_MISMATCH CHECKSUM_COVERAGE_LOW CONSISTENCY_PASS_SKIPPED TAIL_FOOTER`.
- `triage.rs`: score 0–100 with deductions, band, `expected_loss_milli`, `bytes_affected`, `checksum_coverage_milli`, next steps with commands.
- `html.rs` (self-contained RTL/EN report), `db3f.rs` (DFRWS 2019).
- CLI: `odr diagnose FILE [--json|--html P|--db3f P] [--lang ar|en] [--fast] [--list N] [--force --block-size --endian]`, `odr triage PATHS… [--json] [--fast]`, `odr fingerprint learn DICT.json --out BOOK [--product P] [--owner O…]`, `odr fingerprint match report.json --book BOOK [--json]`.
- Tests: `tests/diagnose_synthetic.rs` (6), `tests/diagnose_panel.rs` (9), unit tests in `mod.rs`/`verdict.rs`.
- Research: three parallel reviewer agents (Oracle internals / forensics-IR / algorithms) via the LLM proxy; raw outputs `docs/research/agents/`, decisions `docs/research/06_agent_panel_synthesis_ar.md` (accept/defer/reject with reasons).

### 1.2 Remote lock — kill switch via the public repo (PR #13–#15)
Design: `docs/design/remote-lock.md`. **Owner's words: "I can lock and unlock remotely. That's all."** No server, no hosting — the same model as *Qurat Al-Mahalat* (`lib/remote/license_service.dart`, analysed in `~/sb-git-refs/qurat`), ported to C#.
- `app/windows/Remote/`: `RemoteSource` (GitHub Contents API + raw fallback, nonce, never throws), `LicenseService` (pure `Apply()` state table), `Settings` (`%LOCALAPPDATA%\ODR\settings.json`), `DeviceIdentity` (8 hex of SHA-256(MachineGuid)), `LockForm` (RTL; device id copy + WhatsApp; brute-force slowdown).
- **First run: never asks for a code.** Cached state defaults open; `active:true` keeps it open; offline/404-before-seen/corrupt = open. The code box appears **only** when `active:false` (and `code`/`codeSha256` present). Verified by tests `offline first run -> open`, `404 before ever seeing the file -> open`, `active:true -> open`.
- `odr/license.json` (public): `active` (bool) + `code` (plain, owner's choice) or `codeSha256`; `message`, `blockedDevices[]`, `blockedMessage`, `offlineGraceDays`, `contact`. `_help_ar` explains it inline.
- `tools/odr_control.py status|lock|unlock|set-code|block-device|unblock-device|grace|revoke|hash` (needs `GITHUB_TOKEN` with Contents: write on `odr-apps`).
- Live end-to-end test done against the real repo: lock → app sees `Locked` → wrong code refused → right code accepted → unlock → open.
- CI: `--selftest-license` before signing; on tags, publishes installer + `update.json` to `odr-apps` (secret `ODR_APPS_TOKEN`).
- `.gitignore`: `signing/activation_code.txt`, `*.pfx`, `*.jks` never committed. The current activation code is **only** in `odr-apps/odr/license.json` (`code`) — by the owner's decision it is visible there.

### 1.3 Windows app v0.7 — restructured (PR #16)
Design: `docs/design/windows-app.md`. Shell `MainForm.cs` (navy sidebar, gold, RTL), pages in `app/windows/Ui/`: `HomePage` (4 doors + drag-drop), `RecoverPage` (the old window, logic unchanged), `DiagnosePage` (gauge, verdict, finding cards, regions, next steps, HTML/DB3F/JSON), `TriagePage`, `FingerprintPage`, `SettingsPage`; `Theme.cs`, `Help.cs` (**ℹ️ texts for every page**), `DiagnoseModel.cs`. `--autotest` 14 → 18 checks (+ diagnose on a healthy demo file = 100/100, HEALTHY).

## 2. How to work on this from a new machine (≈ 20 min)

```bash
# 1. clone (token with repo + workflow; the owner issues it — never paste it in chat)
cd /home/user/webapp && git clone https://<TOKEN>@github.com/moain2028/oracle-datafile-recovery.git odr && cd odr
git checkout genspark_ai_developer   # work branch; PRs go to main; the Lead merges (squash)

# 2. Rust toolchain + gates (runbook §1)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.99.0 --profile minimal -c rustfmt -c clippy
source ~/.cargo/env
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo doc --workspace --no-deps

# 3. Windows app compiles on Linux (net48 reference assemblies) — no Windows needed to check C#
mkdir -p lab/out/dotnet && curl -sSL https://dot.net/v1/dotnet-install.sh | bash -s -- --channel 8.0 --install-dir lab/out/dotnet --no-path
lab/out/dotnet/dotnet build app/windows/ODR.csproj -c Release -nologo      # "Build succeeded."

# 4. golden sets (optional, for golden tests): runbook §7c — Releases golden-{26ai,21c,11g}-20261103
# 5. damage matrix for diagnose: lab/out/dmg/*.dbf are generated by odr-corrupt from the 26ai golden (see docs/design/diagnose.md §6)

# 6. release: bump <Version> in app/windows/ODR.csproj, merge to main, then
git tag -a vX.Y.Z-alpha -m "..." && git push origin vX.Y.Z-alpha
# app.yml builds/signs/installs/autotests on Windows and publishes to odr-apps (needs secret ODR_APPS_TOKEN)
```

Remote switch from anywhere: edit `odr-apps/odr/license.json` on GitHub (phone works) or `GITHUB_TOKEN=… python3 tools/odr_control.py lock|unlock`.

## 3. Measured numbers to remember
- dbv class parity 100 % on 4 × 26ai files (LV-078); damage matrix 10 patterns × 3 seeds: recall/precision 100 % except XOR-16 checksum collisions 2/545 (LV-079, declared limit) and forge inside a value (declared).
- Healthy max entropy 7.676 bits/B vs random 7.977; threshold 7.85 + χ² ≤ 330 + |r| ≤ 0.05 (LV-077/080).
- Windows runner: install → 18 tables, 20,000/6/6/2,000 rows exact, Arabic exact, diagnose 100/100.

## 4. Declared limits (unchanged unless measured)
- `payload_scrambled` is judged on **table data blocks only** (index/LOB payload decoders deferred — panel §2).
- `duplicate_rdba` only catches the smallfile/bigfile-ambiguous case; a plain copied block is caught earlier as `misplaced`.
- Remote lock is a kill switch, not per-customer licensing; GitHub API 60 req/h per IP (raw fallback).
- Diagnose throughput not yet benchmarked (M1).

## 5. Next, in order
1. Owner tries v0.7.0 on his machine; collect UI feedback (small screens, fonts).
2. CLI `--examiner/--org/--case/--note` → `evidence` block in JSON/HTML/DB3F (panel §1.8).
3. README.md top section for diagnose/triage/fingerprint (this handover + CHANGELOG already cover it).
4. Golden diagnose test in CI (damage matrix regenerated from the 26ai golden), speed (M1), real damaged file from YemenSoft.
5. In-app update check using `odr/update.json` (fields are already published; the C# consumer is not written yet).

## 6. Security notes for the owner (open)
- Two tokens appeared in chat this session (GitHub `ghp_GmLM…`, GitLab `glpat-_pmX…`). **Revoke both.** Create a fine-grained PAT (Contents: write on `odr-apps` only) and store it as repo secret `ODR_APPS_TOKEN`.
- `signing/` must stay out of git (it is). Code-signing PFX lives only in CI secrets `ODR_SIGN_PFX_B64` / `ODR_SIGN_PASS`.
