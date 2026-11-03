//! In-row LOB locators (`CLOB`/`NCLOB` type# 112, `BLOB` type# 113) — M5.
//!
//! A LOB column stores a **locator** in the row. With `ENABLE STORAGE IN
//! ROW` (the default) a LOB of up to ≈ 3,964 bytes is stored *inline*: the
//! data follows the locator inside the same column value. Larger LOBs live
//! in the LOB segment and the column holds only the locator plus chunk
//! addresses.
//!
//! Layout as **measured on 26ai** (lab-verify LV-068; 12 locators of
//! `T_LOB_BASIC` / `T_LOB_SECURE`, every field matched against
//! `DBMS_LOB.GETLENGTH` / `DBMS_LOB.SUBSTR` of the same row):
//!
//! ```text
//! off  size  field                                   measured
//! 0    4     00 70 00 01 (12.2+: 18c/21c/26ai)       version constant
//!             00 54 00 01 (11.2: 11g XE)             (LV-074)
//! 4    1     type: 02 = CLOB, 01 = BLOB              -
//! 5    1     0c                                      constant
//! 6    1     80 = CLOB data in UCS-2 (varying-width  -
//!             DB charset), 00 = BLOB or CLOB in a
//!             single-byte DB charset (LV-074)
//! 7    1     00 = BasicFile, 80 = SecureFile         -
//! 8    2     00 02 (UCS-2 CLOB) / 00 01 (other)      -
//! 10   10    LOB id                                  unique per LOB value
//! 20   2     u16 BE: bytes from offset 20 to the end -
//! 22   …     inode (differs by storage kind):
//!   BasicFile inline:  09 xx xx xx | u32 BE len | 6 bytes | data (len bytes, to the end)
//!   BasicFile out-row: 05 … | chunk DBAs
//!   SecureFile inline: 48 90 | u16 BE rest | lenlen k | len (k+2 bytes BE) | 01 (00 if empty) | data
//!   SecureFile out-row:40 90 | …
//! ```
//!
//! The decoder is **conservative**: inline data is accepted only when the
//! declared length ends exactly at the end of the column value. Anything
//! else is `DecodeError::Malformed` (⇒ `Value::Invalid`, raw kept), and an
//! out-of-line locator is returned as [`LobData::OutOfLine`] — never as
//! NULL, never as an empty string. `CLOB` data in a database whose
//! character set is a varying-width Unicode set (`AL32UTF8`, `UTF8`) is
//! stored as UCS-2 big-endian (`AL16UTF16`); in a fixed single-byte
//! database character set it is stored in that character set. The
//! locator says which: byte 6 bit `0x80` ([`LobLocator::ucs2`]) — measured
//! set on every CLOB of 26ai/21c/18c/11g `AL32UTF8` and clear on every CLOB
//! of an 11g `AR8MSWIN1256` database (LV-074).

use crate::ByteOrder;
use crate::bytes::{read_u8, read_u16, read_u32};
use crate::error::{DecodeError, DecodeResult};

const WHAT: &str = "lob locator";

/// Fixed locator header length.
pub const HEADER_LEN: usize = 20;
/// Data bytes one `BasicFile` chunk block holds with the default 8 KiB block
/// and `CHUNK 8192`: block − 0x38 `kdlb` header − 4 tail (measured, LV-070).
pub const BASIC_CHUNK_PAYLOAD: u64 = 8192 - 0x38 - 4;

/// LOB storage kind (locator byte 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobStorage {
    /// `STORE AS BASICFILE`.
    BasicFile,
    /// `STORE AS SECUREFILE`.
    SecureFile,
}

/// Where the LOB data is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LobData<'a> {
    /// Data stored in the row right after the inode header (may be empty:
    /// `EMPTY_CLOB()` / `EMPTY_BLOB()` is a locator with zero data bytes).
    Inline(&'a [u8]),
    /// Data lives in the LOB segment; only the locator is in the row.
    OutOfLine {
        /// Data length in bytes (CLOB: UCS-2 bytes = 2 × chars). `SecureFile`
        /// stores it directly; `BasicFile` packs it as
        /// `full_chunks << 16 | bytes_in_last_chunk` (LV-070). `None` only if
        /// the inode kind carries no length.
        bytes: Option<u64>,
        /// Chunk map from the locator, in data order: `(rdba, blocks)`.
        /// `BasicFile`: one entry per chunk DBA; `SecureFile`: one entry per
        /// extent `(first rdba, block count)`. Measured on 26ai (LV-070).
        chunks: Vec<(u32, u32)>,
    },
}

/// A decoded LOB locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobLocator<'a> {
    /// `true` for CLOB/NCLOB (type byte `02`), `false` for BLOB (`01`).
    pub character: bool,
    /// Byte 6 bit `0x80`: the character data is stored as UCS-2
    /// (`AL16UTF16`). Set on every CLOB of an `AL32UTF8` database, clear on
    /// BLOBs and on CLOBs of a single-byte database (`AR8MSWIN1256`),
    /// measured on 26ai/21c/18c/11g (LV-068, LV-074).
    pub ucs2: bool,
    /// Storage kind.
    pub storage: LobStorage,
    /// 10-byte LOB id.
    pub lobid: [u8; 10],
    /// Data.
    pub data: LobData<'a>,
}

impl<'a> LobLocator<'a> {
    /// Parse a stored LOB column value. Multi-byte fields of the locator
    /// are big-endian on the little-endian 26ai file (measured), so the
    /// file byte order is not consulted.
    ///
    /// # Errors
    /// `Malformed` when a header constant, the length field or the inline
    /// fit does not match the bytes present.
    pub fn parse(raw: &'a [u8]) -> DecodeResult<Self> {
        let be = ByteOrder::Big;
        if raw.len() < HEADER_LEN.saturating_add(2) {
            return Err(DecodeError::malformed(
                WHAT,
                0,
                "shorter than the 22-byte locator header",
            ));
        }
        // LV-074: 12.2+ (18c/21c/26ai) write `00 70 00 01`; 11.2 (11g XE,
        // AL32UTF8 and AR8MSWIN1256 alike) writes `00 54 00 01`. Every other
        // field of the 20-byte header was measured identical across them.
        if !matches!(read_u32(raw, 0, be)?, 0x0070_0001 | 0x0054_0001) {
            return Err(DecodeError::malformed(
                WHAT,
                0,
                "locator does not start with 00 70 00 01 or 00 54 00 01",
            ));
        }
        let character = match read_u8(raw, 4)? {
            0x02 => true,
            0x01 => false,
            _ => return Err(DecodeError::malformed(WHAT, 4, "unknown LOB type byte")),
        };
        if read_u8(raw, 5)? != 0x0c {
            return Err(DecodeError::malformed(WHAT, 5, "byte 5 is not 0x0c"));
        }
        let ucs2 = read_u8(raw, 6)? & 0x80 != 0;
        if ucs2 && !character {
            return Err(DecodeError::malformed(WHAT, 6, "UCS-2 flag on a BLOB"));
        }
        let storage = match read_u8(raw, 7)? {
            0x00 => LobStorage::BasicFile,
            0x80 => LobStorage::SecureFile,
            _ => return Err(DecodeError::malformed(WHAT, 7, "unknown storage byte")),
        };
        let mut lobid = [0u8; 10];
        lobid.copy_from_slice(crate::bytes::slice_at(raw, 10, 10)?);
        let rest = usize::from(read_u16(raw, HEADER_LEN, be)?);
        if HEADER_LEN.saturating_add(rest) != raw.len() {
            return Err(DecodeError::malformed(
                WHAT,
                HEADER_LEN,
                "locator length field does not reach the end of the column",
            ));
        }
        let body =
            crate::bytes::slice_at(raw, HEADER_LEN.saturating_add(2), rest.saturating_sub(2))?;
        let data = match storage {
            LobStorage::BasicFile => Self::basic_body(body)?,
            LobStorage::SecureFile => Self::secure_body(body)?,
        };
        Ok(Self {
            character,
            ucs2,
            storage,
            lobid,
            data,
        })
    }

    /// `BasicFile` inode: `09` inline / `05` out of row.
    fn basic_body(body: &'a [u8]) -> DecodeResult<LobData<'a>> {
        match read_u8(body, 0)? {
            0x09 => {
                let len = usize::try_from(read_u32(body, 4, ByteOrder::Big)?)
                    .map_err(|_| DecodeError::malformed(WHAT, 26, "inline length"))?;
                let data = crate::bytes::slice_at(body, 14, len)?;
                if 14usize.saturating_add(len) != body.len() {
                    return Err(DecodeError::malformed(
                        WHAT,
                        26,
                        "BasicFile inline length does not fill the column",
                    ));
                }
                Ok(LobData::Inline(data))
            }
            0x05 => {
                // Measured (26ai, LV-070), BasicFile out-of-row inode:
                //   CLOB 6000 chars (12,000 UCS-2 bytes), 2 chunks of 1 block:
                //     `05 00 00 00 | 00 01 0f 1c | 00 00 00 00 | 00 02 | 04 00 0f 5f | 04 00 0f 63`
                //   BLOB 6,000 bytes, 1 chunk:
                //     `05 00 00 00 | 00 00 17 70 | 00 00 00 00 | 00 04 | 04 00 10 67`
                //   [4..8]  u32 BE = full_chunks << 16 | bytes_in_last_chunk
                //           (1·8132 + 0x0f1c = 12,000 ✓; 0 + 0x1770 = 6,000 ✓;
                //           8132 = 8192 − 0x38 header − 4 tail = payload per block)
                //   [12..14] u16 BE flags (2 = CLOB, 4 = BLOB) — not a count
                //   [14..]  chunk DBAs, u32 BE each, to the end of the locator.
                let packed = read_u32(body, 4, ByteOrder::Big)?;
                let full = u64::from(packed >> 16);
                let last = u64::from(packed & 0xFFFF);
                let rest = body.get(14..).unwrap_or(&[]);
                if rest.len() % 4 != 0 || rest.is_empty() {
                    return Err(DecodeError::malformed(
                        WHAT,
                        36,
                        "BasicFile chunk map is not a whole number of DBAs",
                    ));
                }
                let mut chunks = Vec::with_capacity(rest.len() / 4);
                for i in (0..rest.len()).step_by(4) {
                    chunks.push((read_u32(rest, i, ByteOrder::Big)?, 1u32));
                }
                let nchunks = u64::try_from(chunks.len()).unwrap_or(u64::MAX);
                // The packed length must agree with the chunk map: `full`
                // whole chunks plus one partial (or exactly `full` when the
                // last byte count is 0).
                let expect_chunks = full.saturating_add(u64::from(last != 0));
                if expect_chunks != nchunks {
                    return Err(DecodeError::malformed(
                        WHAT,
                        26,
                        "BasicFile packed length disagrees with the chunk map",
                    ));
                }
                Ok(LobData::OutOfLine {
                    bytes: Some(
                        full.saturating_mul(BASIC_CHUNK_PAYLOAD)
                            .saturating_add(last),
                    ),
                    chunks,
                })
            }
            _ => Err(DecodeError::malformed(
                WHAT,
                22,
                "unknown BasicFile inode flag",
            )),
        }
    }

    /// `SecureFile` inode: `48 90` inline / `40 90` out of row.
    fn secure_body(body: &'a [u8]) -> DecodeResult<LobData<'a>> {
        let flag = read_u8(body, 0)?;
        if read_u8(body, 1)? != 0x90 {
            return Err(DecodeError::malformed(
                WHAT,
                23,
                "SecureFile inode byte 1 is not 0x90",
            ));
        }
        let rest = usize::from(read_u16(body, 2, ByteOrder::Big)?);
        if 4usize.saturating_add(rest) != body.len() {
            return Err(DecodeError::malformed(
                WHAT,
                24,
                "SecureFile inode length does not reach the end of the column",
            ));
        }
        match flag {
            0x48 => {
                // Variable-width length: indicator k ⇒ k + 2 big-endian bytes.
                let k = usize::from(read_u8(body, 4)?);
                if k > 2 {
                    return Err(DecodeError::malformed(WHAT, 26, "SecureFile length width"));
                }
                let width = k.saturating_add(2);
                let len_bytes = crate::bytes::slice_at(body, 5, width)?;
                let len = len_bytes.iter().fold(0usize, |acc, &b| {
                    acc.saturating_mul(256).saturating_add(usize::from(b))
                });
                let marker_at = 5usize.saturating_add(width);
                let marker = read_u8(body, marker_at)?;
                let expected_marker = u8::from(len != 0);
                if marker != expected_marker {
                    return Err(DecodeError::malformed(
                        WHAT,
                        22,
                        "SecureFile inline marker byte",
                    ));
                }
                let data_at = marker_at.saturating_add(1);
                let data = crate::bytes::slice_at(body, data_at, len)?;
                if data_at.saturating_add(len) != body.len() {
                    return Err(DecodeError::malformed(
                        WHAT,
                        26,
                        "SecureFile inline length does not fill the column",
                    ));
                }
                Ok(LobData::Inline(data))
            }
            0x40 => {
                // Measured (26ai, LV-070), SecureFile out-of-row inode:
                //   `40 90 | 00 0c | 21 | 00 2e e0 | 01 00 01 | 04 00 12 50 | 02`
                //   [4]        flags: low nibble = length width k (0x21 → k=1 → 3 bytes)
                //   [5..5+k+2] byte length BE (0x2ee0 = 12,000 UCS-2 bytes = 6,000 chars)
                //   then `01 00 nn`: nn = extent-map entries
                //   then nn × (u32 BE first rdba, u8 block count)
                let k = usize::from(read_u8(body, 4)? & 0x0f);
                if k > 3 {
                    return Err(DecodeError::malformed(WHAT, 26, "SecureFile length width"));
                }
                let width = k.saturating_add(2);
                let len_bytes = crate::bytes::slice_at(body, 5, width)?;
                let bytes = len_bytes.iter().fold(0u64, |acc, &b| {
                    acc.saturating_mul(256).saturating_add(u64::from(b))
                });
                let mut at = 5usize.saturating_add(width);
                let nmap = usize::from(read_u8(body, at.saturating_add(2))?);
                at = at.saturating_add(3);
                let mut chunks = Vec::with_capacity(nmap);
                for _ in 0..nmap {
                    let dba = read_u32(body, at, ByteOrder::Big)?;
                    let nblk = u32::from(read_u8(body, at.saturating_add(4))?);
                    chunks.push((dba, nblk));
                    at = at.saturating_add(5);
                }
                if at != body.len() {
                    return Err(DecodeError::malformed(
                        WHAT,
                        22,
                        "SecureFile extent map does not fill the locator",
                    ));
                }
                Ok(LobData::OutOfLine {
                    bytes: Some(bytes),
                    chunks,
                })
            }
            _ => Err(DecodeError::malformed(
                WHAT,
                22,
                "unknown SecureFile inode flag",
            )),
        }
    }
}
