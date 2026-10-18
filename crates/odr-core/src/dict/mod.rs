//! Data-dictionary mode (L-6, REQ-002). **Owner: Lead.**
//!
//! The chain is: `kcvfhrdb` (file 1 block 1, [`kcvfh`]) → `bootstrap$`
//! segment → `bootstrap$` rows (DDL text with hard-coded `OBJNO`, `TABNO`,
//! `CLUSTER` and `EXTENTS (FILE f BLOCK b)`, [`bootstrap`]) → segments of
//! `OBJ$`, `TAB$`, `COL$`, `USER$`, `TS$`… → user tables. Every binary fact
//! used here is in `docs/design/lab-verify.md` (LV-025..028, LV-032..035).
//!
//! Stage status: [`kcvfh`] ✅ (L-6b.1) · [`bootstrap`] ✅ (L-6a) · segment
//! extent maps, `OBJ$`/`TAB$`/`COL$` readers and the resolver — next (see
//! `docs/handover/`).

pub mod bootstrap;
pub mod kcvfh;

pub use kcvfh::FileHeader;
