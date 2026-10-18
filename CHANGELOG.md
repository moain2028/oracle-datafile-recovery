# Changelog
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
## [Unreleased]
### Added
- LV-059: every `RawRow` carries `block_status`; rows from blocks failing checksum/tail check are **rejected** (`block integrity: …`), zeroed/truncated blocks appear as `integrity:` skips. Measured with `odr-corrupt` on the 26ai golden ASSM file: precision(verified) = 1.0 on zero/bitrot/holes/torn (was 3 wrong verified cells), recall 0.958/0.937/1.0/0.986. Golden test `damaged_copies_never_yield_a_wrong_verified_cell`.
- LV-060: `odr dict|extract --assume PATH=file:N,ts:N[,rfile:N],bs:N` for files whose header is destroyed; rows from such a file are at best *inferred* and labelled; `report.json` marks `assumed`. Golden test `wiped_header_requires_assume_and_downgrades_to_inferred`.
- `odr-corrupt` is a workspace dependency (dev) so damage tests run in-process and deterministically.
- `dict/schema.rs`: `ColumnDef`/`TableDef`/`Cell {Verified, Inferred, Unknown}` + `materialize` (fast `ADD COLUMN … DEFAULT` rule: absent column ⇒ Inferred(DEFAULT$)/Unknown, never NULL); exact `encode_decimal` for DEFAULT$ literals.
- `dict/rows.rs`: `read_heap` / `read_cluster` over any `BlockSource` (chained/migrated prefetch, foreign-objd rejection, deleted rows skipped, `rowid_row` per LV-046); golden test `lab_row_readers_match_oracle_rowids` — all 15 lab tables match Oracle's ROWID sets exactly.
- CLI (step h): `odr dict <files…> [--json]` (headers, SYSTEM detection, tables/columns, dictionary issues) and `odr extract <files…> --table OWNER.NAME --out dir/ [--charset]` writing `verified.csv` / `inferred.csv` (with `_inference`) / `rejected.csv` (with `_reason`) + `report.json` (SHA-256 of every input, segment, skipped blocks, counts). Refuses when the segment's data object id disagrees with the dictionary. Golden CLI tests: 13 tables, verified+inferred == Oracle COUNT(*), 0 rejected, T_ADDCOL layer semantics.
- `Dictionary::properties` reads `SYS.PROPS$`; `odr extract` takes `NLS_CHARACTERSET`/`NLS_NCHAR_CHARACTERSET` from it (`--charset` overrides); `odr dict` prints them (LV-056).
- Migrated cluster members (`F L` + `hrid`) decode (LV-057); deleted-and-overwritten rows are skipped not reported (LV-058) → the whole 26ai dictionary loads with **0 issues** (was 8); +1 table, +1 user recovered.
- `CLU$` in the resolver (`Dictionary::clusters`, `cluster_of`); `odr extract` handles indexed-cluster members (T_CL_DEPT/T_CL_EMP == Oracle, NULL trailing key padded, Arabic names intact); hash clusters refused explicitly.
- Fuzz targets `kcvfh`, `segment` (self-loop termination bound), `bootstrap_columns`; CI runs them.
- `FileInfo::from_header` + tablespace-scoped relative-file resolution (LV-055).
- `dict/resolver.rs` (step f): `Dictionary::load(src, system_file)` — kcvfh → root → bootstrap$ → OBJ$ / TAB$ / COL$ (C_OBJ#) / USER$ / TS$, every column located by name in the file's own bootstrap DDL; `tables() -> Vec<TableDef>`, `table(owner, name)`, `issues` (dropped rows, duplicates, skipped blocks). Golden: 71,849 OBJ$ rows, 2,559 TAB$, 11,317 tables with COL$, 155 users, 10 tablespaces, 6 issues (2 malformed/chained dictionary rows reported, not guessed).
- Golden `lab_dictionary_resolves_odr_tables_like_oracle` (obj#, dataobj#, tablespace, segment header, every column's name/segcol#/type/nullability/hidden, DEFAULT$ + fast-add bit, cluster bobj#/tab#/clucols vs `dba_*`) and `lab_end_to_end_dictionary_to_cells` (SYSTEM file → 13 tables → 131,536 cells == Oracle, no lab metadata used).
- Golden test `lab_values_match_oracle_dump_byte_for_byte` (step g): 141,581 stored values of 13 heap tables compared byte-for-byte with Oracle `DUMP`, 131,524 canonical texts with `TO_CHAR`, plus the fast-ADD-COLUMN rule on real T_ADDCOL rows.
### Fixed
- Fuzz findings: extent maps claiming > 2^26 blocks are refused by the parser; empty quoted column names in bootstrap DDL are rejected.
- SYSTEM detection no longer requires `kcvfhfno == 1` (PDB SYSTEM is file 12); proven by bootstrap$ rows instead (LV-053).
- `LONG` dictionary columns (`COL$.DEFAULT$`) read from the row's last stored column with the exact-length rule (LV-054).
- `tools/gen_charsets.py --check` compares bytes (Windows CI false "stale").
- Fast ADD COLUMN: bit-vector indexes nullable fast-added columns only; set bit ⇒ explicit NULL even when trailing-absent (LV-050).
- `NUMBER` `80 02` (1E-130) decodes instead of being rejected (LV-051).
- ROWDEPENDENCIES `dscn` is 7 bytes on 26ai; width settled per block from the row-packing invariant, never guessed (LV-052).
- `ktbbhict` is one byte (LV-047): 26ai writes 0x1F at 0x25, so the old u16 read rejected ~9.8k real blocks.
- Empty formatted data blocks (`ntab=0,nrow=0,fsbo=14`) are well-formed (LV-048).
- `nrid`/`hrid`/cluster `prev,next` are big-endian on every platform (LV-049) — chained and migrated rows now assemble on little-endian files.
- `odr-corrupt` crate + CLI: deterministic, manifest-backed corruption of datafile copies (bitrot, torn, zero, ransom, wipe-header, truncate, swap, holes, forge); property test that the manifest covers the exact diff.
- `lab/`: Oracle Free 26ai golden-corpus lab (tablespaces ASSM/MSSM/bigfile, deterministic corpus incl. Arabic/cluster/rowdep/chained/fast-add-column, `DUMP 1016` truth export, block dumps, datafile copies after shutdown + dbv), `lab.sh`/`lab.ps1`, README.
- Docs: handover memo + next-agent prompt, research 04 (field input, versions, DMP), REQ-004 (partial-damage extraction), Arabic project brief.
- Cargo workspace (Rust 1.99, edition 2024): `odr-core` (bounds-checked `Cursor`/`ByteOrder`, `kcbh` + XOR checksum + tail check + `classify`, read-only `Evidence` with SHA-256, exact `NUMBER` and `DATE` decoders, reference scanner, row contracts), `odr-testkit` (synthetic blocks + deterministic corruption), `odr-cli` (`odr scan`, text/JSON, exit codes).
- Strict lints (`unsafe` forbidden, indexing/unwrap/panic denied), CI on Windows + Linux (`ci/github-ci.yml`, enable per `ci/README.md`), `cargo deny`, fuzz targets, property tests.
- ADR-0005 engineering quality bar, ADR-0006 multi-agent workflow (amended: Lead + Astra), briefs `docs/agents/{README,ASTRA,LEAD}.md`, `docs/design/lab-verify.md`.
- Data-block layer (Lead L-1): `row::ktb` (ktbbh, ITL, `data_object_id`), `row::kdb` (kdbh/kdbt/kdbr with MSSM/ASSM detection by structural invariants), `row::piece` (lengths, NULL, `0xFE` long form, nrid/hrid, deleted flag); 5 tests incl. 10k proptest cases; `data_block` fuzz target.
- Character sets (Lead L-2): `charset::decode` for AR8MSWIN1256, AR8ISO8859P6, WE8MSWIN1252, WE8ISO8859P1, US7ASCII, AL32UTF8, UTF8 (CESU-8), AL16UTF16 (UTF-16BE); never guesses (U+FFFD + `invalid` count); tables generated by `tools/gen_charsets.py` and cross-checked against three independent sources; `charset` fuzz target.
- Datatypes (Lead L-3): TIMESTAMP[(p)] / WITH TIME ZONE (offset + region id) / WITH LOCAL TIME ZONE, INTERVAL YEAR TO MONTH / DAY TO SECOND, BINARY_FLOAT / BINARY_DOUBLE (order-preserving transform), ROWID (extended + restricted, base-64 text), CHAR padding, RAW hex; reference encoders + 40k proptest cases; `types` fuzz target.
- Row assembly + typed values (Lead L-4): `row::assemble` (chained, migrated, N/P column splits; cycle detection, `MAX_PIECES` bound, no silent column loss) behind a `PieceSource` trait; `value::decode_value` → `Value` with `canonical()` text and `Value::Invalid` that keeps raw bytes (REQ-003.3); `value` fuzz target.
- Research round 2 (`docs/research/03_deep_research_round2_ar.md`): cluster rows, ROWDEPENDENCIES, compression header, `kcvfh` map, `bootstrap$`, optimized `ADD COLUMN … DEFAULT`; `lab-verify` LV-018..LV-024.
- Indexed-cluster rows (`row::cluster`): key rows (`curc`/`comc`/prev/next) and member rows (`cki`), `DataBlock::cluster_key`, `DataBlock::table_of`, `cluster_columns` (pads NULL trailing key columns to `CLUCOLS`). Prerequisite for the dictionary (`TAB$`/`COL$` live in `C_OBJ#`).
- `ROWDEPENDENCIES` rows: `kdbh` flag 0x20 → 6-byte `dscn` skipped and exposed as `RowPiece::dscn` (previously every column would have been shifted by 6 bytes).
- `row_piece` fuzz target; `data_block` fuzz target now exercises cluster lookups.
- Research round 2 (`docs/research/03_deep_research_round2_ar.md`) and L-4b: indexed-cluster row pieces (`row::cluster`: key rows with curc/comc/prev/next, member rows with `cki`, `DataBlock::cluster_key`, `table_of`, `cluster_columns` with NULL-key padding), `ROWDEPENDENCIES` rows (`PieceFormat`, `RowPiece.dscn`), explicit refusal of compressed (9ir2) blocks; tests built from published dumps with `tl` checks; `row_piece` fuzz target; LV-018..028.
- Dictionary L-6a: `dict::bootstrap::Catalog::from_rows` parses `bootstrap$` DDL (OBJNO, TABNO, `EXTENTS (FILE f BLOCK b)`, `CLUSTER c(cols)`, version marker) with quoted-identifier-safe tokenizing and explicit rejection of contradictory rows and conflicting duplicates; `OraNumber::to_i64` (exact, never rounds); golden test from a published BBED dump of `bootstrap$`; `bootstrap` fuzz target; LV-032..034.
- Dictionary L-6b.1 (`dict::kcvfh`): datafile header decoder (`FileHeader::parse`/`detect`): block size, byte order (both orders tried; ambiguity rejected), DBID/name, file#, **root dba `kcvfhrdb`** (bootstrap$ segment), creation/checkpoint SCN, status; version-dependent fields (`ts#`, tablespace name, `rfile#`) are `Option` and accepted only when self-consistent. Golden test from a published 11.2.0.4 BBED dump cross-checked against `x$kcvfh`/`v$datafile` (LV-035/036).
- `block::RdbaScheme` (smallfile/bigfile) + `Rdba::{smallfile, split, is_null}` — a bigfile rdba can never yield a file number (LV-031/037).
- `value::ColumnType::{Long, LongRaw}` (type# 8/24; needed for `COL$.DEFAULT$`).
- Research 05: authoritative 9iR2 `sql.bsq` column lists of `OBJ$`/`TAB$`/`COL$`/`USER$`/`TS$`/`SEG$`/`CLU$`, 10g bootstrap object map, ASSM segment-header/EMB extent-map layout notes.
- **Lab ran for the first time (Oracle AI Database 26ai Free 23.26, in-sandbox Docker):** golden set `lab/out/golden/20261018T080131Z` (not committed): 15 tables + cluster, 141,613 truth values (`rows.psv`, DUMP 1016 incl. 4000-byte columns), 20,702 rowids, segment/extent/column metadata, block-dump traces, datafile copies after `SHUTDOWN IMMEDIATE`, `dbv` clean. `tests/golden_lab.rs` runs against it when `ODR_GOLDEN` is set (kcvfh vs `x$kcvfh`, every block Valid, bootstrap$ walk → 60 rows → 59-object catalog, 14 segment headers vs `dba_extents`).
- Dictionary L-6b.2: `dict::segment` (extent maps of 0x10/0x23/0x24 + bounded `walk_segment`), `dict::source` (`BlockSource`, `FileSet` over read-only `Evidence`, `ImageSet`), `bootstrap$` column lists (`BootObject.columns`, LV-038/045).
- `DecodeError: Serialize` (forensic reports).
### Fixed
- **`0xFE` long column length is in the file byte order, not big-endian** (LV-009 refuted on a real 26ai little-endian file: 21 bootstrap$ rows > 250 bytes). Previously every row with a > 250-byte value would have failed to decode on x86 files.
- Lab scripts: SQL*Plus-breaking inline comments, NCHAR/VARCHAR2 mix in `T_JOURNAL`, `DUMP()` > 4000 bytes (ORA-01489) in the truth export.
### Changed
- **Breaking (pre-1.0):** `row::piece::parse_piece` takes a `PieceFormat`; `RowPiece` gains `dscn` and `cluster`. Cluster pieces are no longer `Unsupported`.
- `parse_piece` takes a `PieceFormat`; compressed blocks (`kdbh` 0x40) are rejected as `Unsupported` with an explicit reason; undocumented combinations (chained cluster pieces, chained/migrated rows with row dependencies) are `Unsupported`, never guessed.
- ADR-0001 accepted (Rust, Windows-first static build).
- طقم التوثيق (هيكلية cortex-memory v0.3): AGENTS/CLAUDE/STATUS/LESSONS/CONTRIBUTING/SECURITY/LICENSE، قوالب ADR/design/requirements/runbooks/postmortems، قالب PR.
- `docs/design/architecture.md` (C4)، ADR-0001..0004، REQ-001..003.
## [0.0.1] - 2026-10-11
### Added
- البحث التأسيسي وأسئلة مقابلة الخبير.
