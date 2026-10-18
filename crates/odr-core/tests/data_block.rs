//! Data-block layer tests (Lead). Blocks are assembled here byte-by-byte from
//! the documented layout, independently of the parser, for both layouts and
//! both byte orders. Real-file confirmation: `docs/design/lab-verify.md`.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use odr_core::ByteOrder;
use odr_core::row::{DataBlock, Layout, data_object_id, flag};
use proptest::prelude::*;

const BS: usize = 8192;

fn put16(b: &mut [u8], o: usize, v: u16, ord: ByteOrder) {
    let a = if ord == ByteOrder::Little {
        v.to_le_bytes()
    } else {
        v.to_be_bytes()
    };
    b[o..o + 2].copy_from_slice(&a);
}
fn put32(b: &mut [u8], o: usize, v: u32, ord: ByteOrder) {
    let a = if ord == ByteOrder::Little {
        v.to_le_bytes()
    } else {
        v.to_be_bytes()
    };
    b[o..o + 4].copy_from_slice(&a);
}

/// Encode one column the way Oracle stores it. The `0xFE` long length is in
/// the file byte order (LV-009, confirmed on a real 26ai little-endian file).
fn col(v: Option<&[u8]>, ord: ByteOrder) -> Vec<u8> {
    match v {
        None => vec![0xFF],
        Some(d) if d.len() <= 250 => [&[d.len() as u8][..], d].concat(),
        Some(d) => {
            let n = d.len() as u16;
            let len = match ord {
                ByteOrder::Little => n.to_le_bytes(),
                ByteOrder::Big => n.to_be_bytes(),
            };
            [&[0xFE][..], &len[..], d].concat()
        }
    }
}

struct Piece {
    flag: u8,
    nrid: Option<(u32, u16)>,
    cols: Vec<Option<Vec<u8>>>,
}

fn encode_piece(p: &Piece, ord: ByteOrder) -> Vec<u8> {
    let mut v = vec![p.flag, 0, p.cols.len() as u8];
    if let Some((dba, slot)) = p.nrid {
        // nrid is big-endian regardless of file order (LV-049).
        v.extend(dba.to_be_bytes());
        v.extend(slot.to_be_bytes());
    }
    for c in &p.cols {
        v.extend(col(c.as_deref(), ord));
    }
    v
}

/// Build a table block; `None` pieces become unused directory slots.
fn build(ord: ByteOrder, layout: Layout, objd: u32, itc: u16, pieces: &[Option<Piece>]) -> Vec<u8> {
    let mut b = vec![0u8; BS];
    b[0] = 0x06;
    b[1] = 0xA2;
    b[0x14] = 1;
    put32(&mut b, 0x18, objd, ord);
    b[0x24] = itc as u8; // ktbbhict is one byte (LV-047)
    let kdbh = 0x2C + 24 * usize::from(itc) + if layout == Layout::Assm { 8 } else { 0 };
    let nrow = pieces.len() as u16;
    let fsbo = 14 + 4 + 2 * usize::from(nrow);
    // Rows are stacked downward from the tail check.
    let mut top = BS - 4;
    let mut offs = Vec::new();
    for p in pieces {
        match p {
            Some(p) => {
                let bytes = encode_piece(p, ord);
                top -= bytes.len();
                b[top..top + bytes.len()].copy_from_slice(&bytes);
                offs.push((top - kdbh) as u16);
            }
            None => offs.push(0xFFFF),
        }
    }
    let free_end = (top - kdbh) as u16;
    b[kdbh] = 0;
    b[kdbh + 1] = 1;
    put16(&mut b, kdbh + 2, nrow, ord);
    put16(&mut b, kdbh + 4, 0xFFFF, ord);
    put16(&mut b, kdbh + 6, fsbo as u16, ord);
    put16(&mut b, kdbh + 8, free_end, ord);
    put16(&mut b, kdbh + 10, free_end - fsbo as u16, ord);
    put16(&mut b, kdbh + 12, free_end - fsbo as u16, ord);
    put16(&mut b, kdbh + 14, 0, ord); // kdbt: first slot
    put16(&mut b, kdbh + 16, nrow, ord); // kdbt: rows
    for (i, o) in offs.iter().enumerate() {
        put16(&mut b, kdbh + 18 + 2 * i, *o, ord);
    }
    b
}

const HFL: u8 = flag::H | flag::F | flag::L;

fn sample() -> Vec<Option<Piece>> {
    let long = vec![b'X'; 1000];
    vec![
        Some(Piece {
            flag: HFL,
            nrid: None,
            cols: vec![Some(vec![0xC1, 0x02]), Some(b"Ali".to_vec())],
        }),
        Some(Piece {
            flag: HFL,
            nrid: None,
            cols: vec![Some(vec![0xC1, 0x03]), None, Some(long)],
        }),
        None,
        Some(Piece {
            flag: HFL | flag::D,
            nrid: None,
            cols: vec![Some(vec![0xC1, 0x04])],
        }),
        Some(Piece {
            flag: flag::H | flag::F,
            nrid: Some(((1 << 22) | 0x4D, 3)),
            cols: vec![Some(vec![0xC1, 0x05])],
        }),
        // Arabic "محمد" in AR8MSWIN1256
        Some(Piece {
            flag: HFL,
            nrid: None,
            cols: vec![Some(vec![0xC1, 0x06]), Some(vec![0xE3, 0xCD, 0xE3, 0xCF])],
        }),
    ]
}

#[test]
fn parses_both_layouts_and_byte_orders() {
    for ord in [ByteOrder::Little, ByteOrder::Big] {
        for layout in [Layout::Mssm, Layout::Assm] {
            for itc in [1u16, 2, 3, 24] {
                let b = build(ord, layout, 74_565, itc, &sample());
                assert_eq!(data_object_id(&b, ord), Ok(74_565));
                let db = DataBlock::parse(&b, ord).unwrap();
                assert_eq!(db.layout, layout, "{ord:?} {layout:?} itc={itc}");
                assert_eq!(db.kdbh.nrow, 6);
                let rows: Vec<_> = db.rows().collect();
                assert_eq!(rows.len(), 5, "unused slot 2 is skipped");
                let r0 = rows[0].1.as_ref().unwrap();
                assert!(r0.is_complete());
                assert_eq!(
                    r0.columns,
                    vec![Some(&[0xC1u8, 0x02][..]), Some(&b"Ali"[..])]
                );
                let r1 = rows[1].1.as_ref().unwrap();
                assert_eq!(r1.columns[1], None);
                assert_eq!(r1.columns[2].unwrap().len(), 1000, "0xFE long length");
                assert!(rows[2].1.as_ref().unwrap().is_deleted());
                let r4 = rows[3].1.as_ref().unwrap();
                assert_eq!(r4.next, Some(((1 << 22) | 0x4D, 3)), "chained row nrid");
                assert!(!r4.is_complete());
                assert_eq!(
                    rows[4].1.as_ref().unwrap().columns[1],
                    Some(&[0xE3u8, 0xCD, 0xE3, 0xCF][..])
                );
            }
        }
    }
}

#[test]
fn bad_row_does_not_stop_block() {
    let mut b = build(ByteOrder::Little, Layout::Assm, 1, 2, &sample());
    let db = DataBlock::parse(&b, ByteOrder::Little).unwrap();
    let off = match db.slot(0).unwrap() {
        odr_core::row::Slot::Row(o) => o,
        odr_core::row::Slot::Unused => unreachable!(),
    };
    b[off + 3] = 0xFC; // reserved length byte in column 1 of row 0
    let db = DataBlock::parse(&b, ByteOrder::Little).unwrap();
    let rows: Vec<_> = db.rows().collect();
    assert!(rows[0].1.is_err());
    assert_eq!(rows.iter().filter(|r| r.1.is_ok()).count(), 4);
}

#[test]
fn rejects_inconsistent_headers() {
    let ord = ByteOrder::Little;
    let good = build(ord, Layout::Mssm, 1, 2, &sample());
    let kdbh = 0x2C + 48;
    let mut b = good.clone();
    put16(&mut b, kdbh + 2, 7, ord); // nrow no longer matches fsbo
    assert!(DataBlock::parse(&b, ord).is_err());
    let mut b = good.clone();
    b[0x24] = 0; // zero ITLs
    assert!(DataBlock::parse(&b, ord).is_err());
    // LV-047: the count is one byte; 0x25 must be ignored (26ai writes 0x1F).
    let mut b = good;
    b[0x25] = 0x1F;
    let db = DataBlock::parse(&b, ord).unwrap();
    assert_eq!(db.ktb.itl_count, 2);
}

#[test]
fn empty_formatted_block_has_no_rows() {
    // LV-048: ntab = 0, nrow = 0, fsbo = 14 — a formatted but never-used block.
    let ord = ByteOrder::Little;
    let b = build(ord, Layout::Mssm, 1, 2, &[]);
    let mut b = b;
    let kdbh = 0x2C + 48;
    b[kdbh + 1] = 0;
    put16(&mut b, kdbh + 6, 14, ord); // fsbo = 14 + 4*0 + 2*0
    let db = DataBlock::parse(&b, ord).unwrap();
    assert_eq!(db.tables.len(), 0);
    assert_eq!(db.rows().count(), 0);
    // But ntab = 0 with rows present is still rejected.
    put16(&mut b, kdbh + 2, 1, ord);
    put16(&mut b, kdbh + 6, 16, ord);
    assert!(DataBlock::parse(&b, ord).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5_000))]

    #[test]
    fn never_panics_on_random_blocks(raw in proptest::collection::vec(any::<u8>(), 0..BS + 1), big in any::<bool>()) {
        let ord = if big { ByteOrder::Big } else { ByteOrder::Little };
        if let Ok(db) = DataBlock::parse(&raw, ord) {
            for r in db.rows() { let _ = r; }
        }
    }

    #[test]
    fn never_panics_on_mutated_valid_blocks(pos in 0..BS, val in any::<u8>(), n in 1..8usize) {
        let mut b = build(ByteOrder::Little, Layout::Assm, 9, 2, &sample());
        for k in 0..n { b[(pos + k * 977) % BS] = val.wrapping_add(k as u8); }
        if let Ok(db) = DataBlock::parse(&b, ByteOrder::Little) {
            for r in db.rows() { let _ = r; }
        }
    }
}
