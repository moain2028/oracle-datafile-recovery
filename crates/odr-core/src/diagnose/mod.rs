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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            depth: Depth::Logical,
            max_block_records: 200_000,
        }
    }
}

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
    opts: Options,
}

impl State {
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
        findings: Vec::new(),
        triage: None,
        options: opts,
    };
    report.findings = pattern::findings(&report);
    Ok(report)
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
