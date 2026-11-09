# Agent C_algorithms_eng (gpt-5.3-codex)

_Generated 2026-11-09 05:21 UTC, 82s_

# 1) Performance plan: reach **>=300 MB/s on 4 cores** (SHA-256, no SHA-NI)

Your current design already removed the worst bottleneck (`vec![len]` + 3 passes). To consistently hit 300+ MB/s, make the scanner an explicit **3-stage pipeline** with bounded queues and chunk-parallel classification.

## 1.1 Pipeline architecture

Use fixed **4 MiB chunks** (already aligned with your design, and 512 × 8 KiB blocks/chunk):

1. **Reader thread (single producer)**  
   - Reads sequentially using `pread`/`io_uring` into a reusable slab of chunk buffers.
   - Applies readahead hints.
   - Emits `(chunk_id, file_offset, len, Arc<[u8]>)`.

2. **Hash thread (ordered consumer)**  
   - Receives chunks in order.
   - Updates `sha2::Sha256` incrementally (or `blake3::Hasher` in fast mode).
   - Emits hash progress + forwards chunk to classify queue.

3. **Classifier pool (Rayon, N workers)**  
   - Splits each chunk into blocks (`block_size`).
   - Parses/verdict per block in parallel.
   - Writes only compact results (counters, regions, suspect list), never block copies.

4. **Aggregator thread**  
   - Merges partial counters and damage spans in file order.
   - Tracks layout-confidence stats, object summary, and bounded suspect reservoir.

This is effectively zero-copy at the block layer (no per-block allocations), and one-copy at read boundary only.

## 1.2 I/O strategy choices

### Linux default (recommended)
- `rustix` for `posix_fadvise(SEQUENTIAL|WILLNEED)` and optionally `readahead()`.
- `std::os::unix::fs::FileExt::read_at` for robust positioned reads.
- Why: predictable, portable enough, lower complexity than full `io_uring`, excellent for sequential scans.

### Linux high-end option
- `io-uring` crate (`io-uring` or `tokio-uring` low-level mode), queue depth 32–64, fixed buffers.
- Why: better overlap under high latency (network/NAS), lower syscall overhead.

### mmap?
- `memmap2` as **opt-in** for local SSD/tmpfs only.
- Do **not** default to mmap for multi-TB or flaky network shares (SIGBUS risk / hard fault behavior).

## 1.3 Crates

- `rayon` – mature parallel chunk/block map-reduce.
- `crossbeam-channel` – low-latency bounded queues between stages.
- `sha2` – standard SHA-256 evidence hash.
- `blake3` – fast optional hash mode (triage).
- `rustix` – fadvise, seek-hole/data, robust unix ops.
- `memmap2` – optional fast path.
- `smallvec` – avoid heap for short per-block evidence vectors.
- `ahash` or `hashbrown` – fast maps for object counters.
- `serde`/`serde_json` – report serialization.
- `rustfft` – periodicity FFT (later section).

## 1.4 Throughput target math

300 MB/s overall with 2 passes (open-hash + scan/hash-after) means ~600 MB/s raw read demand. On NVMe/tmpfs that is realistic.  
CPU side: 4 cores × software SHA-256 + parsing should be fine if:
- Chunk size >= 2 MiB (use 4 MiB).
- Minimal allocations.
- Entropy/randomness tests only on suspect blocks.
- Deep logical checks optionally sampled or `--deep`.

---

# 2) Robustness: never crash, always partial results

## 2.1 Multi-TB safety

- All offsets/counts in `u64`; use checked arithmetic:
  - `byte_off = block_no.checked_mul(block_size as u64)`
  - reject overflow as diagnostic error, not panic.
- Process streamingly; never allocate by file length.
- Keep only:
  - global counters,
  - active region state,
  - bounded suspect lists,
  - object map (capped / spill).

## 2.2 Sparse files

Detect holes with `SEEK_DATA/SEEK_HOLE` (`rustix::fs::lseek`) and annotate:
- Hole ranges reported as `SparseHole`, not `Zeroed` (different meaning).
- If filesystem doesn’t support, fall back to normal reads.

## 2.3 Files open by Oracle

Open read-only with shared semantics:
- Linux: `O_RDONLY|O_CLOEXEC`, no lock escalation.
- Windows: `FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE`.
Capture metadata at start/end (`size`, `mtime`, inode/fileid).  
If changed during run: mark `evidence.changed_during_scan = true` and downgrade confidence; still emit report.

## 2.4 Mid-file EIO and network instability

On read failure for `[off, off+len)`:
1. Binary-split interval until block granularity.
2. Mark unreadable block(s) as `UnreadableIO { errno }`.
3. Continue with next block/chunk.

This guarantees progress and precise bad-range reporting. Never abort full scan for localized EIO.

## 2.5 Memory caps

Configurable caps:
- `--max-suspects` (default 200k records, as you have).
- `--max-regions` (e.g., 2M then coalesce aggressively).
- `--max-objects` (e.g., 1M; overflow spills to temp sorted runs).
- `--max-evidence-bytes` for JSON detail.

When cap is hit: emit `"truncated_detail": true` but keep summary correct.

---

# 3) Missing algorithmic power (high impact)

## 3.a Beyond XOR collisions: structural re-verification

LV-079 is correct: XOR checksum misses paired bit flips. Add **row/block internal consistency scoring** for `0x06` blocks even when checksum passes:

- Validate `ktbbh`, `kdbh`, `kdbr[]` directory bounds.
- Row offsets monotonic and inside block.
- Row header flags + column count sane.
- Column length encodings within row boundary.
- ITL/row lock bytes consistent.
- Optional cross-check: recompute free-space map consistency.

Output `Damage::Logical("struct_mismatch")` with evidence offsets. This catches checksum-collision tampering and random paired flips.

## 3.b Encrypted vs compressed discrimination (opaque blocks)

For blocks that fail Oracle parse, classify via multi-test randomness score (not entropy alone):

Metrics per block:
- Shannon entropy `H`
- Chi-square p-value over 256 bins
- Serial correlation coefficient `r`
- Monte-Carlo π error `|pi_est - π|`
- Fast LZ4 compression ratio `cr = compressed_len / raw_len`

Suggested thresholds (8 KiB blocks):
- **Likely encrypted/random**:
  - `H >= 7.90`
  - `0.01 <= p_chi <= 0.99`
  - `|r| <= 0.03`
  - `|pi_err| <= 0.02`
  - `cr >= 0.98`
- **Likely compressed-structured/other opaque**:
  - `7.65 <= H < 7.90` and at least one non-random indicator  
    (`p_chi < 1e-3` or `|r| > 0.05` or `cr < 0.97`)

Important: bytes-only cannot perfectly distinguish encryption vs high-grade compression. Report as confidence class:
- `EncryptedHighConfidence`
- `HighEntropyOpaque`

## 3.c Periodicity detection via autocorrelation/FFT (replace gap voting)

Build binary damage signal over block index: `x[i]=1 if damaged else 0`.  
Compute:
- autocorrelation peaks for lags `k`,
- FFT power spectrum (`rustfft`) to detect dominant frequency.

Detection rule:
- Require at least 4 repeats and peak prominence >= 6 dB above local baseline.
- Convert frequency to period `P` blocks.
- Accept if median phase residual <= 1 block.

This will outperform simple equal-gap voting for jittered ransomware periodic patterns.

## 3.d Carving orphan Oracle blocks from RAW images/unallocated space

Scan raw media in 512B steps (or 4KiB if performance mode), test candidate 8KiB-aligned windows:

Signature checks:
1. `kcbh` parse valid, format nibble == 2.
2. Tail/header consistency holds.
3. `rdba` self-consistent with candidate logical position hypothesis (smallfile/bigfile model).
4. Optional: checksum valid or structural score high.

Cluster carved blocks by `(file#, block# range, SCN continuity)` into candidate files.  
This gives true forensic carving capability beyond DBF containers.

## 3.e Deleted-row recovery from `kdbr` slots

From data blocks passing structural checks:
- Inspect row directory slots pointing to row pieces marked deleted/migrated.
- Recover payload if bytes still intact and not reallocated.
- Label recovered rows as `deleted_inferred`, with slot index and confidence.
- Never mix with verified live rows.

This is a major differentiator for incident response and forensic value.

## 3.f Version reconciliation across two damaged copies

Given two images of same file:
- Match blocks by block number.
- Score each candidate copy with:
  - readability/parse,
  - checksum/tail validity,
  - structural score,
  - SCN freshness (per segment/object neighborhood).
- Select winner per block; emit merged file + provenance map.

This “best-of-both” merge is very practical for partially damaged backups.

## 3.g Reed-Solomon-like recovery feasibility

From a **single Oracle datafile alone**, RS-style reconstruction is generally impossible:
- No parity shards/redundant coding within file format for arbitrary missing blocks.
- XOR checksum is integrity, not erasure recovery.
Recovery requires external redundancy (ASM mirror, storage RAID parity, backup copies, redo/standby).

State this explicitly in docs to avoid overclaiming.

---

# 4) Hidden-damage detectors to add now

1. **STALE valid blocks**  
   Block structurally valid but SCN far behind neighbors/segment header.  
   Detector: per object, rolling median SCN; flag if  
   `median_scn - block_scn > max(1_000_000, 10*MAD)`.

2. **Duplicate `rdba` in same file**  
   Same rdba appears at two physical locations.  
   Build `(rdba -> first_lbn)` map (or external sort for huge files) and flag duplicates.

3. **Swapped files**  
   `kcvfh.file#` / dominant `rdba.file#` mismatch expected file identity.  
   Also detect if filename suffix (e.g., `users01`) conflicts with header file number (heuristic confidence only).

4. **Wrong declared block size**  
   If `kcvfh` says `bsz`, but kcbh-inference strongly prefers another (`score gap >=0.25`), flag `HeaderBlockSizeMismatch`.

5. **Silent truncation masked by copied header**  
   Header says large `fsz`, but physical EOF earlier; already in `TRUNCATED`, but add explicit evidence:
   `declared_blocks`, `actual_blocks`, `missing_tail_blocks`.

6. **Cross-file block transplant**  
   Valid block with plausible structure but `rdba.file#` belongs to other known file in same incident set.  
   Flag as `MisdirectedCrossFile`.

---

# 5) Rust-level pseudocode (top 6)

## 5.1 Streaming pipeline engine

```rust
fn diagnose(path: &Path, cfg: Cfg) -> Report {
    let (tx_read, rx_read) = bounded::<Chunk>(cfg.qd);
    let (tx_hashed, rx_hashed) = bounded::<Chunk>(cfg.qd);

    spawn_reader(path, cfg, tx_read);          // pread/io_uring + readahead
    spawn_hasher(rx_read, tx_hashed, cfg.hash);// sha256 or blake3 ordered
    let pool = rayon::ThreadPoolBuilder::new().num_threads(cfg.workers).build().unwrap();

    let mut agg = Aggregator::new(cfg);
    while let Ok(chunk) = rx_hashed.recv() {
        let partial = pool.install(|| classify_chunk_parallel(&chunk, cfg));
        agg.merge(partial);
    }
    agg.finish()
}
```

## 5.2 EIO-resilient block isolation

```rust
fn read_range_resilient(file: &File, off: u64, len: usize, bs: usize, out: &mut [u8], errs: &mut Vec<IoErrRange>) {
    match file.read_at(out, off) {
        Ok(n) if n == len => return,
        Ok(n) => mark_short_read_as_truncated(off, n, len, bs, errs),
        Err(e) if e.raw_os_error() == Some(libc::EIO) => {
            if len <= bs {
                errs.push(IoErrRange { off, len, errno: libc::EIO });
                out.fill(0); // placeholder, classifier marks UnreadableIO
            } else {
                let mid = (len / 2 / bs) * bs;
                read_range_resilient(file, off, mid, bs, &mut out[..mid], errs);
                read_range_resilient(file, off + mid as u64, len - mid, bs, &mut out[mid..], errs);
            }
        }
        Err(e) => errs.push(IoErrRange { off, len, errno: e.raw_os_error().unwrap_or(-1) }),
    }
}
```

## 5.3 Multi-metric opaque-block classifier

```rust
fn classify_opaque(block: &[u8]) -> OpaqueClass {
    let h = shannon_entropy(block);
    let p_chi = chi_square_pvalue(block);
    let r = serial_corr(block);
    let pi_err = monte_carlo_pi_error(block);
    let cr = lz4_ratio(block);

    let random_like = h >= 7.90
        && (0.01..=0.99).contains(&p_chi)
        && r.abs() <= 0.03
        && pi_err <= 0.02
        && cr >= 0.98;

    if random_like { OpaqueClass::EncryptedHighConfidence }
    else if h >= 7.65 { OpaqueClass::HighEntropyOpaque }
    else { OpaqueClass::GarbageLikely }
}
```

## 5.4 Periodicity via FFT/autocorrelation

```rust
fn detect_periodic_damage(damaged: &[bool]) -> Option<PeriodHit> {
    let x: Vec<f32> = damaged.iter().map(|&b| if b {1.0} else {0.0}).collect();
    let ac = autocorr(&x, max_lag = x.len()/4);
    let peak = find_prominent_peak(&ac, min_repeats=4, min_prom_db=6.0)?;
    let fft_hit = dominant_frequency(&x)?; // rustfft
    if consistent(peak.period, fft_hit.period, tol=1) {
        Some(PeriodHit { period_blocks: peak.period, confidence: combine(peak, fft_hit) })
    } else { None }
}
```

## 5.5 RAW image orphan block carver

```rust
fn carve_oracle_blocks(img: &mut dyn ReadAt, cfg: CarveCfg) -> Vec<CarvedBlock> {
    let mut out = Vec::new();
    for off in (0..cfg.image_len).step_by(cfg.step) {
        let buf = read_window(img, off, cfg.block_size);
        if let Some(h) = parse_kcbh(&buf, cfg.endian) {
            if h.format_nibble == 2 && tail_matches(&buf, &h)
               && rdba_self_consistent(&h, off, cfg) {
                let score = structural_score(&buf);
                out.push(CarvedBlock { off, rdba: h.rdba, scn: h.scn, score });
            }
        }
    }
    cluster_by_file_and_sequence(out)
}
```

## 5.6 Two-copy reconciliation merge

```rust
fn reconcile(a: &FileImg, b: &FileImg, layout: Layout) -> Merged {
    let mut merged = Merged::new(layout.blocks);
    for blk in 0..layout.blocks {
        let ca = assess_block(a.read_block(blk), blk);
        let cb = assess_block(b.read_block(blk), blk);
        let winner = choose_best(ca, cb); // validity > structural > freshness(SCN)
        merged.write_block(blk, winner.bytes);
        merged.provenance.push((blk, winner.source, winner.score));
    }
    merged
}
```

---

# 6) Test plan (quantitative, acceptance-focused)

## 6.1 Performance benchmarks

1. **Single large file (>=200 GB), tmpfs/NVMe**
   - `--fast`, SHA-256, 4 workers.
   - Target: >=300 MB/s end-to-end reported throughput.
2. Compare backends:
   - pread+fadvise vs io_uring vs mmap (local only).
3. CPU profiles:
   - Hash thread utilization, classifier utilization, queue occupancy.

## 6.2 Robustness matrix

- Multi-TB sparse synthetic file (holes + data): no OOM, correct hole accounting.
- File mutating during scan: report generated with `changed_during_scan=true`.
- Injected EIO at random ranges: zero panics; unreadable ranges localized to block granularity.
- SMB/NFS throttling/disconnect simulation: graceful partial report + errors list.

## 6.3 Detection quality

1. Existing LV acceptance remains (dbv parity, recall, precision).
2. New detectors:
   - stale SCN: seeded stale blocks in otherwise valid segments; target recall >99%, FP <0.1%.
   - duplicate rdba/transplant: 100% detection in synthetic swaps.
3. Opaque classifier:
   - corpora: true encrypted, compressed non-Oracle, random, garbage text.
   - report confusion matrix; target encrypted precision >98% for `HighConfidence` bucket.

## 6.4 Periodicity detector validation

- Generate ransomware patterns with jitter ±1 block, varying period.
- Require detection for periods 8..8192 blocks with >=4 repeats.
- Compare against old gap-voting baseline; measure F1 improvement.

## 6.5 Carving and deleted-row recovery

- RAW images with embedded Oracle blocks + decoys.
- Metrics:
  - block carving precision/recall,
  - correct `(file#, block#)` assignment rate,
  - deleted-row byte-equality against ground truth dumps.

## 6.6 Reconciliation tests

- Two copies with complementary corruption masks.
- Validate merged image has strictly fewer damaged blocks than each input.
- Provenance map deterministic and auditable.

---

## Bottom line

Your current `odr diagnose` direction is strong and evidence-driven. The biggest upgrades now are:

1) production-grade streaming pipeline with resilient I/O error isolation,  
2) deeper logical verification to cover XOR blind spots,  
3) FFT/autocorrelation periodicity,  
4) carving + two-copy reconciliation for real recovery lift,  
5) hidden-damage detectors (stale/duplicate/swapped/mismatch) to catch “valid-but-wrong” blocks.

If you implement the pipeline and caps as above, **300 MB/s on 4 cores is realistic** without SHA-NI, and robustness will be at forensic-tool level rather than utility-tool level.