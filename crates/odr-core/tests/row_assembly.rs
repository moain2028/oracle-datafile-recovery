//! Row assembly + typed values (Lead L-4).
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use odr_core::charset::Charset;
use odr_core::error::DecodeResult;
use odr_core::row::{AssembleError, PieceSource, RowPiece, assemble, flag};
use odr_core::value::{Charsets, ColumnType, Value, decode_value};
use proptest::prelude::*;
use std::collections::HashMap;

/// (flag, columns, next) of one stored piece.
type Entry = (u8, Vec<Option<Vec<u8>>>, Option<(u32, u16)>);

/// In-memory piece store: (rdba, slot) → (flag, columns, next).
#[derive(Default)]
struct Store(HashMap<(u32, u16), Entry>);

impl Store {
    fn put(
        &mut self,
        at: (u32, u16),
        flag: u8,
        cols: Vec<Option<&[u8]>>,
        next: Option<(u32, u16)>,
    ) {
        self.0.insert(
            at,
            (
                flag,
                cols.into_iter().map(|c| c.map(<[u8]>::to_vec)).collect(),
                next,
            ),
        );
    }
}

fn view(slot: u16, e: &Entry) -> RowPiece<'_> {
    RowPiece {
        slot,
        flag: e.0,
        lock: 0,
        columns: e.1.iter().map(|c| c.as_deref()).collect(),
        next: e.2,
        head: None,
        dscn: None,
        cluster: None,
        len: 0,
    }
}

impl PieceSource for Store {
    fn piece(&self, rdba: u32, slot: u16) -> Option<DecodeResult<RowPiece<'_>>> {
        self.0.get(&(rdba, slot)).map(|e| Ok(view(slot, e)))
    }
}

fn head_of(s: &Store, at: (u32, u16)) -> RowPiece<'_> {
    view(at.1, &s.0[&at])
}

const H: u8 = flag::H;
const F: u8 = flag::F;
const L: u8 = flag::L;

#[test]
fn single_piece_row() {
    let mut s = Store::default();
    s.put((1, 0), H | F | L, vec![Some(b"A"), None, Some(b"C")], None);
    let r = assemble(&s, 1, &head_of(&s, (1, 0))).unwrap();
    assert_eq!(
        r.columns,
        vec![Some(b"A".to_vec()), None, Some(b"C".to_vec())]
    );
    assert_eq!(r.pieces, vec![(1, 0)]);
}

#[test]
fn chained_row_with_split_column() {
    // Column 2 ("HELLOWORLD") is split: "HELLO" ends piece 1 (flag N), "WORLD" starts piece 2 (flag P).
    let mut s = Store::default();
    s.put(
        (10, 3),
        H | F | flag::N,
        vec![Some(b"id"), Some(b"HELLO")],
        Some((11, 0)),
    );
    s.put(
        (11, 0),
        flag::P,
        vec![Some(b"WORLD"), Some(b"x")],
        Some((12, 7)),
    );
    s.put((12, 7), L, vec![None, Some(b"z")], None);
    let r = assemble(&s, 10, &head_of(&s, (10, 3))).unwrap();
    assert_eq!(
        r.columns,
        vec![
            Some(b"id".to_vec()),
            Some(b"HELLOWORLD".to_vec()),
            Some(b"x".to_vec()),
            None,
            Some(b"z".to_vec())
        ]
    );
    assert_eq!(r.pieces, vec![(10, 3), (11, 0), (12, 7)]);
}

#[test]
fn migrated_row() {
    let mut s = Store::default();
    s.put((5, 1), H, vec![], Some((9, 4))); // head stub, no columns
    s.put((9, 4), F | L, vec![Some(b"moved"), Some(b"row")], None);
    let r = assemble(&s, 5, &head_of(&s, (5, 1))).unwrap();
    assert_eq!(
        r.columns,
        vec![Some(b"moved".to_vec()), Some(b"row".to_vec())]
    );
}

#[test]
fn corruption_is_reported_never_hidden() {
    let mut s = Store::default();
    s.put((1, 0), H | F, vec![Some(b"a")], Some((2, 0)));
    // Missing piece.
    assert_eq!(
        assemble(&s, 1, &head_of(&s, (1, 0))),
        Err(AssembleError::MissingPiece { rdba: 2, slot: 0 })
    );
    // Cycle back to the head.
    s.put((2, 0), 0, vec![Some(b"b")], Some((1, 0)));
    assert_eq!(
        assemble(&s, 1, &head_of(&s, (1, 0))),
        Err(AssembleError::Cycle { rdba: 1, slot: 0 })
    );
    // Cycle between two continuation pieces.
    s.put((2, 0), 0, vec![Some(b"b")], Some((3, 0)));
    s.put((3, 0), 0, vec![Some(b"c")], Some((2, 0)));
    assert_eq!(
        assemble(&s, 1, &head_of(&s, (1, 0))),
        Err(AssembleError::Cycle { rdba: 2, slot: 0 })
    );
    // Continuation with H flag.
    s.put((2, 0), H, vec![Some(b"b")], None);
    assert!(matches!(
        assemble(&s, 1, &head_of(&s, (1, 0))),
        Err(AssembleError::BadFlags { .. })
    ));
    // N without P.
    let mut t = Store::default();
    t.put((1, 0), H | F | flag::N, vec![Some(b"a")], Some((2, 0)));
    t.put((2, 0), L, vec![Some(b"b")], None);
    assert!(matches!(
        assemble(&t, 1, &head_of(&t, (1, 0))),
        Err(AssembleError::BadFlags { .. })
    ));
    // Not a head / deleted.
    let mut u = Store::default();
    u.put((1, 0), F | L, vec![Some(b"a")], None);
    assert_eq!(
        assemble(&u, 1, &head_of(&u, (1, 0))),
        Err(AssembleError::NotHead)
    );
    u.put((1, 0), H | F | L | flag::D, vec![Some(b"a")], None);
    assert_eq!(
        assemble(&u, 1, &head_of(&u, (1, 0))),
        Err(AssembleError::Deleted)
    );
}

#[test]
fn very_long_chain_is_bounded() {
    let mut s = Store::default();
    s.put((0, 0), H | F, vec![Some(b"x")], Some((1, 0)));
    for i in 1..5000u32 {
        s.put((i, 0), 0, vec![Some(b"x")], Some((i + 1, 0)));
    }
    assert_eq!(
        assemble(&s, 0, &head_of(&s, (0, 0))),
        Err(AssembleError::TooLong)
    );
}

// ---------- typed values ----------
const CS: Charsets = Charsets {
    database: Charset::Ar8MsWin1256,
    national: Charset::Al16Utf16,
};

#[test]
fn typed_values_and_canonical_text() {
    let v = decode_value(Some(&[0xC2, 0x02, 0x0E, 0x33]), ColumnType::Number, CS);
    assert_eq!(v.canonical().as_deref(), Some("113.5"));
    let v = decode_value(
        Some(&[0xE3, 0xCD, 0xE3, 0xCF]),
        ColumnType::Varchar2 { national: false },
        CS,
    );
    assert_eq!(v.canonical().as_deref(), Some("محمد"));
    assert!(v.is_exact());
    let v = decode_value(
        Some(&[0x06, 0x39, 0x06, 0x31]),
        ColumnType::Varchar2 { national: true },
        CS,
    );
    assert_eq!(v.canonical().as_deref(), Some("عر"));
    assert_eq!(decode_value(None, ColumnType::Number, CS), Value::Null);
    assert_eq!(decode_value(None, ColumnType::Number, CS).canonical(), None);
    let v = decode_value(Some(&[0xDE, 0xAD]), ColumnType::Raw, CS);
    assert_eq!(v.canonical().as_deref(), Some("DEAD"));
    assert_eq!(
        ColumnType::from_oracle(1, 2),
        ColumnType::Varchar2 { national: true }
    );
    // LV-068: LOB locators are decoded since 2026-10-24.
    assert_eq!(
        ColumnType::from_oracle(112, 1),
        ColumnType::Clob { national: false }
    );
    assert_eq!(ColumnType::from_oracle(113, 1), ColumnType::Blob);
    assert_eq!(
        ColumnType::from_oracle(114, 1),
        ColumnType::Unsupported(114)
    );
}

/// Character sets of the database the 26ai LOB locators below were copied
/// from (golden `AL32UTF8`). Decoding them as if the database were
/// `AR8MSWIN1256` is a contradiction LV-074 rejects (see
/// `lob_locator_charset_form_lv074`).
const CS_UTF8: Charsets = Charsets {
    database: Charset::Al32Utf8,
    national: Charset::Al16Utf16,
};

/// LV-074 — the locator says how CLOB data is stored (byte 6 bit `0x80` =
/// UCS-2) and 11.2 writes a different header constant (`00 54 00 01`).
/// Bytes copied from the real blocks of `golden-ar8-20261103` (11g XE,
/// `AR8MSWIN1256`, `T_LOB_BASIC` id 2 col C, block 5/3612 slot 1) and of
/// `golden-11g-20261103` (11g XE, `AL32UTF8`, same row). Before LV-074 both
/// were rejected ("locator does not start with 00 70 00 01").
#[test]
fn lob_locator_charset_form_lv074() {
    use odr_core::types::{LobData, LobLocator};
    let ar8 = hex(
        "00540001020c0000000100000001000000489ed600230900000000000013000000000001ddc7cae6d1c920d1dee32031202d20e3d1ddde",
    );
    let l = LobLocator::parse(&ar8).unwrap();
    assert!(l.character && !l.ucs2, "single-byte CLOB: no UCS-2 flag");
    assert!(matches!(l.data, LobData::Inline(b) if b.len() == 19));
    let v = decode_value(Some(&ar8), ColumnType::Clob { national: false }, CS);
    assert_eq!(v.canonical().as_deref(), Some("فاتورة رقم 1 - مرفق"));
    assert!(v.is_exact());
    // Same row, 11g AL32UTF8: `00 54 00 01` header, UCS-2 data.
    let g11 = hex(
        "00540001020c8000000200000001000000487cda0036090000000000002600000000000106410627062a0648063106290020063106420645002000310020002d00200645063106410642",
    );
    let l = LobLocator::parse(&g11).unwrap();
    assert!(l.character && l.ucs2);
    let v = decode_value(Some(&g11), ColumnType::Clob { national: false }, CS_UTF8);
    assert_eq!(v.canonical().as_deref(), Some("فاتورة رقم 1 - مرفق"));
    // Contradictions are rejected, never decoded with the wrong charset:
    // a UCS-2 locator in a single-byte database, a single-byte locator in
    // an AL32UTF8 database.
    assert!(matches!(
        decode_value(Some(&g11), ColumnType::Clob { national: false }, CS),
        Value::Invalid { .. }
    ));
    assert!(matches!(
        decode_value(Some(&ar8), ColumnType::Clob { national: false }, CS_UTF8),
        Value::Invalid { .. }
    ));
    // Any other header constant is still refused.
    let mut bad = ar8.clone();
    bad[1] = 0x60;
    assert!(LobLocator::parse(&bad).is_err());
}

/// LV-068 — in-row LOB locators, bytes copied from the real 26ai blocks
/// (`T_LOB_BASIC` block 16/3622 and `T_LOB_SECURE` block 16/4447 of golden
/// `20261024T085903Z`). Inline data decodes exactly; the out-of-line
/// locator is reported, never turned into NULL or an empty value; a
/// truncated locator is `Invalid`, not a shorter LOB.
#[test]
fn lob_locators_from_real_blocks() {
    use odr_core::types::{LobData, LobLocator, LobStorage};
    // BasicFile BLOB, inline DEADBEEF00FF (T_LOB_BASIC id 1, col B).
    let basic_blob: Vec<u8> =
        hex("00700001010c0000000100000001000000096b2800160900000000000006000000000001deadbeef00ff");
    let l = LobLocator::parse(&basic_blob).unwrap();
    assert!(!l.character);
    assert_eq!(l.storage, LobStorage::BasicFile);
    assert_eq!(
        l.data,
        LobData::Inline(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0xff])
    );
    assert_eq!(
        decode_value(Some(&basic_blob), ColumnType::Blob, CS_UTF8)
            .canonical()
            .as_deref(),
        Some("DEADBEEF00FF")
    );
    // BasicFile empty CLOB (EMPTY_CLOB(), id 3).
    let basic_empty: Vec<u8> =
        hex("00700001020c8000000200000001000000096b2a00100900000000000000000000000000");
    let l = LobLocator::parse(&basic_empty).unwrap();
    assert!(l.character);
    assert_eq!(l.data, LobData::Inline(&[]));
    assert_eq!(
        decode_value(
            Some(&basic_empty),
            ColumnType::Clob { national: false },
            CS_UTF8
        )
        .canonical()
        .as_deref(),
        Some("")
    );
    // BasicFile out-of-line CLOB (6000 chars, id 6): locator only.
    let basic_out: Vec<u8> = hex(
        "00700001020c8000000200000001000000096b8700180500000000010f1c0000000000020400 0f5f04000f63",
    );
    let l = LobLocator::parse(&basic_out).unwrap();
    assert!(
        matches!(l.data, LobData::OutOfLine { bytes: Some(12_000), ref chunks } if chunks == &[(0x0400_0f5f, 1), (0x0400_0f63, 1)])
    );
    let v = decode_value(
        Some(&basic_out),
        ColumnType::Clob { national: false },
        CS_UTF8,
    );
    assert!(matches!(v, Value::LobOutOfLine { .. }));
    assert!(!v.is_exact());
    assert!(v.canonical().unwrap().starts_with("\\x0070"));
    // SecureFile BLOB inline (id 1, col B).
    let secure_blob: Vec<u8> =
        hex("00700001010c0080000100000001000000096b8a00104890000a0000060 1deadbeef00ff");
    let l = LobLocator::parse(&secure_blob).unwrap();
    assert_eq!(l.storage, LobStorage::SecureFile);
    assert_eq!(
        l.data,
        LobData::Inline(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0xff])
    );
    // SecureFile empty BLOB (id 3).
    let secure_empty: Vec<u8> =
        hex("00700001010c0080000100000001000000096b8d000a489000040000 0000");
    assert_eq!(
        LobLocator::parse(&secure_empty).unwrap().data,
        LobData::Inline(&[])
    );
    // SecureFile out of line (id 6, col B).
    let secure_out: Vec<u8> =
        hex("00700001010c0080000100000001000000096b910012409000 0c2100177001000104001350 01");
    assert!(matches!(
        LobLocator::parse(&secure_out).unwrap().data,
        LobData::OutOfLine { bytes: Some(6000), ref chunks } if chunks == &[(0x0400_1350, 1)]
    ));
    // SecureFile CLOB inline with Arabic (id 2): UCS-2 in row, AL32UTF8 database.
    let secure_arabic: Vec<u8> = hex(
        "00700001020c8080000200000001000000096b8b00304890002a0000260106410627062a0648063106290020063106420645002000310020002d00200645063106410642",
    );
    let v = decode_value(
        Some(&secure_arabic),
        ColumnType::Clob { national: false },
        Charsets {
            database: Charset::Al32Utf8,
            national: Charset::Al16Utf16,
        },
    );
    assert_eq!(v.canonical().as_deref(), Some("فاتورة رقم 1 - مرفق"));
    // Truncated locator: Invalid, never a shorter LOB.
    let cut = &basic_blob[..basic_blob.len() - 1];
    assert!(matches!(
        decode_value(Some(cut), ColumnType::Blob, CS_UTF8),
        Value::Invalid { .. }
    ));
    // Column type disagreeing with the locator (BLOB locator in a CLOB column): Invalid.
    assert!(matches!(
        decode_value(
            Some(&basic_blob),
            ColumnType::Clob { national: false },
            CS_UTF8
        ),
        Value::Invalid { .. }
    ));
}

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn malformed_value_never_looks_valid() {
    // Non-normalised NUMBER (trailing zero digit) must not become "1".
    let v = decode_value(Some(&[0xC1, 0x02, 0x01]), ColumnType::Number, CS);
    assert!(matches!(v, Value::Invalid { .. }));
    assert!(!v.is_exact());
    assert_eq!(v.canonical().as_deref(), Some("\\xC10201"));
    // Feb 30 is not a date.
    let v = decode_value(Some(&[120, 126, 2, 30, 1, 1, 1]), ColumnType::Date, CS);
    assert!(matches!(v, Value::Invalid { .. }));
    // Bad UTF-8 keeps the raw bytes.
    let cs = Charsets {
        database: Charset::Al32Utf8,
        ..CS
    };
    match decode_value(
        Some(&[0x41, 0xFF]),
        ColumnType::Varchar2 { national: false },
        cs,
    ) {
        Value::Text { invalid, raw, .. } => {
            assert_eq!(invalid, 1);
            assert_eq!(raw, Some(vec![0x41, 0xFF]));
        }
        other => unreachable!("unexpected {other:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5_000))]

    /// Random pointer graphs never hang or panic; every outcome is either a row or a typed error.
    #[test]
    fn random_chains_terminate(edges in proptest::collection::vec((0u32..20, any::<u8>(), proptest::option::of(0u32..20)), 1..40)) {
        let mut s = Store::default();
        for (i, (_, fl, nx)) in edges.iter().enumerate() {
            s.put((i as u32, 0), *fl, vec![Some(b"v")], nx.map(|n| (n, 0)));
        }
        if let Some(e) = s.0.get(&(0, 0)) {
            let h = view(0, e);
            let _ = assemble(&s, 0, &h);
        }
    }

    #[test]
    fn decode_value_never_panics(raw in proptest::collection::vec(any::<u8>(), 0..24), code in prop::sample::select(vec![1u16, 2, 12, 23, 69, 96, 100, 101, 180, 181, 182, 183, 231, 112]), form in 1u8..3) {
        let v = decode_value(Some(&raw), ColumnType::from_oracle(code, form), CS);
        let _ = v.canonical();
    }
}
