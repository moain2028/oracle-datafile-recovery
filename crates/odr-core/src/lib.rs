//! # odr-core
//!
//! Read-only, panic-free parsing core for Oracle datafiles (`.dbf`).
//!
//! Design rules (see `AGENTS.md`, ADR-0002, ADR-0005):
//! * Input bytes are **hostile**: every access is bounds-checked and returns
//!   [`DecodeError`] instead of panicking. `unsafe` is forbidden crate-wide.
//! * A wrong value is worse than a missing value: decoders reject malformed
//!   encodings rather than guessing.
//! * Source files are never written to (see [`source`]).
//!
//! Module ownership (see `docs/agents/README.md`): `bytes`, `error`, `block`,
//! `source`, `row`, `types`, `charset`, `dict`, `value` — Lead; `scan`, `header` — Astra.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod block;
pub mod bytes;
pub mod charset;
pub mod diagnose;
pub mod dict;
pub mod error;
pub mod row;
pub mod scan;
pub mod source;
pub mod types;
pub mod value;

pub use bytes::{ByteOrder, Cursor};
pub use error::{DecodeError, OdrError};
