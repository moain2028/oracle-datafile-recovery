//! Error types. Pure decoding never performs I/O, so [`DecodeError`] is small,
//! `Clone` and comparable (useful for tests and for recording decisions in the
//! evidence ledger). [`OdrError`] is the top-level error including I/O.

use serde::Serialize;
use thiserror::Error;

/// An error produced while decoding untrusted bytes. Never fatal for a scan:
/// callers record it and continue with the next block (REQ-001.4).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[non_exhaustive]
pub enum DecodeError {
    /// Not enough bytes for the requested read.
    #[error("truncated: need {need} byte(s) at offset {offset}, buffer has {len}")]
    Truncated {
        /// Offset at which the read was attempted.
        offset: usize,
        /// Number of bytes required.
        need: usize,
        /// Total buffer length.
        len: usize,
    },
    /// Bytes are present but violate the format.
    #[error("malformed {what} at offset {offset}: {reason}")]
    Malformed {
        /// Structure being decoded (e.g. `"NUMBER"`, `"kcbh"`).
        what: &'static str,
        /// Offset of the offending byte, relative to the decoded buffer.
        offset: usize,
        /// Human-readable reason, stable for tests.
        reason: &'static str,
    },
    /// Valid but not yet supported (e.g. a datatype or block format).
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
}

impl DecodeError {
    /// Shorthand constructor for [`DecodeError::Malformed`].
    pub const fn malformed(what: &'static str, offset: usize, reason: &'static str) -> Self {
        Self::Malformed {
            what,
            offset,
            reason,
        }
    }
}

/// Top-level error type.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OdrError {
    /// Decoding failure.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// I/O failure on a source or output.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for decoding.
pub type DecodeResult<T> = Result<T, DecodeError>;
