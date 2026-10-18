//! Data-dictionary mode (L-6, REQ-002). **Owner: Lead.**
//!
//! The chain is: `kcvfhrdb` (file 1 block 1, [`kcvfh`]) → `bootstrap$`
//! segment → `bootstrap$` rows (DDL text with hard-coded `OBJNO`, `TABNO`,
//! `CLUSTER` and `EXTENTS (FILE f BLOCK b)`, [`bootstrap`]) → segments of
//! `OBJ$`, `TAB$`, `COL$`, `USER$`, `TS$`… → user tables. Every binary fact
//! used here is in `docs/design/lab-verify.md` (LV-025..028, LV-032..035).
//!
//! Stage status: [`kcvfh`] ✅ · [`segment`] ✅ (extent maps, walker) ·
//! [`bootstrap`] ✅ · `OBJ$`/`TAB$`/`COL$` readers and the resolver — next
//! (see `docs/handover/`).

pub mod bootstrap;
pub mod kcvfh;
pub mod resolver;
pub mod rows;
pub mod schema;
pub mod segment;
pub mod source;

pub use kcvfh::FileHeader;
