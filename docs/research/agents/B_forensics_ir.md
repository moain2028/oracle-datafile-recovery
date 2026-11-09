# Agent B_forensics_ir (gpt-5.1)

_Generated 2026-11-09 05:29 UTC, 592s_

1. Court‑/Insurer‑/IR‑grade requirements vs current `odr diagnose`
---------------------------------------------------------------

From a forensic / insurance / IR standpoint, `odr diagnose` is already much stronger than most vendor tools, but several elements must be explicit and machine‑verifiable.

### 1.1 Identification of the evidence and tool

**What is mostly there**

- Evidence file path and layout are described.
- Two hashes (“before” and “during”) are conceptually present (SHA‑256 mentioned in REQ‑001.6 / ADR‑0002).

**What is missing or must be tightened**

1. **Exact hash algorithms + values + scope**

   Required:

   - `hash_algorithm`: Must be explicit (e.g., `SHA-256`, `BLAKE3`), not assumed.
   - `hash_pre_scan`: over the **entire file as read from the evidence source**.
   - `hash_scan`: recomputed over the **exact byte stream actually inspected**.
   - A statement that the two are identical or, if not, why (partial imaging, read error, etc.).

   Implementation detail:

   - Record both hashes in all outputs (text, JSON, HTML, DB3F).
   - Record byte range used for hashing; typically:
     - `offset_start = 0`
     - `offset_end = file_size - 1` (inclusive), or shorter if partial image.
   - In JSON:

     ```json
     "evidence": {
       "path": "D:\\oradata\\ONYX\\users01.dbf",
       "size_bytes": 2147483648,
       "hash": {
         "algorithm": "SHA-256",
         "pre_scan": "a3f2...",
         "scan": "a3f2...",
         "range": { "start": 0, "end": 2147483647 }
       }
     }
     ```

2. **Tool identification and version pinning**

   Required:

   - `tool_name` (`odr diagnose`), `tool_version` (semantic), and ideally:
     - `build_id` or git commit.
     - `build_date`.
     - `config_flags` (e.g. `--fast`, `--force`).

   For reproducibility and expert testimony, the report must say:

   - “This report was produced by `odr diagnose` version 0.x.y (commit ABC123, Rust 1.xx.x, build date 2026‑11‑07), invoked as: …”

3. **Run context**

   Useful for chain of custody and reproducibility:

   - Hostname, OS, and architecture:
     - `host.name`, `host.os`, `host.arch`.
   - Time zone of the examiner’s system.

   These are optional from a strict minimum standard but frequently requested by courts and insurers.

4. **Examiner identity / organization**

   The tool cannot infer this, but should allow:

   - CLI options such as:
     - `--examiner-name "Ali Ahmed"`  
     - `--examiner-org "YemenSoft"`.
   - These should be recorded in report metadata.

   If not provided, fields should be present but explicitly null/empty to avoid ambiguity.

### 1.2 Time, acquisition, and chain‑of‑custody details

1. **Timestamps with timezone and format**

   Requirements:

   - ISO 8601 timestamps with timezone offset or `Z`.
   - At least:
     - `acquired_at` (if known; user‑supplied).
     - `analysis_started_at`.
     - `analysis_completed_at`.

   CLI could add:

   - `--acquired-at "2026-11-09T13:20:00+03:00"` (optional, user attested).
   - The tool must not fabricate acquisition times; if unknown, record `null`.

2. **Acquisition notes & evidence path semantics**

   For court / insurer, we need a clear boundary between:

   - The **source evidence** (original disk, image, exported file).
   - The **copy** actually analyzed (local path).

   The report should have:

   - `evidence_source_description`: free text (`"Customer provided copy via USB"`, `"FTK Imager .E01, exported .dbf"`, etc.), provided via `--evidence-notes`.
   - `evidence_container`:
     - `type`: `raw_file`, `disk_image`, `E01`, `AFF`, `logical_export`, etc.
     - `path`: path to the container, if known.
     - `internal_path`: for a file extracted from a container (e.g. `/volume1/oradata/users01.dbf`).

   If `odr` only sees a local `.dbf`, it should state that it analyzed a **logical copy** and has no visibility into imaging method or original media.

3. **Chain‑of‑custody elements**

   `odr` can contribute:

   - Integrity proof (hashes).
   - Immutable record of:
     - When it was run.
     - On what file path.
     - Using which binary and configuration.

   It should not claim full chain‑of‑custody; instead:

   - Include a **“Scope of Analysis” / “Limitations”** section, e.g.:

     > This report attests only to the condition of the file as provided to the tool at the time of analysis, on the basis of byte‑level examination of that file. It does not address how this file was acquired from original media or whether earlier alterations occurred before acquisition.

### 1.3 Reproducibility, methodology, and uncertainty

1. **Reproducibility statement**

   Required language:

   - Clearly state that another examiner, using:
     - The same version of `odr`.
     - The same evidence file (same SHA‑256).
     - The same parameters.
   - Should obtain identical results.

   Suggested text element:

   > Reproducibility: Given the same input file (SHA‑256 = …) and the same `odr diagnose` version and parameters, this analysis is deterministic; another examiner should observe identical findings.

2. **Methodology description**

   In a court‑oriented annex or in HTML:

   - Briefly outline:
     - Source of layout inference: `kcvfh` @ block 1; OS header at block 0 offsets:
       - Block size: `u32 @ 0x14` (20).
       - Block count: `u32 @ 0x18` (24).
       - Magic: `u32 @ 0x1C` (28) = `0x7A7B7C7D`.
     - Entropy threshold:
       - Shannon entropy of a 8 KiB block.
       - `Encrypted` if block not parseable as Oracle and `H ≥ 7.85` bits/byte.
       - `Garbage` otherwise.
     - Block corruption classification rules (as in §3.1 of your design).

   This helps another expert validate the tool’s logic.

3. **Uncertainty and limits**

   You already encode this logically (e.g., lost write out‑of‑scope, double‑bit flips not detectable). The report should **explicitly** mention:

   - “This tool cannot detect lost‑write corruption without redo or standby comparison.”
   - “Certain rare two‑bit flips may cancel in the XOR checksum (LV‑079); in lab measurements this was ~0.37% of induced bitrots.”
   - “We infer encryption patterns from content entropy and position; we do not possess decryption keys.”

   Prefer “inferred” vs “verified” where appropriate:

   - Header destroyed → layout “inferred, not directly read from header”.
   - Blocks with correct checksum but logical faults → “logical corruption inferred from intra‑block consistency checks.”

4. **Evidence container & DB3F referencing**

   Since you integrate DB3F, the report should:

   - State the **DB3F file name** exported (if any).
   - Give its hash and size.
   - Note that DB3F can be independently validated by other tools.

---

2. Ransomware patterns on large Oracle datafiles
-----------------------------------------------

### 2.1 Known families and partial‑encryption behaviour

Patterns focus on **large files** such as `.dbf`:

1. **LockBit (2.0 / 3.0)**

   - Historically used **intermittent encryption**:
     - Encrypt first N bytes or first X MB.
     - Then encrypt with a stride: e.g., 16 bytes every 256 bytes (for generic files).
   - On large databases, IR reports (2023‑2024) show:
     - First 1–4 MiB encrypted contiguously.
     - Then periodic encrypted chunks every fixed distance (128 KiB to several MB), tuned for performance.

2. **BlackCat / ALPHV**

   - User‑configurable intermittent encryption:
     - E.g., encrypt `chunk_size` bytes every `skip_size` bytes.
   - On large blobs (VMs/DB), patterns similar to LockBit but with more variety.

3. **Akira**

   - Tends to do **head‑heavy encryption**:
     - Large contiguous prefix of file.
     - Sometimes fixed‑size encrypted blocks with equal spacing later in file.

4. **Play**

   - Known to use partial encryption and **append ransom notes**.
   - Often encrypts:
     - First N MB.
     - Last portion of file (small tail) – may hold marker or footer alignment.

5. **Royal / Black Basta‑style variants**

   - Intermittent / partial encryption:
     - For large files, e.g., encrypt 10–20% of total bytes with a proprietary stride.
   - May embed markers/logical signatures into encrypted regions, but not always distinct at the byte pattern level.

6. **Conti (legacy; many spin‑offs)**

   - Older families: more **full‑file encryption** for smaller files.
   - For large volume data: often entire file or very large contiguous segments (tens/hundreds of MB).
   - Less sophisticated intermittent modes than newer groups.

### 2.2 Observable patterns at 8 KiB block granularity

For a tool like `odr diagnose`, which works at block level:

1. **Head‑only / head‑dominant encryption**

   - First continuous run of blocks classified as `Encrypted` or `Garbage` (by entropy).
   - Length: commonly measured in MB:
     - 1 MiB → 128 blocks of 8192 bytes.
     - 4 MiB → 512 blocks.
     - 16 MiB → 2048 blocks.
   - `RANSOMWARE_HEAD` directly maps to this.

2. **Periodic/intermittent encryption**

   - You already detect `RANSOMWARE_PERIODIC` via ≥4 damaged regions with a fixed stride (±1 block).
   - Distinguishing features:
     - Fixed distance `Δblocks` between start of each encrypted region.
     - Encrypted spans themselves may have constant size (e.g., N blocks) for some families.

3. **Tail and footer**

   You should be aware of three tail‑side phenomena:

   1. **File extension and renaming**

      - Some families rename `.dbf` to `.dbf.lockbit` or similar; the payload itself may be unchanged until the OS flushes a footer.
      - `odr diagnose` sees only the bytes; renaming alone is irrelevant, but IR teams need to correlate extension with content.

   2. **Ransom note as a separate file**

      - Many place note files (`README.txt`, etc.) outside the DBF; irrelevant to your analysis.

   3. **Ransom footer appended inside the encrypted file**

      - Some strains append a structured footer to each encrypted file, often:
        - Embedded victim ID.
        - Nonce, RSA‑encrypted session key.
        - Magic constant or ASCII marker (`LOCKBIT`, `ALPHV`, etc., sometimes obfuscated).
      - This may be appended at the very end of the file, occasionally separated with a small delimiter.

   For `.dbf`:

   - Because an Oracle datafile has a predictable structure (header blocks, tablespace metadata, segment headers), any non‑Oracle tail can be flagged as anomalous:
     - Last few blocks:
       - not parseable as Oracle.
       - high entropy **or** ASCII text inconsistent with Oracle block magic.
   - You should inspect the last, say, 32–64 blocks, looking for:
     - Printable ASCII markers.
     - Known ransom note fragments.
     - Specific magic constants (per family, if known from threat intel).

### 2.3 Can we identify the ransomware family from the pattern alone?

**Caution**: It is dangerous to assert a specific family name purely from layout/entropy patterns; many toolkits share or randomize strategies. From a forensic‑sound perspective:

- You **can** reliably say:
  - “Head‑only contiguous encryption region of X MiB” vs
  - “Intermittent encryption with stride S blocks and span length L blocks”.
- You **cannot** safely bind this to: “this is LockBit 3.0” without corroborating evidence:
  - File extensions (.lockbit, .play, .royal), ransom note contents, process artifacts, registry, etc.

Therefore:

1. **Pattern‑based family guess** should only be presented as:

   - “Pattern consistent with families A, B, C” **with low confidence**.
   - Or omitted from the formal report but optionally shown as an IR‑helper hint.

2. **Safer alternative**:

   - Expose the **measured parameters**:

     - Head encrypted size: `head_encrypted_bytes`.
     - Whether tail appears manipulated: `tail_pattern = {footer_present: bool, marker_snippets: [...]}`.
     - Periodic pattern:
       - `segment_encrypted_bytes`.
       - `segment_step_bytes`.
       - `segment_count`.

   - Let IR analysts or external tooling map patterns to families using external threat‑intel rules.

### 2.4 File tail and hidden / mixed damage

You should expect:

1. **Footer / marker presence**

   - In `RANSOMWARE_HEAD` and `RANSOMWARE_PERIODIC` cases, inspect final blocks:
     - If entropy is **lower** than typical encrypted blocks (e.g. < 7.85) and contains ASCII, you might be looking at:
       - Embedded ransom note, including contact email, TOR URL.
     - If footer is present, include a **hex/ASCII excerpt** (first N bytes of the abnormal tail region) in the report so IR can pivot.

2. **Hidden damage scenarios to accommodate**

   **a. Renamed or extension‑changed files**

   - `.dbf` → `.dbf.lockbit`, etc.
   - `odr` should:
     - Not rely on extension at all.
     - But record the current filename and extension for human correlation.

   **b. Partially restored from Volume Shadow Copy / backup**

   - Common scenario: some `.dbf` come from an earlier VSS snapshot, others from post‑attack disk, or a single file is a **copy‑paste of segments** by a human or ad‑hoc repair tool.
   - Symptoms:
     - Mixed SCN ranges in different regions of the same file.
     - Sudden version jumps in `kcvfh`‑type metadata across block ranges.
     - Regions with `Zeroed` blocks interleaved with old valid data.

   `odr diagnose` already sees corruption patterns, but you might extend:

   - A **“mixed epoch”** heuristic:
     - Record `scn` for blocks by range:
       - If a contiguous area shows SCNs grossly lower than surrounding ones, flag as potential “rollback to older copy segment”.
     - This is advanced and may belong to an M7 phase, but even a simple histogram in summary (“min/max/median SCN per file”, “multi‑modal SCN distribution”) is informative.

   **c. NTFS sparse files and partial physical allocation**

   - If `.dbf` is sparse and partially allocated (e.g., created or copied via tools that leave holes), `Zeroed` regions inside “used” ranges appear.
   - You already encode `ZEROED_REGIONS`. For IR:
     - Be explicit that these can be:
       - Uninitialized space; **or**
       - Partial‑copy / imaging failures; **or**
       - Overwritten with zeros by wiper / ransomware.

   **d. Volume Shadow Copy mix**

   - Some forensic acquisitions may splice segments from different snapshots:
     - Usually visible as non‑monotonic SCN and checkpoint positions.
   - For your Yemeni ERP use‑case, you likely see more **copy‑over‑network** truncations/holes than VSS splicing. Still, provide:

     - “Intra‑file inconsistency” pattern if checkpoints or file header metadata differs in clones of the same tablespace (beyond scope for first release, but note it).

---

3. Triage score: critique and an alternative metric
---------------------------------------------------

### 3.1 Issues with a 0–100 “recoverability score”

From a forensic, insurer, and IR perspective, a plain 0–100 score with simple deductions is problematic:

1. **Ambiguity**:

   - Does 80 mean “80% data intact”?  
   - Or “80% chance that recovery will succeed”?  
   - Or “our subjective rating of recoverability”?

2. **Non‑linearity and unexplainable jumps**:

   - E.g. deduct 30 points if header destroyed, 20 if ransomware found.
   - Small mechanical rule changes → large shifts in result.
   - Difficult to defend in expert testimony: “Why 30, not 25?”

3. **Mixes qualitatively different factors**

   - Head‑only encryption of 5% of file size can kill the dictionary (SYSTEM tablespace), making logical extraction vastly harder even if 95% of data blocks are unencrypted.
   - A simple % deduction does not reflect **business impact** or **recovery man‑hours**.

4. **Reluctance of insurers/courts to accept subjective scales**

   - Insurers want:
     - “X% of data blocks appear intact.”
     - “N rows plausibly recoverable.”
   - Courts prefer:
     - Raw counts and percentages with described methodology.
     - Expert narrative conclusion, not an opaque score.

### 3.2 Proposed alternative: quantitative recoverability metrics

Replace or subordinate the “0–100 score” with **explicit probabilities/percentages** and a short explanation.

Define:

1. **Block‑level metrics**

   - `used_blocks`: count of blocks that belong to objects (`0x06`, `Undo`, `LobChunk`, etc.) within the file’s declared used range.
   - For each damage state `D`:

     - `blocks_D`: number of used blocks with `Damage = D`.

   - E.g.:
     - `blocks_healthy`
     - `blocks_encrypted`
     - `blocks_zeroed`
     - `blocks_checksum_fail`
     - `blocks_logical`
     - etc.

   - Then:

     ```text
     intact_block_fraction = blocks_healthy / used_blocks
     ```

2. **Row‑level metrics (where deep mode is enabled)**

   - By object (DATA_OBJECT_ID) and overall:

     - `rows_live_total`
     - `rows_live_in_healthy_blocks`
     - Approximate `expected_rows_recoverable`:

       ```text
       expected_rows_recoverable = rows_live_in_healthy_blocks
       + alpha * rows_live_in_torn_blocks
       ```

       where `alpha` is empirically estimated probability of row salvage from a torn block (e.g. 0.3–0.5, documented).

3. **File‑level “expected data loss %”**

   Estimate:

   ```text
   estimated_loss_fraction = 1 - (rows_live_in_healthy_blocks / rows_live_total)
   ```

   Or, if rows are too expensive to count, approximate by blocks:

   ```text
   estimated_loss_fraction ≈ (blocks_corrupt_and_not_scavengable / used_blocks)
   ```

   Provide a **range** rather than a single number:

   - A simple **confidence interval** (CI):

     - Lower bound: assume **all** logically/structurally suspect but readable blocks are unusable.
     - Upper bound: assume a fraction (e.g. 50%) of such blocks can be partially salvaged.

4. **Presenting to the user**

   Example textual output:

   - “Estimated data loss: 5–10% of live rows in this file.”
   - “At least 90% of used data blocks appear intact (no corruption detected).”
   - “SYSTEM tablespace header destroyed; object names will be inferred rather than verified.”

5. **If you still want a single score**

   - Compute:

     ```text
     recoverability_score = round(100 * (1 - estimated_loss_fraction_midpoint))
     ```

     where `estimated_loss_fraction_midpoint` is average of lower/upper bound.
   - But **always** show the underlying:

     - `% blocks intact`
     - `% rows in intact blocks` (or “rows per damage type”).

   - Document in the report:

     - “This score is derived from block/row metrics and is not a probability; see underlying percentages for precise information.”

---

4. DB3F compliance: gaps and extensions
--------------------------------------

The Database Forensic File Format (DB3F) 2019 spec focuses on:

- Evidence container metadata (case, examiner, hashes).
- Logical DB elements (tables, records, fragments).
- Generic enough for carved records, not block forensic detail.

Given ODR’s design, there are two categories:

### 4.1 Likely gaps vs spec (to verify and address)

1. **Global metadata fields**

   DB3F expects a **top‑level header/metadata** section containing:

   - Evidence identifiers.
   - Hash(es) and algorithm.
   - Acquisition / analysis timestamps.
   - Tool name/version.

   Gaps to fix:

   - Ensure DB3F output includes:
     - `evidence_file_path`
     - `evidence_hash_value` and `hash_algorithm`
     - `dbms`: `"Oracle"`
     - `db_block_size`
     - `tablespace_name` (if known), or `unknown`.
     - `odr_version`.

2. **Block address representation**

   DB3F generally uses byte offsets; you also have logical block numbers (`rdba`).

   - Must ensure:
     - For each chunk/record you output, you provide:
       - Physical byte offset: `block_number * block_size`.
       - Logical address as “block_id” or “page_number”.

   If current DB3F export lacks:

   - `page_number` or `block_number`.
   - `file_number` vs `rdba` decomposition:

     ```text
     file# = (rdba >> 22) & 0x3FF
     block# = rdba & 0x3FFFFF
     ```

   then add them.

3. **Object identity**

   DB3F aims at **table‑centric** representation. You already collect:

   - `DATA_OBJECT_ID`
   - Block type (data/index/LOB).
   - Live row count (deep mode).

   Ensure:

   - There is a DB3F `Table` or `Object` entity keyed by:
     - `data_object_id`.
     - Possibly `segment_type` (TABLE, INDEX, LOB).
   - Entries for corrupt blocks are associated with `unknown` object where `objd` not trusted.

4. **Record/row fields**

   The spec describes generic record fields:

   - `record_id`, `table_id`, `field_list` (`{name, value, type}`).

   ODR’s **diagnose** is not primarily carving rows but counting; still, to be DB3F‑compliant, your **DB3F output should represent what you do have**:

   - For `diagnose`, you may emit:
     - `record_count_per_table`.
     - Or per block stats; but not full row content.

   If you already have `odr extract` using DB3F, ensure:

   - Any “diagnose DB3F” is clearly marked as **summary‑only** and consistent with fields:
     - `is_fragment`: generally `false` at this stage.
     - `source`: `"odr_diagnose"` vs `"odr_extract"`.

### 4.2 ODR‑specific fields to add (beyond 2019 spec but compatible)

Because DB3F is extensible, you can safely add **namespaced fields**:

1. **Per‑block evidence**

   For each block (if you export them):

   - `odr:block_kind`: `"Data"|"Index"|"Lob"|...| "Unknown"`.
   - `odr:damage`: `"None"|"Truncated"|"Zeroed"|"Encrypted"|...`.
   - `odr:entropy_bits_per_byte`.
   - `odr:scn` (as 64‑bit integer).
   - `odr:rdba`.
   - `odr:checksum_status`: `"ok"|"fail"|"not_applicable"`.

2. **Pattern‑level findings**

   A DB3F “Analysis” or “Annotation” section including:

   - `odr:pattern_code`: `["RANSOMWARE_HEAD", "ZEROED_REGIONS", ...]`.
   - For each pattern:
     - `odr:severity`: `"Critical"|"High"|...`.
     - `odr:confidence`: numeric [0,1].
     - `odr:evidence_blocks`: list or ranges (start_block, length).
     - `odr:human_reason` and `odr:recommendation`.

3. **Uncertainty / inference flags**

   - `odr:layout_source`: `"kcvfh"|"os_header"|"inferred_kcbh"|"cli_force"`.
   - `odr:system_dictionary_status`: `"verified"|"lost_header_inferred_only"`.
   - `odr:object_id_trust`: for each object, whether `DATA_OBJECT_ID` came from blocks with valid checksums.

These extensions keep DB3F valid while conveying your richer forensic semantics.

---

5. Top 10 implementable improvements for a Yemeni ERP support team
------------------------------------------------------------------

Ranked by immediate value for YemenSoft‑style support and IR.

### 5.1 High‑impact, short‑term

1. **Formal evidence metadata block in all outputs**

   - Add a consistent `report_metadata` section (text/JSON/HTML) with:
     - `tool_name`, `tool_version`, `build_id`.
     - `analysis_started_at`, `analysis_completed_at` (ISO8601 with timezone).
     - Hash algorithm + pre‑/post‑scan values.
     - CLI used.
   - Value: Makes every report court/insurer ready, with minimal dev work.

2. **Replace opaque 0–100 score with structured metrics**

   - Report:
     - `% of used blocks intact`.
     - `% of used blocks encrypted/zeroed/garbage/etc.`.
     - “Estimated data loss range: X–Y% of live rows.”
   - Optionally keep a “headline” score but **always** show the underlying numbers.
   - Value: More honest, defensible, and directly usable for pricing recovery services.

3. **Arabic‑first HTML report template with two layers**

   - **Management layer** (top):
     - Simple statements:
       - “Approximately 95–98% of your data is recoverable.”
       - “Ransomware encrypted only the initial 2 MiB of this file; the rest appears intact.”
   - **Technical annex**:
     - All block class stats.
     - Patterns, evidence, offsets.
   - Localized to Arabic but with optional English for courts.
   - Value: Directly usable by Yemeni support staff with non‑technical customers.

4. **Per‑file ransomware tail inspection**

   - Implement tail scanning:
     - For last N blocks (e.g. last 32–64 blocks).
     - Look for:
       - High‑entropy `Encrypted`/`Garbage` blocks that don’t fit normal Oracle patterns.
       - ASCII ransom text (keywords: `decrypt`, `bitcoin`, email, `.onion`).
   - In report, add:

     - “Suspicious non‑Oracle footer detected at byte offset 0x…, possible embedded ransom note / key material.”

   - Value: Helps IR teams and insurers without requiring them to open `.dbf` in hex editors.

5. **CLI options for examiner identity and evidence notes**

   - New flags:

     - `--examiner-name`
     - `--examiner-org`
     - `--evidence-notes`

   - Echo into all outputs and DB3F.
   - Value: Closes a standard objection from auditors and courts (“who did this, and under what circumstances?”).

### 5.2 Medium‑term but high value

6. **DB3F export upgrade with ODR‑namespaced block annotations**

   - Ensure DB3F export contains:
     - Evidence file hash and size.
     - Block numbers, offsets, damage status, entropy, SCN.
     - Pattern results as analysis entries.
   - Publish a short DB3F schema note for ODR fields (`odr:*`).
   - Value: Interoperability with forensic labs; differentiates YemenSoft’s tool from competitors.

7. **Object‑level “business impact” summary**

   - Per `DATA_OBJECT_ID` (and, if SYSTEM available, mapped table name):

     - `rows_live_estimated`.
     - `% blocks corrupted for this object`.
     - Simple classification:
       - “Mostly intact”, “Partially damaged”, “Heavily damaged / likely lost”.

   - Emphasize key app tables (Onyx‑specific: customers, invoices) once REQ‑007/Use case 7 is built.
   - Value: Let frontline support tell the customer **which business modules are affected**.

8. **Ransomware pattern parameterization instead of family labeling**

   - For `RANSOMWARE_HEAD`:

     - Report: `head_encrypted_bytes`, `head_encrypted_blocks`.

   - For `RANSOMWARE_PERIODIC`:

     - `segment_size_blocks`, `stride_blocks`, `segment_count`.

   - Do **not** state family names; instead: “pattern consistent with intermittent encryption used by several families.”
   - Value: IR can pivot to threat‑intel; tool stays forensically conservative.

9. **Explicit limitations / uncertainty section in the report**

   - Standard paragraph listing:

     - Lost write undetectable without redo.
     - Rare checksum collisions (LV‑079).
     - Logical inferences vs verified structures (e.g., inferred layout vs kcvfh).
     - No assessment of OS‑level artifacts or process evidence.

   - Value: Shields YemenSoft and examiners from overclaiming; aligns with expert‑witness best practices.

10. **Performance + safety knob preset profiles**

   - Provide named profiles:

     - `--profile support-fast`: disables deep row counting, keeps all block checks; target >300 MB/s.
     - `--profile forensic-full`: enables full `structural_check`, row counts, entropy, tail analysis (slower).

   - Profiles printed in metadata:

     - `profile: "support-fast"`.

   - Value: Support teams get predictable runtimes on customer laptops; forensic labs select full mode consciously.

---

These steps will turn `odr diagnose` from a strong engineering tool into a rigorously defensible forensic instrument while still staying practical for a Yemeni ERP vendor’s frontline support and IR partners.