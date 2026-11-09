# Agent A_oracle_internals (gpt-5.2)

_Generated 2026-11-09 05:21 UTC, 101s_

## 1) Corruption / anomaly types real DBAs see that `verdict()` cannot detect or will misclassify (block-only), with byte-level signatures & block-only detection

I’m focusing on things that (a) happen in the field, (b) can be detected from *a single block image* without redo/other blocks, and (c) are currently outside or shaky in your rule set.

### 1.1 “Stale / wrong incarnation” block (SCN regression inside the block itself)
**What happens:** A block is an *older* version of the same block (classic lost-write symptom) or from a prior restore/incarnation. Oracle usually needs redo to prove it, but there are **block-internal SCN invariants** you can still flag as “SCN-inconsistent” with high confidence.

**Byte signature (kcbh):**
- `kcbh` contains **block SCN** split across fields (version-dependent). In modern releases it’s effectively a 48-bit or 64-bit SCN: `scn_base + scn_wrap` style.
- You already extract `scn` from the header.

**Block-only detection heuristic (single block):**
- Compare `kcbh.scn` against **ITL transaction SCNs** (per-ITL commit scn / uba-related scn) and **row piece SCNs** (if you parse rows). A block with header SCN *lower than* an ITL “known committed SCN” or a row directory entry SCN is internally inconsistent → logical corruption / stale image.

**Why `verdict()` misses it:** you don’t currently cross-check header SCN vs ITL/row/undo metadata; “lost write” is out-of-scope, but **this is not inter-block**—it’s intra-block consistency.

---

### 1.2 ITL / interested transaction list corruption (hidden logical corruption with valid checksum)
**What happens:** Bit flips or overwrites inside the transaction slots cause ORA-600 (kdbchk 610x/611x class), ORA-1578 sometimes *not* raised because checksum matches or is off.

**Byte signature (ktbbh / table directory area):**
- Data blocks (type `0x06`, `ktbbhtyp=1`) have ITL entries with:
  - `xid` (usn|slt|sqn) triplet
  - `uba` (undo record pointer: file#, block#, rec#)
  - flags (active/committed), lock byte, scn/seq depending on version

**Block-only detection:**
- Validate **ITL count vs available space**: ITL slots must fit between header and row directory. If ITL count implies overlap → corruption.
- For each ITL:
  - XID fields not all-zero unless slot unused.
  - If slot marked active/committed, UBA must be non-zero and plausible: `rec# != 0`, `block# != 0`, `file# within 1..maxfile` (use file# plausibility only, not correctness).
  - If block is ASSM-managed, **ITL “cleanout” invariants**: committed ITL should have commit SCN != 0 in releases that store it.

**Why you miss/misclassify:** `structural_check` is referenced but unspecified; most “logical” checks in tools stop at row directory. The ITL/UBA layer is where kdbchk 6101–6160 lives. These corruptions often present as “valid checksum, parseable header” → you’ll currently return `None` (healthy) or generic `Logical` only if your parser happens to trip.

---

### 1.3 Row directory / row piece header internal contradictions (kdbchk 6110/6111 style)
**What happens:** Row count, directory offsets, and row piece headers disagree. Classic “soft corruption”: block readable, checksum OK.

**Byte signature:**
- Row directory is an array of 2-byte offsets (or 4-byte in some formats) near the end of the block.
- Row piece header begins at each offset with flags, column count/length bytes, etc.

**Block-only detection:**
- For each row directory entry:
  - Offset must be within `[header_end .. row_dir_start)` and aligned for the format.
  - Row header must not extend beyond row_dir_start.
  - Row chaining flags must be consistent (e.g., if “forwarded” then has valid forward pointer structure).
- Ensure **monotonicity / non-overlap**: row pieces should not overlap the directory region or each other (allow free space gaps).

**Why you miss it:** Unless LV-062 explicitly does this fully, a lot of these pass “header parsed + tail matches + checksum ok”.

---

### 1.4 Index branch/leaf structural corruption (kdxco / kdxle) that still looks like “Index block”
**What happens:** Index block headers look fine (`ktbbhtyp=2`), but the internal entries are inconsistent: broken sorted order, impossible child pointers, bad prefix compression bytes.

**Byte signature (index block internals):**
- Index blocks contain `kdxco` (common header) and then leaf/branch structures:
  - Leaf entries with key length, rowid, flags
  - Branch entries with key + child DBA
- There are version-specific header bytes, but certain invariants persist.

**Block-only detection:**
- Walk entries until you hit free space; validate:
  - Entry lengths not zero, not exceeding remaining.
  - **Sorted order** of keys within the block (binary compare) for leaf and branch.
  - For branch: child pointer RDBA values must be block-aligned (multiple of block size) and file# plausible.
- Validate `kdxco` declared count vs actual parsable entries.

**Why you miss it:** Your kind classification recognizes index vs data, but you don’t describe any kdx-level checks. These are common “logical corruptions” DBAs get in ORA-600 [kdx*] / ORA-1499 territory.

---

### 1.5 LOB: SecureFile LOB index vs LOB chunk header inconsistencies (block-local checks exist)
**What happens:** LOB metadata blocks (typ=5) and chunk blocks (0x28) can be internally inconsistent even if “looks like LOB”.

**Byte signature:**
- SecureFile LOB chunks have their own mini-header (Oracle calls it a LOB chunk header; fields include chunk id / seq, flags, maybe objd).
- Your `BlockKind::LobChunk (0x28)` is just type-based today.

**Block-only detection:**
- For LOB chunk blocks:
  - Validate chunk header fields: “chunk length” must be <= usable space; flags must be within known set; reserved bytes should be 0 in many builds.
  - If chunk header contains **LOBID / objd / tsn** fields (release-dependent), validate plausibility and internal checksums if present.
- For typ=5 “LOB blocks” (SecureFile):
  - Validate locator-like fixed patterns (you already did for locators inside rows; but the LOB *block* format also has signatures).

**Why you miss it:** `ktbbhtyp=5` mapping is good, but without structural validation you will treat many internally broken LOB blocks as healthy, causing later extraction failures that look “mysterious”.

---

### 1.6 ASSM bitmap block internal validity & contradictions (space management corruptions)
**What happens:** Bitmap blocks (ASSM) get corrupt: wrong header, wrong bitmap sizes, bad nibble values. Oracle raises ORA-8103/ORA-600 depending.

**Byte signature:**
- ASSM blocks are `Space` kinds (`0x1D/0x1E/0x20–0x22` per your mapping).
- They contain bitmap runs with restricted value ranges (0..n classes) and often a small header with block ids / levels.

**Block-only detection:**
- Validate:
  - Header fields: level, parent pointers, first/last block ids (format known per version).
  - Bitmap bytes restricted to known classes; detect impossible values.
  - Count of bits matches expected coverage for block size.

**Why you miss it:** you classify them, but no logical check described. These corruptions can exist with valid checksum and will break extent allocation / freelist logic.

---

### 1.7 Segment header contradictions detectable locally (extent map inside the block)
**What happens:** Segment header block (0x10/0x23 per you) contains extent map entries with impossible DBAs, out-of-order extents, or overlaps—block-local.

**Byte signature:**
- Segment header holds extent descriptors: `(file#, block#, length)` repeated.
- It also stores HWM and space management pointers.

**Block-only detection:**
- Parse extent table:
  - Each extent length > 0 and <= max for TS.
  - Extents are non-overlapping and increasing in DBA order (Oracle keeps them ordered in many cases).
  - HWM must fall within allocated extents.
- Validate that the segment header’s `objd` matches the `objd` field in its own header (if present) and that it’s non-zero.

**Why you miss it:** you count segment headers as `Other` to match dbv (LV-078), but that’s *classification*, not validation. A DBA cares about “segment header corrupt” because it blocks full object access.

---

### 1.8 ORA-08103 “object-id mismatch” signatures inside a single block
**What happens:** Block belongs to a different object than expected (common after transport mishaps or misdirected writes). Even without a data dictionary you can detect **internal object identity mismatch** within the block.

**Byte signature:**
- Many block types carry:
  - `kcbh.objd` (data object id)
  - `ktbbh.objd` or segment-relative identifiers (depends)
  - Sometimes SCN/object sequence in the block tail

**Block-only detection:**
- Cross-check duplicate identity fields:
  - If `kcbh.objd != ktbbh.objd` (for blocks where both exist) → logical corruption / misplacement *even if rdba matches file position*.
- For index blocks: compare `kdxco.objd` (if present) vs `kcbh.objd`.

**Why you miss it:** you only attribute objd “if ktbbh is safe”, but you don’t mention cross-field consistency checks. This is precisely the type of corruption that produces ORA-08103.

---

### 1.9 9i/10g / 32K / big-endian format traps that will produce false “Garbage/Encrypted/Torn”
**What happens:** Real recoveries often involve older files or different endian (AIX/Solaris). Your layout inference relies heavily on “nibble=2, tail matches, rdba matches” which is brittle across formats.

**Byte signature pitfalls:**
- `kcbh` layout (offsets of scn, seq, flags) differs between 9i/10g and later.
- 32K blocks have different internal alignment; some tail fields move.
- Big-endian swaps the “magic-looking” constants; your OS block0 magic `0x7A7B7C7D` will appear byte-swapped.

**Block-only detection:**
- Maintain **per-version kcbh parsers** keyed by `kcbh.bhver`/`kcbh.fmt` byte if present; otherwise do multi-hypothesis parsing and score.
- For endian: detect plausible `kcbh.type`/`rdba` under both endian interpretations.

**Why you miss it:** doc says “all sizes × both endians”, but the actual scoring rules (“nibble==2”) can reject older legitimate formats and produce misclassification cascades (Encrypted/Garbage).

---

### 1.10 TDE-encrypted tablespace vs ransomware: your current “Encrypted = unparseable + entropy≥7.85” is wrong in Oracle terms
**Fact (critical):** In Oracle TDE tablespace encryption, the **Oracle block header (kcbh) is NOT encrypted**. The encrypted part is the block payload (and sometimes parts of tail), but headers remain parseable specifically so Oracle can do IO and checks.

**Byte-level implication:**
- A TDE block will still have valid `kcbh` (type, rdba, scn, flags) and often a **checksum that is computed over ciphertext** (implementation detail varies).
- Entropy of payload can be high, but block is still structurally parseable. Your engine will likely mark it `None` (healthy), not `Encrypted`, and you will **miss “ransomware” detection if attacker encrypts only payload but keeps headers**.

**Block-only discriminator:**
- If header parses and rdba matches, but payload is high-entropy and **contains no recognizable internal sub-structures for that block kind** (rows/index entries/ASSM bitmaps), this suggests “payload encrypted without Oracle awareness” (ransomware) vs legitimate TDE.
- Conversely, legitimate TDE often has consistent internal format markers for encrypted blocks (e.g., flags in `kcbh.flg` indicating encryption; also TS-level encryption is known from file header / tablespace flags in `kcvfh` if you parse it).

**Why you misclassify:** you equate “encrypted” with “unparseable”; that’s not true for TDE or for many ransomware families that avoid touching headers.

---

## 2) Algorithms / heuristics (pseudo-code) for the top 8 gaps + false-positive risks

Below are **block-only** algorithms that can run in `--deep` and feed `Damage::Logical(fault=...)` with strong evidence.

### 2.1 ITL sanity + UBA plausibility (data blocks)
```pseudo
function check_itl(block):
  h = parse_kcbh(block)
  t = parse_ktbbh(block)  // data header
  itl = parse_itl_array(block, t.itl_count)

  assert itl_region_end <= row_dir_start  else fault("ITL_OVERLAP")

  for i, e in enumerate(itl):
    if e.is_unused():
      continue
    if e.xid.usn==0 or e.xid.slt==0 or e.xid.sqn==0:
      fault("ITL_XID_ZERO", slot=i)
    if e.uba.is_zero():
      fault("ITL_UBA_ZERO", slot=i)
    if e.uba.rec == 0:
      fault("ITL_UBA_BADREC", slot=i)
    if not plausible_file(e.uba.file) or e.uba.block==0:
      fault("ITL_UBA_BADDBA", slot=i)
```
**False-positive risk:** medium on very old versions / unusual states (freshly formatted blocks, delayed cleanout). Mitigate by requiring **multiple** violations or coupling with row-dir sanity.

---

### 2.2 Row directory + row piece bounds and non-overlap (data blocks)
```pseudo
function check_row_directory(block):
  t = parse_ktbbh(block)
  dir = parse_row_dir(block, t.row_count)

  for off in dir:
    if off == 0: continue  // deleted entry
    if off < min_row_start or off >= row_dir_start:
      fault("ROW_OFF_OOB", off)
    rh = parse_row_header_at(block, off)
    if off + rh.total_len > row_dir_start:
      fault("ROW_OVER_DIR", off)
  if rows_overlap(dir, parsed_row_lengths):
      fault("ROW_OVERLAP")
```
**False-positive risk:** low if parser supports row chaining/forwarding variants; otherwise medium. You must model row header variants accurately.

---

### 2.3 Index block entry walk + sorted order (kdx)
```pseudo
function check_index_block(block):
  kdx = parse_kdxco(block)
  entries = []
  p = kdx.first_entry
  while p < kdx.free_space_start:
    e = parse_kdx_entry(block, p)
    if e.len <= 0 or p+e.len > kdx.free_space_start:
      fault("KDX_LEN_BAD", p)
    entries.append(e.key_bytes)
    if kdx.is_branch:
      if not plausible_rdba(e.child_rdba): fault("KDX_CHILD_BAD")
    p += e.len
  if not is_sorted(entries):
    fault("KDX_NOT_SORTED")
```
**False-positive risk:** medium because key comparison must honor Oracle’s internal collation/NULL semantics and prefix compression. Mitigate by checking only **binary non-decreasing** for raw bytes and flag as “suspect” not “proven” unless other invariants fail.

---

### 2.4 Segment header extent map validation
```pseudo
function check_segment_header(block):
  sh = parse_seg_header(block)
  extents = parse_extent_array(block)
  if any(ext.len==0 or ext.block==0 or ext.file==0): fault("EXT_BAD")
  if overlaps_or_unsorted(extents): fault("EXT_OVERLAP_OR_ORDER")
  if sh.hwm not within any extent range:
     fault("HWM_OUTSIDE_EXTENTS")
```
**False-positive risk:** medium: Oracle can store “pending” extents or special markers. Mitigate by whitelisting known marker values per version and accepting gaps.

---

### 2.5 ASSM bitmap internal constraints
```pseudo
function check_assm_bitmap(block):
  bm = parse_assm_header(block)
  if bm.level > bm.max_level: fault("ASSM_LEVEL_BAD")
  bits = parse_bitmap_payload(block)
  if any(bitval not in allowed_classes): fault("ASSM_CLASS_BAD")
  if coverage(bits) != expected_coverage(block_size, bm.level):
      fault("ASSM_COVERAGE_BAD")
```
**False-positive risk:** medium-high unless you implement exact ASSM formats per release. Recommend initially: only enforce **value-range** + **basic length** checks.

---

### 2.6 Cross-field object identity mismatch (ORA-08103 style)
```pseudo
function check_objd_consistency(block):
  h = parse_kcbh(block)
  if block.kind in {Data, Index, Lob}:
     t = parse_ktbbh_or_kdxco(block)
     if t.objd != 0 and h.objd != 0 and t.objd != h.objd:
         fault("OBJD_MISMATCH", h.objd, t.objd)
```
**False-positive risk:** low; when both are present they should match. Biggest risk is “field not present / parser wrong for version”.

---

### 2.7 TDE-vs-ransomware discriminator using “header-valid + payload-structure score”
```pseudo
function classify_crypto(block):
  if not header_parses: return (Encrypted|Garbage via entropy)
  // header parses => could be TDE, could be ransomware preserving header
  s = structure_score(block.kind, block.payload)
  e = shannon_entropy(block.payload)
  if e > 7.85 and s very low:
      return Damage::EncryptedPayloadSuspect
  else:
      return None
```
**False-positive risk:** medium: compressed blocks and binary LOB payload can also be high entropy and low structure. Needs better discriminator (see §3).

---

### 2.8 Multi-hypothesis layout parsing for old versions / endian
```pseudo
function infer_layout(file_sample_windows):
  candidates = []
  for bs in [2k..32k..64k?] and endian in [LE, BE] and fmt in [11g, 10g, 9i]:
     score = 0
     for each sampled block:
        if parses_kcbh(block, bs, endian, fmt) and tail_ok(block) and rdba_matches(block):
            score++
     candidates.append((bs,endian,fmt,score))
  pick best if margin rules satisfied
```
**False-positive risk:** low if you require strong margin; but can “reject too often” on heavily corrupted files. That’s acceptable per your “refuse vs guess” principle.

---

## 3) What entropy threshold 7.85 will get wrong, and a better discriminator

### 3.1 False negatives (missed ransomware)
- **Ransomware that preserves Oracle headers** and encrypts only payload: entropy of *whole block* may be dragged down by header + directory zeros; your “unparseable” predicate will be false because kcbh parses, so you won’t even compute entropy (by design: “entropy only for non-healthy”). Result: you may label as `None` and miss encryption.

### 3.2 False positives (legitimate high-entropy payloads)
- **HCC/OLTP compression**: compressed fragments can approach random-like entropy, especially on small samples (8KB).
- **SecureFile LOB binary data** (already compressed or encrypted at application layer): can exceed your 7.85 threshold on payload-only.
- **TDE tablespace encryption**: payload entropy high, but it’s legitimate.

### 3.3 Better discriminator than a single Shannon threshold
Use a **two-feature** or **three-feature** classifier on *payload region only* (exclude kcbh + fixed headers + row directory area):
1. **Entropy (H)** of payload.
2. **Byte frequency chi-square vs uniform** (χ²): encrypted data is close to uniform; compressed data often still shows bias.
3. **n-gram repetition / LZ complexity**: encrypted has minimal repeats; some compression retains patterns.

Then incorporate **structural score** appropriate to block kind:
- Data block: ability to parse row directory and at least N row headers consistently.
- Index block: ability to parse entry walk and see monotonic offsets.
- ASSM: allowed bitmap classes distribution.

A practical rule set:
- If header parses, rdba matches, checksum ok, but **structure_score == 0** AND `(H>7.85 AND χ² within tight band)` → “payload-encrypted/scrambled”.
- If structure_score > 0 → not ransomware-encrypted at block level (could be TDE; handle separately).

Also: if you can parse `kcvfh` and it indicates **tablespace encryption enabled**, treat high-entropy payload as “likely TDE” not “ransomware”, unless structure_score collapses in a way inconsistent with Oracle’s own encrypted format.

---

## 4) Design elements that are WRONG (or dangerously misleading)

### 4.1 **TDE handling is conceptually wrong**
Your current statement/logic: “Encrypted = not parseable AND entropy ≥ 7.85”.  
Oracle reality: with TDE tablespace encryption, **kcbh is readable**; most blocks remain parseable at header level. Therefore:
- You will not label TDE as encrypted (fine), but you also **cannot use ‘unparseable’ as a proxy for ‘encrypted’**.
- Worse: you may miss ransomware that encrypts payload but keeps headers.

You need two orthogonal concepts:
- `EncryptedByOracle(TDE)` (detectable via file header / TS flags + header flags)
- `EncryptedOrScrambledExternally` (payload looks cipher-like + structure destroyed)

### 4.2 `ChecksumFail` gating and `DB_BLOCK_CHECKSUM=OFF`
You currently say: “ChecksumFail — flag raised and XOR != 0”. In practice:
- Many environments run with checksum OFF; then you’ll never raise `ChecksumFail`.
- You must treat “checksum not enabled” as an **evidence limitation**, not as health. Otherwise you’ll mislead forensic users (“no physical corruption”) when you simply lack the check.

Report should include per-file: “checksum present/absent in header; verification coverage”.

### 4.3 `MarkedCorrupt` = `seq_kcbh = 0xFF` is incomplete
`DBMS_REPAIR` marking and media manager marks do show up via seq/flags, but:
- There are multiple “poisoned block” encodings across versions (not always seq=0xFF alone).
- Also, seq=0xFF can appear in other contexts or be overwritten coincidentally in random data—needs corroboration (type/rdba/tail).

Treat `seq=0xFF` as **strong indicator** only if `kcbh.type` is plausible and tail matches.

### 4.4 Treating “tail mismatch” as `Torn` is overconfident
A tail mismatch can come from:
- wrong endian/format hypothesis,
- partial overwrite of tail only,
- block size mis-detection,
- encryption/payload scramble that happened to preserve some header bytes.

So `Torn` should require additional evidence: e.g., header parses, rdba matches, checksum fails or SCN/seq implausible, and tail signature mismatch under the *correct* layout hypothesis.

### 4.5 dbv parity is not sufficient validation (and can constrain you incorrectly)
You intentionally match dbv categories (LV-078), including dbv’s oddity “Seg is only 0x0E” and counting segment headers as Other. That’s fine for acceptance tests, but **do not let dbv semantics dictate your internal model**:
- Segment headers (0x10/0x23) are critical forensic artifacts; hiding them under Other in reporting reduces value.
- You can still match dbv counters while providing richer internal classification.

### 4.6 Layout inference scoring (“nibble=2”) is brittle
“nibble of format = 2” as a primary signature is risky:
- Older formats / big-endian / 32K may not satisfy it reliably.
- Random data has 1/16 chance to match it, which you already saw leaking into Torn classification.

Make the score a weighted combination of *many* invariants: type range, rdba match, sane scn/seq progression ranges, checksum field presence, tail signature, block size alignment.

### 4.7 Object summary attribution policy can hide misdirected writes
You refuse objd attribution on checksum fail (good). But misdirected writes can have valid checksum and valid ktbbh yet belong to a different file position. Your object summary will count them under the “wrong” object unless you also apply `Misplaced` earlier and **exclude misplaced blocks from object stats** (or split them into “objd-at-header vs objd-at-position”).

### 4.8 “Precision 100%: no untouched block flagged” is unrealistic once you add logical checks
As soon as you implement deeper invariants (kdbchk-like), you will inevitably encounter blocks that Oracle tolerates (edge states, delayed cleanout, format variance) but your checker flags. Forensics tools must accept:
- precision will be <100% unless you scope checks carefully and include “Suspect” vs “Proven” levels.
Your current acceptance criterion will discourage necessary checks.

---

If you want, I can map the kdbchk 6101–6160 families to concrete invariants (ITL/rowdir/free-space/lock byte/ctc) and propose a fault taxonomy that stays evidentiary (“this field implies overlap at byte offsets X..Y”) rather than Oracle-error-name guessing.