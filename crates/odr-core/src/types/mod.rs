//! Oracle internal datatype decoders.
//!
//! Contract for every decoder in this module:
//! * Input is the raw column bytes (length byte already stripped).
//! * Output is either an exact value or a [`DecodeError`](crate::DecodeError) — never an
//!   approximation. Floats are never used for NUMBER.
//! * Decoders are pure, allocation-light and never panic (fuzzed).
//!
//! Internal type codes (OCI, \[O2\]): 1 `VARCHAR2`/`NVARCHAR2`, 2 `NUMBER`, 8 `LONG`,
//! 12 `DATE`, 23 `RAW`, 24 `LONG RAW`, 69 `ROWID`, 96 `CHAR`/`NCHAR`, 100 `BINARY_FLOAT`, 101 `BINARY_DOUBLE`,
//! 112 CLOB/NCLOB, 113 BLOB, 180 TIMESTAMP, 181 TIMESTAMP WITH TIME ZONE,
//! 182 INTERVAL YEAR TO MONTH, 183 INTERVAL DAY TO SECOND,
//! 231 TIMESTAMP WITH LOCAL TIME ZONE.

// Arithmetic in the datatype decoders is on values widened from `u8`/`u32`
// into `i16`/`i64` before subtraction, so it cannot overflow.
#![allow(clippy::arithmetic_side_effects)]

pub mod binary_float;
pub mod char;
pub mod date;
pub mod interval;
pub mod lob;
pub mod number;
pub mod rowid;
pub mod timestamp;

pub use binary_float::{decode_binary_double, decode_binary_float};
pub use date::OraDate;
pub use interval::{IntervalDs, IntervalYm};
pub use lob::{LobData, LobLocator, LobStorage};
pub use number::OraNumber;
pub use rowid::OraRowid;
pub use timestamp::{OraTimestamp, TimeZone};
