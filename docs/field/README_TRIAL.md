# ODR — Oracle Datafile Recovery (trial build)

Reads Oracle `.dbf` datafiles directly — no Oracle instance — and extracts table rows
into three files: `verified.csv` (bytes proven identical to what Oracle stores),
`inferred.csv` (derived by a stated rule, rule named per row) and `rejected.csv`
(could not be proven, reason per row). The tool never writes to the source files.

**Start:** `QUICKSTART_ar.md` (one page, Arabic). Self-checking run: `try_odr.ps1` (Windows) / `try_odr.sh` (Linux).

```
odr dict    <SYSTEM.dbf> <other.dbf>...                       # list tables and columns
odr extract <SYSTEM.dbf> <table.dbf> --table OWNER.TABLE --out DIR
odr scan    <any.dbf>                                         # block-level inspection
odr scavenge <any.dbf> --out DIR                              # no SYSTEM file: inferred only
odr-corrupt --input X.dbf --output Y.dbf --block-size 8192 --seed N --op bitrot:400   # damage a COPY
```

Exit codes: `0` all rows verified/inferred · `2` some rows rejected · `1` request refused.

This is a confidential evaluation build. © 2026 moain2028. All rights reserved. Use is governed by LICENSE (evaluation licence); open-source components: THIRD_PARTY_LICENSES.txt.
Redistribution, decompilation or reverse engineering is not permitted. See `LICENSE`.
Feedback: open an issue in the private repository or contact the owner directly.
