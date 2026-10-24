//! Typed column values (REQ-003.1, REQ-003.3). **Owner: Lead.**
//!
//! [`decode_value`] turns raw column bytes plus a declared [`ColumnType`] into
//! a [`Value`]. The rule that matters most for accounting data: **a value
//! that does not decode cleanly is never turned into a plausible-looking
//! value.** It becomes [`Value::Invalid`] and keeps its raw bytes and the
//! reason (REQ-003.3). Text that has bytes not valid in its character set
//! keeps its raw bytes next to the decoded text.
//!
//! Canonical text ([`Value::canonical`]) is the exact form the golden corpus
//! uses (ADR-0004). Floats use Rust's shortest round-trip form, which is
//! exact for `BINARY_FLOAT`/`BINARY_DOUBLE`.

use crate::charset::{self, Charset};
use crate::error::DecodeError;
use crate::types::char::raw_to_hex;
use crate::types::{
    IntervalDs, IntervalYm, LobData, LobLocator, OraDate, OraNumber, OraRowid, OraTimestamp,
    decode_binary_double, decode_binary_float,
};
use std::borrow::Cow;

/// Declared column type, from the dictionary (`col$.type#`, `col$.charsetform`)
/// or from the inference engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ColumnType {
    /// `VARCHAR2` (`national = false`) or `NVARCHAR2` (`national = true`).
    Varchar2 {
        /// Uses the national character set.
        national: bool,
    },
    /// `CHAR` or `NCHAR`.
    Char {
        /// Uses the national character set.
        national: bool,
    },
    /// `NUMBER` / `FLOAT` / `INTEGER`.
    Number,
    /// `DATE`.
    Date,
    /// `TIMESTAMP[(p)]`.
    Timestamp,
    /// `TIMESTAMP WITH TIME ZONE`.
    TimestampTz,
    /// `TIMESTAMP WITH LOCAL TIME ZONE`.
    TimestampLtz,
    /// `INTERVAL YEAR TO MONTH`.
    IntervalYm,
    /// `INTERVAL DAY TO SECOND`.
    IntervalDs,
    /// `BINARY_FLOAT`.
    BinaryFloat,
    /// `BINARY_DOUBLE`.
    BinaryDouble,
    /// `RAW`.
    Raw,
    /// `LONG` — text in the database character set, stored in-row like
    /// `VARCHAR2` (at most one per table; the dictionary uses it for
    /// `COL$.DEFAULT$` and `VIEW$.TEXT`).
    Long,
    /// `LONG RAW` — bytes, stored in-row like `RAW`.
    LongRaw,
    /// `ROWID` / `UROWID`.
    Rowid,
    /// `CLOB` (`national = false`) / `NCLOB` (`national = true`), type# 112.
    Clob {
        /// Uses the national character set (`NCLOB`).
        national: bool,
    },
    /// `BLOB`, type# 113.
    Blob,
    /// A type not decoded yet (LOB locators, `XMLType`, objects…); raw bytes are kept.
    Unsupported(u16),
}

impl ColumnType {
    /// Map Oracle's internal type code (`col$.type#`) and character-set form
    /// (`col$.charsetform`: 1 = database, 2 = national).
    pub const fn from_oracle(type_code: u16, charset_form: u8) -> Self {
        let national = charset_form == 2;
        match type_code {
            1 => Self::Varchar2 { national },
            2 => Self::Number,
            8 => Self::Long,
            12 => Self::Date,
            23 => Self::Raw,
            24 => Self::LongRaw,
            69 => Self::Rowid,
            96 => Self::Char { national },
            100 => Self::BinaryFloat,
            101 => Self::BinaryDouble,
            112 => Self::Clob { national },
            113 => Self::Blob,
            180 => Self::Timestamp,
            181 => Self::TimestampTz,
            182 => Self::IntervalYm,
            183 => Self::IntervalDs,
            231 => Self::TimestampLtz,
            other => Self::Unsupported(other),
        }
    }
}

/// Character sets needed to decode text columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charsets {
    /// Database character set (`NLS_CHARACTERSET`).
    pub database: Charset,
    /// National character set (`NLS_NCHAR_CHARACTERSET`, usually `AL16UTF16`).
    pub national: Charset,
}

/// A decoded column value.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// SQL NULL (including trailing columns not stored in the row).
    Null,
    /// `NUMBER`.
    Number(OraNumber),
    /// `DATE`.
    Date(OraDate),
    /// `TIMESTAMP` family.
    Timestamp(OraTimestamp),
    /// `INTERVAL YEAR TO MONTH`.
    IntervalYm(IntervalYm),
    /// `INTERVAL DAY TO SECOND`.
    IntervalDs(IntervalDs),
    /// `BINARY_FLOAT`.
    Float(f32),
    /// `BINARY_DOUBLE`.
    Double(f64),
    /// `ROWID`.
    Rowid(OraRowid),
    /// Text. `raw` is kept (`Some`) whenever `invalid > 0`.
    Text {
        /// UTF-8 text (invalid sequences replaced by U+FFFD).
        text: String,
        /// Number of invalid sequences; `0` = exact.
        invalid: usize,
        /// Original bytes when the conversion was not exact.
        raw: Option<Vec<u8>>,
    },
    /// `RAW` bytes.
    Raw(Vec<u8>),
    /// A LOB whose data is **not in the row** (LV-068): the locator decoded
    /// but the value lives in the LOB segment, which ODR does not read yet.
    /// Never exact, never NULL — the caller must report it as unavailable.
    LobOutOfLine {
        /// 10-byte LOB id from the locator.
        lobid: [u8; 10],
        /// The stored locator bytes.
        raw: Vec<u8>,
    },
    /// Bytes that do not decode as the declared type (REQ-003.3).
    Invalid {
        /// Original bytes.
        raw: Vec<u8>,
        /// Why decoding failed.
        error: DecodeError,
    },
    /// Type not decoded yet; raw bytes kept.
    Unsupported {
        /// Oracle type code.
        type_code: u16,
        /// Original bytes.
        raw: Vec<u8>,
    },
}

impl Value {
    /// Whether this value is exactly what Oracle stored (no loss, no doubt).
    pub fn is_exact(&self) -> bool {
        match self {
            Self::Text { invalid, .. } => *invalid == 0,
            Self::Invalid { .. } | Self::Unsupported { .. } | Self::LobOutOfLine { .. } => false,
            _ => true,
        }
    }

    /// Canonical text form (golden-corpus format); `None` for NULL.
    /// Invalid and unsupported values are rendered as `\x` + hex so they can
    /// never be mistaken for a real value.
    pub fn canonical(&self) -> Option<Cow<'_, str>> {
        Some(match self {
            Self::Null => return None,
            Self::Number(n) => Cow::Owned(n.to_string()),
            Self::Date(d) => Cow::Owned(d.to_string()),
            Self::Timestamp(t) => Cow::Owned(t.to_string()),
            Self::IntervalYm(i) => Cow::Owned(i.to_string()),
            Self::IntervalDs(i) => Cow::Owned(i.to_string()),
            Self::Float(x) => Cow::Owned(float_text(f64::from(*x), x.to_string())),
            Self::Double(x) => Cow::Owned(float_text(*x, x.to_string())),
            Self::Rowid(r) => Cow::Owned(r.to_string()),
            Self::Text { text, .. } => Cow::Borrowed(text.as_str()),
            Self::Raw(b) => Cow::Owned(raw_to_hex(b)),
            Self::Invalid { raw, .. }
            | Self::Unsupported { raw, .. }
            | Self::LobOutOfLine { raw, .. } => Cow::Owned(format!("\\x{}", raw_to_hex(raw))),
        })
    }
}

fn float_text(x: f64, shortest: String) -> String {
    if x.is_nan() {
        "NaN".into()
    } else if x == f64::INFINITY {
        "Infinity".into()
    } else if x == f64::NEG_INFINITY {
        "-Infinity".into()
    } else {
        shortest
    }
}

/// Decode one column. `None` (NULL) always yields [`Value::Null`].
pub fn decode_value(raw: Option<&[u8]>, ty: ColumnType, cs: Charsets) -> Value {
    let Some(raw) = raw else { return Value::Null };
    let invalid = |error: DecodeError| Value::Invalid {
        raw: raw.to_vec(),
        error,
    };
    let text = |set: Charset| {
        let d = charset::decode(raw, set);
        let exact = d.is_exact();
        Value::Text {
            text: d.text.into_owned(),
            invalid: d.invalid,
            raw: (!exact).then(|| raw.to_vec()),
        }
    };
    match ty {
        ColumnType::Varchar2 { national } | ColumnType::Char { national } => {
            text(if national { cs.national } else { cs.database })
        }
        ColumnType::Long => text(cs.database),
        ColumnType::Number => OraNumber::decode(raw).map_or_else(invalid, Value::Number),
        ColumnType::Date => OraDate::decode(raw).map_or_else(invalid, Value::Date),
        ColumnType::Timestamp | ColumnType::TimestampLtz => {
            OraTimestamp::decode(raw).map_or_else(invalid, Value::Timestamp)
        }
        ColumnType::TimestampTz => {
            OraTimestamp::decode_tz(raw).map_or_else(invalid, Value::Timestamp)
        }
        ColumnType::IntervalYm => IntervalYm::decode(raw).map_or_else(invalid, Value::IntervalYm),
        ColumnType::IntervalDs => IntervalDs::decode(raw).map_or_else(invalid, Value::IntervalDs),
        ColumnType::BinaryFloat => decode_binary_float(raw).map_or_else(invalid, Value::Float),
        ColumnType::BinaryDouble => decode_binary_double(raw).map_or_else(invalid, Value::Double),
        ColumnType::Raw | ColumnType::LongRaw => Value::Raw(raw.to_vec()),
        ColumnType::Rowid => OraRowid::decode(raw).map_or_else(invalid, Value::Rowid),
        ColumnType::Clob { .. } | ColumnType::Blob => {
            let national = matches!(ty, ColumnType::Clob { national: true });
            let character = matches!(ty, ColumnType::Clob { .. });
            match LobLocator::parse(raw) {
                Err(e) => invalid(e),
                Ok(l) if l.character != character => invalid(DecodeError::malformed(
                    "lob locator",
                    4,
                    "locator LOB type disagrees with the column type",
                )),
                Ok(LobLocator {
                    data: LobData::OutOfLine { .. },
                    lobid,
                    ..
                }) => Value::LobOutOfLine {
                    lobid,
                    raw: raw.to_vec(),
                },
                Ok(LobLocator {
                    data: LobData::Inline(bytes),
                    ..
                }) => {
                    if character {
                        // LV-068: in-row CLOB data is UCS-2 (AL16UTF16) when
                        // the database character set is a varying-width
                        // Unicode set; otherwise it is in the DB character set.
                        let set = if national {
                            cs.national
                        } else if cs.database.is_varying_width_unicode() {
                            Charset::Al16Utf16
                        } else {
                            cs.database
                        };
                        let d = charset::decode(bytes, set);
                        let exact = d.is_exact();
                        Value::Text {
                            text: d.text.into_owned(),
                            invalid: d.invalid,
                            raw: (!exact).then(|| raw.to_vec()),
                        }
                    } else {
                        Value::Raw(bytes.to_vec())
                    }
                }
            }
        }
        ColumnType::Unsupported(type_code) => Value::Unsupported {
            type_code,
            raw: raw.to_vec(),
        },
    }
}
