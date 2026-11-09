//! `odr diagnose` — corruption detection and explanation engine
//! (`docs/design/diagnose.md`, REQ-001, REQ-005).
//!
//! Pipeline: [`layout::decide`] → streamed [`verdict::verdict`] per block →
//! contiguous damage [`Region`]s + per-object summary → [`pattern::findings`].
//!
//! Memory is `O(regions + objects + reported blocks)`, never `O(file)`; a
//! 1 TB file streams through a 4 MiB buffer. The SHA-256 "after" hash is
//! computed from the very bytes being judged, so one pass both classifies
//! and proves the evidence did not change under us (ADR-0002).

pub mod db3f;
pub mod html;
pub mod layout;
pub mod pattern;
pub mod triage;
pub mod verdict;

pub use layout::{Layout, LayoutSource, ReadAt};
pub use pattern::{Finding, FindingCode, Severity};
pub use triage::{Band, Triage, triage};
pub use verdict::{BlockKind, Damage, DbvClass, Depth, Verdict, entropy_milli, verdict};

use crate::row::kdb::StructuralFault;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Streaming buffer: a multiple of every legal block size.
pub const CHUNK_LEN: usize = 4 * 1024 * 1024;

/// Options for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    /// Physical only, or physical + logical.
    pub depth: Depth,
    /// Keep at most this many per-block damage records (the counters and
    /// regions are always complete).
    pub max_block_records: usize,
    /// Keep at most this many clean-block witnesses `(index, objd, scn,
    /// rdba)` for the consistency post-pass (stale SCN, duplicate `rdba`).
    /// 16 bytes each; the default covers a 64 GiB file at 8 KiB blocks
    /// (8 M blocks = 128 MiB). Beyond the cap the post-pass is **skipped**
    /// and the report says so (`consistency_pass`).
    pub max_witnesses: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            depth: Depth::Logical,
            max_block_records: 200_000,
            max_witnesses: 8 * 1024 * 1024,
        }
    }
}

/// Did the consistency post-pass (stale SCN / duplicate `rdba`) run?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsistencyPass {
    /// Ran over every clean `0x06` block.
    Complete,
    /// Too many witnesses for [`Options::max_witnesses`]: skipped. The
    /// report must disclose this as *absence of evidence*.
    Skipped,
}

/// Minimum SCN gap below the object median before a block is called stale,
/// regardless of spread. One million SCNs is many hours of activity on a
/// busy instance and weeks on an idle one — a block that far behind its
/// siblings was not written in the same era.
pub const STALE_SCN_MIN_GAP: u64 = 1_000_000;
/// Spread multiplier: a block is stale when `median − scn > max(MIN_GAP,
/// STALE_SCN_MAD_MULT × MAD)`.
pub const STALE_SCN_MAD_MULT: u64 = 10;
/// Objects with fewer clean blocks than this have no meaningful median.
pub const STALE_SCN_MIN_BLOCKS: usize = 8;

/// A run of consecutive blocks sharing the same damage code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    /// First block.
    pub start: u64,
    /// Number of blocks.
    pub len: u64,
    /// Damage code (see [`Damage::code`]).
    pub code: String,
}

impl Region {
    /// One past the last block.
    pub const fn end(&self) -> u64 {
        self.start.saturating_add(self.len)
    }
}

/// One damaged block, for the detailed list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DamagedBlock {
    /// Block number.
    pub index: u64,
    /// What the header said it was (if readable).
    pub kind: BlockKind,
    /// What is wrong.
    pub damage: Damage,
    /// `type_kcbh` if readable.
    pub type_byte: Option<u8>,
}

/// Per-`DATA_OBJECT_ID` summary (REQ-001.5). Only physically clean `0x06`
/// blocks contribute an `objd`; damaged blocks are tallied under
/// [`Report::unattributed_damaged`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectSummary {
    /// Blocks by kind (`Data`/`Index`/`Lob`/other) — all clean.
    pub blocks: u64,
    /// Of which table data blocks.
    pub data_blocks: u64,
    /// Of which index blocks.
    pub index_blocks: u64,
    /// Of which LOB blocks.
    pub lob_blocks: u64,
    /// Clean data blocks that failed the logical check.
    pub logical_faults: u64,
    /// Live rows counted on clean data blocks (logical depth only).
    pub live_rows: Option<u64>,
    /// Lowest block number seen.
    pub first_block: u64,
    /// Highest block number seen.
    pub last_block: u64,
}

/// Counters by `dbv` class and by damage code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    /// Blocks examined (excluding block 0, like `dbv`).
    pub examined: u64,
    /// Formatted, clean blocks by `dbv` class.
    pub data: u64,
    /// Index.
    pub index: u64,
    /// LOB.
    pub lob: u64,
    /// Undo segment header (`0x0E`). **Also counted in `other`**, as `dbv`
    /// does (LV-078).
    pub seg: u64,
    /// Other formatted (includes `seg`).
    pub other: u64,
    /// Formatted, never used.
    pub empty: u64,
    /// By damage code.
    pub damaged: BTreeMap<String, u64>,
    /// Highest SCN seen on a clean block.
    pub highest_scn: Option<u64>,
    /// Clean, formatted, non-empty blocks whose header carried the
    /// checksum flag (`flg_kcbh & 0x04`). `DB_BLOCK_CHECKSUM=OFF` leaves
    /// it clear on every block: the report must then say that bit rot
    /// *cannot* be detected by checksum (panel §1.2).
    pub checksum_flagged: u64,
}

impl Counters {
    /// Total damaged blocks.
    pub fn damaged_total(&self) -> u64 {
        self.damaged
            .values()
            .copied()
            .fold(0u64, u64::saturating_add)
    }
    /// Blocks that are formatted and in use (not empty, not damaged).
    /// `seg` is excluded because it is already inside `other`.
    pub fn used_clean(&self) -> u64 {
        [self.data, self.index, self.lob, self.other]
            .iter()
            .fold(0u64, |a, &b| a.saturating_add(b))
    }
    /// Share of used clean blocks that carry a checksum, in per-mille.
    /// `None` when nothing is in use.
    pub fn checksum_coverage_milli(&self) -> Option<u32> {
        let used = self.used_clean();
        if used == 0 {
            return None;
        }
        let m = self
            .checksum_flagged
            .min(used)
            .saturating_mul(1000)
            .checked_div(used)
            .unwrap_or(0);
        u32::try_from(m).ok()
    }
}

/// Full diagnosis of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Decided layout and how.
    pub layout: Layout,
    /// File length in bytes.
    pub file_len: u64,
    /// Blocks actually present (`file_len / block_size`, rounded up).
    pub blocks_present: u64,
    /// Whether the last block is partial.
    pub last_block_partial: bool,
    /// SHA-256 of the bytes as scanned (hex).
    pub sha256_scanned: String,
    /// Counters.
    pub counters: Counters,
    /// Contiguous damage regions, in file order.
    pub regions: Vec<Region>,
    /// Detailed damaged blocks (capped by [`Options::max_block_records`]).
    pub damaged_blocks: Vec<DamagedBlock>,
    /// Whether `damaged_blocks` was capped.
    pub damaged_blocks_truncated: bool,
    /// Per-object summaries, keyed by `DATA_OBJECT_ID`.
    pub objects: BTreeMap<u32, ObjectSummary>,
    /// Damaged `0x06`-looking blocks whose `objd` could not be trusted.
    pub unattributed_damaged: u64,
    /// Logical faults by rule name.
    pub logical_by_rule: BTreeMap<String, u64>,
    /// Whether the stale-SCN / duplicate-`rdba` post-pass ran.
    pub consistency_pass: ConsistencyPass,
    /// Printable excerpt of the file tail (last block that is neither an
    /// Oracle block nor zero), for ransomware footers / appended notes
    /// (panel §1.7). Hex of the first 64 bytes plus an ASCII rendering.
    pub tail_excerpt: Option<TailExcerpt>,
    /// Interpreted findings, most severe first.
    pub findings: Vec<Finding>,
    /// Recoverability score, verdict and next steps. Filled by
    /// [`Report::with_triage`]; `None` straight out of [`run`].
    pub triage: Option<Triage>,
    /// Options used.
    pub options: Options,
}

impl Report {
    /// Attach the triage summary (score, verdict, next steps). `file_name`
    /// appears in the suggested commands.
    #[must_use]
    pub fn with_triage(mut self, file_name: &str) -> Self {
        self.triage = Some(triage::triage(&self, file_name));
        self
    }

    /// Whether any block is damaged or any finding is above `Info`.
    pub fn has_damage(&self) -> bool {
        self.counters.damaged_total() > 0
            || self.findings.iter().any(|f| f.severity > Severity::Info)
    }
}

/// A foreign tail: bytes after the last Oracle-shaped block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailExcerpt {
    /// First foreign block.
    pub start_block: u64,
    /// Foreign blocks to end of file.
    pub blocks: u64,
    /// Hex of the first 64 bytes of the first foreign block.
    pub hex: String,
    /// Same bytes, printable ASCII with `.` for the rest.
    pub ascii: String,
}

/// A clean `0x06` block's identity, kept for the post-pass.
#[derive(Debug, Clone, Copy)]
struct Witness {
    index: u64,
    objd: u32,
    rdba: u32,
    scn: u64,
    kind: BlockKind,
    live_rows: Option<u32>,
}

/// Render the first 64 bytes as hex and printable ASCII.
fn excerpt(bytes: &[u8]) -> (String, String) {
    use std::fmt::Write as _;
    let head = bytes.get(..64.min(bytes.len())).unwrap_or(&[]);
    let mut hex = String::with_capacity(head.len().saturating_mul(3));
    let mut ascii = String::with_capacity(head.len());
    for &b in head {
        let _ = write!(hex, "{b:02x} ");
        ascii.push(if (0x20..0x7f).contains(&b) {
            char::from(b)
        } else {
            '.'
        });
    }
    (hex.trim_end().to_owned(), ascii)
}

/// Name of a [`StructuralFault`] rule for grouping.
fn fault_name(f: &StructuralFault) -> &'static str {
    match f {
        StructuralFault::Misplaced { .. } => "misplaced",
        StructuralFault::LockOutsideItl { .. } => "lock_outside_itl (kdbchk 6101)",
        StructuralFault::ItlLockCount { .. } => "itl_lock_count (kdbchk 6108)",
        StructuralFault::FreeChain { .. } => "free_chain (kdbchk 6106)",
        StructuralFault::UndecodableLiveRow { .. } => "undecodable_live_row",
        StructuralFault::UnusedSlotOffChain { .. } => "unused_slot_off_chain",
        StructuralFault::RowOverlap { .. } => "row_overlap",
        StructuralFault::RowOutsideHeap { .. } => "row_outside_heap",
        StructuralFault::SymbolRefCount { .. } => "symbol_refcount (LV-071)",
        StructuralFault::AvailableSpace { .. } => "available_space (kdbchk 6110)",
        #[allow(unreachable_patterns)] // `#[non_exhaustive]` in the same crate: future variants
        _ => "other",
    }
}

/// Incremental state while streaming.
struct State {
    counters: Counters,
    regions: Vec<Region>,
    damaged_blocks: Vec<DamagedBlock>,
    truncated_list: bool,
    objects: BTreeMap<u32, ObjectSummary>,
    unattributed: u64,
    logical_by_rule: BTreeMap<String, u64>,
    witnesses: Vec<Witness>,
    witnesses_overflow: bool,
    /// Last block that was damaged as `Garbage`/`Encrypted` (a candidate
    /// footer start) and its excerpt; cleared when a clean block follows.
    tail: Option<TailExcerpt>,
    opts: Options,
}

impl State {
    fn note_tail(&mut self, index: u64, v: &Verdict, blk: &[u8]) {
        match v.damage {
            Damage::Garbage { .. } | Damage::Encrypted { .. } | Damage::Truncated { .. } => {
                if let Some(t) = &mut self.tail {
                    t.blocks = t.blocks.saturating_add(1);
                } else {
                    let (hex, ascii) = excerpt(blk);
                    self.tail = Some(TailExcerpt {
                        start_block: index,
                        blocks: 1,
                        hex,
                        ascii,
                    });
                }
            }
            _ => self.tail = None,
        }
    }

    fn push(&mut self, index: u64, v: &Verdict) {
        if index == 0 {
            return;
        }
        self.counters.examined = self.counters.examined.saturating_add(1);
        if v.damage.is_damaged() {
            let code = v.damage.code();
            let e = self.counters.damaged.entry(code.to_owned()).or_insert(0);
            *e = e.saturating_add(1);
            match self.regions.last_mut() {
                Some(r) if r.code == code && r.end() == index => r.len = r.len.saturating_add(1),
                _ => self.regions.push(Region {
                    start: index,
                    len: 1,
                    code: code.to_owned(),
                }),
            }
            if self.damaged_blocks.len() < self.opts.max_block_records {
                self.damaged_blocks.push(DamagedBlock {
                    index,
                    kind: v.kind,
                    damage: v.damage,
                    type_byte: v.type_byte,
                });
            } else {
                self.truncated_list = true;
            }
            if let Damage::Logical { fault } = v.damage {
                let e = self
                    .logical_by_rule
                    .entry(fault_name(&fault).to_owned())
                    .or_insert(0);
                *e = e.saturating_add(1);
                // A logical fault keeps a trustworthy header: attribute it.
                if let Some(o) = v.objd {
                    let e = self.objects.entry(o).or_insert_with(|| ObjectSummary {
                        first_block: index,
                        last_block: index,
                        ..ObjectSummary::default()
                    });
                    e.logical_faults = e.logical_faults.saturating_add(1);
                    e.first_block = e.first_block.min(index);
                    e.last_block = e.last_block.max(index);
                }
            } else if v.type_byte == Some(0x06) {
                self.unattributed = self.unattributed.saturating_add(1);
            }
            return;
        }
        if let Some(s) = v.scn {
            self.counters.highest_scn = Some(self.counters.highest_scn.map_or(s, |m| m.max(s)));
        }
        if v.checksum_present
            && !matches!(v.kind.dbv_class(), DbvClass::Empty | DbvClass::NotCounted)
        {
            self.counters.checksum_flagged = self.counters.checksum_flagged.saturating_add(1);
        }
        if let (Some(objd), Some(rdba), Some(scn)) = (v.objd, v.rdba, v.scn) {
            if self.witnesses.len() < self.opts.max_witnesses {
                self.witnesses.push(Witness {
                    index,
                    objd,
                    rdba,
                    scn,
                    kind: v.kind,
                    live_rows: v.live_rows,
                });
            } else {
                self.witnesses_overflow = true;
            }
        }
        match v.kind.dbv_class() {
            DbvClass::Data => self.counters.data = self.counters.data.saturating_add(1),
            DbvClass::Index => self.counters.index = self.counters.index.saturating_add(1),
            DbvClass::Lob => self.counters.lob = self.counters.lob.saturating_add(1),
            // LV-078: `dbv` reports "Seg" as a sub-count of "Other" (its
            // class totals exceed "Examined" by exactly the Seg count).
            DbvClass::Seg => {
                self.counters.seg = self.counters.seg.saturating_add(1);
                self.counters.other = self.counters.other.saturating_add(1);
            }
            DbvClass::Other => self.counters.other = self.counters.other.saturating_add(1),
            DbvClass::Empty => self.counters.empty = self.counters.empty.saturating_add(1),
            DbvClass::NotCounted => {}
        }
        if let Some(o) = v.objd {
            let e = self.objects.entry(o).or_insert_with(|| ObjectSummary {
                first_block: index,
                last_block: index,
                ..ObjectSummary::default()
            });
            e.blocks = e.blocks.saturating_add(1);
            match v.kind {
                BlockKind::Data => e.data_blocks = e.data_blocks.saturating_add(1),
                BlockKind::Index => e.index_blocks = e.index_blocks.saturating_add(1),
                BlockKind::Lob => e.lob_blocks = e.lob_blocks.saturating_add(1),
                _ => {}
            }
            if let Some(n) = v.live_rows {
                e.live_rows = Some(e.live_rows.unwrap_or(0).saturating_add(u64::from(n)));
            }
            e.first_block = e.first_block.min(index);
            e.last_block = e.last_block.max(index);
        }
    }
}

/// Diagnose a source under a decided layout.
///
/// # Errors
/// I/O errors from the source.
pub fn run<R: ReadAt + ?Sized>(
    src: &mut R,
    layout: Layout,
    opts: Options,
) -> std::io::Result<Report> {
    let bs = layout.block_size.max(1);
    let order = layout.order;
    let file_len = src.len();
    let chunk = CHUNK_LEN.checked_div(bs).unwrap_or(1).saturating_mul(bs);
    let mut buf = vec![0u8; chunk.max(bs)];
    let mut hasher = Sha256::new();
    let mut st = State {
        counters: Counters::default(),
        regions: Vec::new(),
        damaged_blocks: Vec::new(),
        truncated_list: false,
        objects: BTreeMap::new(),
        unattributed: 0,
        logical_by_rule: BTreeMap::new(),
        witnesses: Vec::new(),
        witnesses_overflow: false,
        tail: None,
        opts,
    };
    let mut offset: u64 = 0;
    let mut index: u64 = 0;
    let mut last_partial = false;
    loop {
        let got = src.read_at(offset, &mut buf)?;
        if got == 0 {
            break;
        }
        let Some(data) = buf.get(..got) else { break };
        hasher.update(data);
        let mut pos = 0usize;
        while pos < got {
            let end = pos.saturating_add(bs).min(got);
            let Some(blk) = data.get(pos..end) else { break };
            if blk.len() < bs {
                last_partial = true;
            }
            let v = verdict(blk, bs, order, index, opts.depth);
            st.push(index, &v);
            if index > 0 {
                st.note_tail(index, &v, blk);
            }
            index = index.saturating_add(1);
            pos = end;
        }
        offset = offset.saturating_add(u64::try_from(got).unwrap_or(u64::MAX));
        if got < buf.len() {
            break;
        }
    }
    let sha = hasher.finalize();
    let sha_hex = sha.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    });
    let consistency_pass = if st.witnesses_overflow {
        ConsistencyPass::Skipped
    } else {
        consistency_post_pass(&mut st);
        ConsistencyPass::Complete
    };
    // A foreign tail only counts when it reaches the end of the file *and*
    // is not merely the truncated last block.
    let tail_excerpt = st.tail.filter(|t| t.blocks > 1 || !last_partial);
    let mut report = Report {
        layout,
        file_len,
        blocks_present: index,
        last_block_partial: last_partial,
        sha256_scanned: sha_hex,
        counters: st.counters,
        regions: st.regions,
        damaged_blocks: st.damaged_blocks,
        damaged_blocks_truncated: st.truncated_list,
        objects: st.objects,
        unattributed_damaged: st.unattributed,
        logical_by_rule: st.logical_by_rule,
        consistency_pass,
        tail_excerpt,
        findings: Vec::new(),
        triage: None,
        options: opts,
    };
    report.findings = pattern::findings(&report);
    Ok(report)
}

/// Median of a non-empty sorted slice (lower median for even lengths).
fn median_sorted(v: &[u64]) -> Option<u64> {
    v.get(v.len().checked_sub(1)?.checked_div(2)?).copied()
}

/// Second pass over the clean-block witnesses: a block whose SCN sits far
/// below its object's median (lost write / mixed restore) becomes
/// [`Damage::StaleScn`]; a block whose `rdba` was already seen earlier in
/// the file becomes [`Damage::DuplicateRdba`]. Both are *reclassified*:
/// moved from the clean class to the damaged counters and regions.
#[allow(clippy::too_many_lines)] // three phases: duplicates, stale SCN, apply
fn consistency_post_pass(st: &mut State) {
    let mut demote: Vec<(u64, Damage)> = Vec::new();

    // --- duplicate rdba: sort by (rdba, index); any run > 1 is a duplicate.
    let mut by_rdba: Vec<(u32, u64)> = st.witnesses.iter().map(|w| (w.rdba, w.index)).collect();
    by_rdba.sort_unstable();
    let mut dup_indices: Vec<u64> = Vec::new();
    for pair in by_rdba.windows(2) {
        if let [a, b] = pair
            && a.0 == b.0
        {
            dup_indices.push(b.1);
            demote.push((b.1, Damage::DuplicateRdba { first_at: a.1 }));
        }
    }
    dup_indices.sort_unstable();

    // --- stale SCN per object.
    let mut by_obj: Vec<&Witness> = st
        .witnesses
        .iter()
        .filter(|w| dup_indices.binary_search(&w.index).is_err())
        .collect();
    by_obj.sort_unstable_by_key(|w| (w.objd, w.scn));
    let mut i = 0usize;
    while i < by_obj.len() {
        let Some(first) = by_obj.get(i) else { break };
        let objd = first.objd;
        let mut j = i;
        while by_obj.get(j).is_some_and(|w| w.objd == objd) {
            j = j.saturating_add(1);
        }
        let group = by_obj.get(i..j).unwrap_or(&[]);
        if group.len() >= STALE_SCN_MIN_BLOCKS {
            let scns: Vec<u64> = group.iter().map(|w| w.scn).collect();
            if let Some(med) = median_sorted(&scns) {
                let mut devs: Vec<u64> = scns.iter().map(|&s| s.abs_diff(med)).collect();
                devs.sort_unstable();
                let mad = median_sorted(&devs).unwrap_or(0);
                let gap = STALE_SCN_MIN_GAP.max(mad.saturating_mul(STALE_SCN_MAD_MULT));
                for w in group {
                    if w.scn < med && med.saturating_sub(w.scn) > gap {
                        demote.push((
                            w.index,
                            Damage::StaleScn {
                                scn: w.scn,
                                median: med,
                            },
                        ));
                    }
                }
            }
        }
        i = j;
    }
    if demote.is_empty() {
        return;
    }

    // --- apply: fix counters, objects, regions, list.
    demote.sort_unstable_by_key(|d| d.0);
    let by_index: BTreeMap<u64, Witness> = st.witnesses.iter().map(|w| (w.index, *w)).collect();
    for (index, damage) in &demote {
        // The witness was a clean `0x06` block attributed to its object:
        // move it out of the clean class it was counted in.
        let Some(w) = by_index.get(index) else {
            continue;
        };
        if let Some(o) = st.objects.get_mut(&w.objd) {
            o.blocks = o.blocks.saturating_sub(1);
            o.logical_faults = o.logical_faults.saturating_add(1);
            match w.kind {
                BlockKind::Data => o.data_blocks = o.data_blocks.saturating_sub(1),
                BlockKind::Index => o.index_blocks = o.index_blocks.saturating_sub(1),
                BlockKind::Lob => o.lob_blocks = o.lob_blocks.saturating_sub(1),
                _ => {}
            }
            if let (Some(n), Some(total)) = (w.live_rows, o.live_rows) {
                o.live_rows = Some(total.saturating_sub(u64::from(n)));
            }
        }
        match w.kind.dbv_class() {
            DbvClass::Data => st.counters.data = st.counters.data.saturating_sub(1),
            DbvClass::Index => st.counters.index = st.counters.index.saturating_sub(1),
            DbvClass::Lob => st.counters.lob = st.counters.lob.saturating_sub(1),
            DbvClass::Seg => {
                st.counters.seg = st.counters.seg.saturating_sub(1);
                st.counters.other = st.counters.other.saturating_sub(1);
            }
            DbvClass::Other => st.counters.other = st.counters.other.saturating_sub(1),
            DbvClass::Empty | DbvClass::NotCounted => {}
        }
        let code = damage.code();
        let e = st.counters.damaged.entry(code.to_owned()).or_insert(0);
        *e = e.saturating_add(1);
        st.regions.push(Region {
            start: *index,
            len: 1,
            code: code.to_owned(),
        });
        if st.damaged_blocks.len() < st.opts.max_block_records {
            st.damaged_blocks.push(DamagedBlock {
                index: *index,
                kind: w.kind,
                damage: *damage,
                type_byte: Some(0x06),
            });
        } else {
            st.truncated_list = true;
        }
    }
    // Regions and the list must stay in file order; merge adjacent runs.
    st.regions.sort_by_key(|r| r.start);
    let mut merged: Vec<Region> = Vec::with_capacity(st.regions.len());
    for r in st.regions.drain(..) {
        match merged.last_mut() {
            Some(m) if m.code == r.code && m.end() == r.start => {
                m.len = m.len.saturating_add(r.len);
            }
            _ => merged.push(r),
        }
    }
    st.regions = merged;
    st.damaged_blocks.sort_by_key(|b| b.index);
}

/// Convenience: decide the layout then run. `Ok(None)` when the layout
/// cannot be decided and nothing was forced.
///
/// # Errors
/// I/O errors from the source.
pub fn diagnose<R: ReadAt + ?Sized>(
    src: &mut R,
    forced: Option<(usize, crate::bytes::ByteOrder)>,
    opts: Options,
) -> std::io::Result<Option<Report>> {
    let Some(layout) = layout::decide(src, forced)? else {
        return Ok(None);
    };
    run(src, layout, opts).map(Some)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            counters: Counters::default(),
            regions: Vec::new(),
            damaged_blocks: Vec::new(),
            truncated_list: false,
            objects: BTreeMap::new(),
            unattributed: 0,
            logical_by_rule: BTreeMap::new(),
            witnesses: Vec::new(),
            witnesses_overflow: false,
            tail: None,
            opts: Options::default(),
        }
    }

    fn witness(st: &mut State, index: u64, objd: u32, rdba: u32, scn: u64) {
        st.witnesses.push(Witness {
            index,
            objd,
            rdba,
            scn,
            kind: BlockKind::Data,
            live_rows: Some(3),
        });
        st.counters.data = st.counters.data.saturating_add(1);
        let o = st.objects.entry(objd).or_default();
        o.blocks = o.blocks.saturating_add(1);
        o.data_blocks = o.data_blocks.saturating_add(1);
        o.live_rows = Some(o.live_rows.unwrap_or(0).saturating_add(3));
    }

    #[test]
    fn median_of_sorted() {
        assert_eq!(median_sorted(&[]), None);
        assert_eq!(median_sorted(&[5]), Some(5));
        assert_eq!(median_sorted(&[1, 9]), Some(1));
        assert_eq!(median_sorted(&[1, 5, 9]), Some(5));
        assert_eq!(median_sorted(&[1, 2, 3, 4]), Some(2));
    }

    #[test]
    fn post_pass_demotes_duplicate_rdba_and_stale_scn() {
        let mut st = state();
        // Object 7: twelve blocks at SCN 10_000_000 ± 10, one at 100.
        for i in 0..12u64 {
            let scn = if i == 5 { 100 } else { 10_000_000 + i };
            witness(&mut st, 10 + i, 7, 0x0040_0000 | (10 + i as u32), scn);
        }
        // A second position carrying the rdba of block 11 (bigfile-ambiguous
        // case, which placement alone cannot see).
        witness(&mut st, 4_200_000, 7, 0x0040_000B, 10_000_000);
        // Object 8: too few blocks for a median — never judged.
        for i in 0..4u64 {
            witness(
                &mut st,
                100 + i,
                8,
                0x0040_0000 | (100 + i as u32),
                if i == 0 { 1 } else { 99_000_000 },
            );
        }
        consistency_post_pass(&mut st);

        assert_eq!(st.counters.damaged.get("duplicate_rdba"), Some(&1));
        assert_eq!(st.counters.damaged.get("stale_scn"), Some(&1));
        assert_eq!(st.counters.data, 17 - 2);
        let o7 = &st.objects[&7];
        assert_eq!(o7.blocks, 11);
        assert_eq!(o7.logical_faults, 2);
        assert_eq!(o7.live_rows, Some(11 * 3));
        assert_eq!(st.objects[&8].blocks, 4);
        // File order, merged regions.
        let starts: Vec<u64> = st.regions.iter().map(|r| r.start).collect();
        assert_eq!(starts, vec![15, 4_200_000]);
        assert!(
            st.damaged_blocks
                .windows(2)
                .all(|w| w[0].index < w[1].index)
        );
        assert!(matches!(
            st.damaged_blocks[0].damage,
            Damage::StaleScn { scn: 100, median } if median >= 10_000_000
        ));
        assert_eq!(
            st.damaged_blocks[1].damage,
            Damage::DuplicateRdba { first_at: 11 }
        );
    }

    #[test]
    fn post_pass_is_a_no_op_without_outliers() {
        let mut st = state();
        for i in 0..20u64 {
            witness(&mut st, i + 2, 7, 0x0040_0002 + i as u32, 5_000_000 + i * 3);
        }
        consistency_post_pass(&mut st);
        assert!(st.counters.damaged.is_empty());
        assert_eq!(st.counters.data, 20);
        assert_eq!(st.regions.len(), 0);
    }

    #[test]
    fn excerpt_renders_hex_and_printable_ascii() {
        let (h, a) = excerpt(b"AB\x00\xff");
        assert_eq!(h, "41 42 00 ff");
        assert_eq!(a, "AB..");
        let (h, a) = excerpt(&[0x41u8; 100]);
        assert_eq!(h.len(), 64 * 3 - 1);
        assert_eq!(a.len(), 64);
        assert_eq!(excerpt(&[]), (String::new(), String::new()));
    }
}
