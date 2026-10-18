//! Cluster rows and `ROWDEPENDENCIES` rows (Lead). Every row below is copied
//! byte-for-byte from a published Oracle block dump; the dump's `tl:` (total
//! row length) is asserted against our decoded length, which proves the header
//! layout arithmetically (any extra or missing header byte changes `tl`).
//! Sources: `docs/research/03_deep_research_round2_ar.md` §3–§4.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use odr_core::row::{
    ClusterPart, DataBlock, Layout, PieceFormat, cluster_columns, flag, kdbh_flag, piece,
};
use odr_core::types::OraNumber;
use odr_core::{ByteOrder, DecodeError};
use proptest::prelude::*;

const BS: usize = 8192;

fn put16(b: &mut [u8], o: usize, v: u16, ord: ByteOrder) {
    let a = match ord {
        ByteOrder::Little => v.to_le_bytes(),
        ByteOrder::Big => v.to_be_bytes(),
    };
    b[o..o + 2].copy_from_slice(&a);
}
fn put32(b: &mut [u8], o: usize, v: u32, ord: ByteOrder) {
    let a = match ord {
        ByteOrder::Little => v.to_le_bytes(),
        ByteOrder::Big => v.to_be_bytes(),
    };
    b[o..o + 4].copy_from_slice(&a);
}
fn u16b(v: u16, ord: ByteOrder) -> [u8; 2] {
    match ord {
        ByteOrder::Little => v.to_le_bytes(),
        ByteOrder::Big => v.to_be_bytes(),
    }
}
fn u32b(v: u32, ord: ByteOrder) -> [u8; 4] {
    match ord {
        ByteOrder::Little => v.to_le_bytes(),
        ByteOrder::Big => v.to_be_bytes(),
    }
}
fn col(d: &[u8]) -> Vec<u8> {
    [&[d.len() as u8][..], d].concat()
}

/// Cluster-key row: flag lock cc curc comc prev(6) next(6) key columns.
fn key_row(curc: u16, comc: u16, rid: (u32, u16), keys: &[&[u8]], ord: ByteOrder) -> Vec<u8> {
    let mut v = vec![0xAC, 0x00, keys.len() as u8];
    v.extend(u16b(curc, ord));
    v.extend(u16b(comc, ord));
    for _ in 0..2 {
        // prev/next rids are big-endian regardless of file order (LV-049).
        v.extend(rid.0.to_be_bytes());
        v.extend(rid.1.to_be_bytes());
    }
    for k in keys {
        v.extend(col(k));
    }
    v
}
/// Cluster member row: flag lock cc cki columns.
fn member_row(fl: u8, cki: u8, cols: &[&[u8]]) -> Vec<u8> {
    let mut v = vec![fl, 0x02, cols.len() as u8, cki];
    for c in cols {
        v.extend(col(c));
    }
    v
}

/// Build a data block with `tables[t]` = raw row pieces of table `t`.
fn build(ord: ByteOrder, layout: Layout, kflag: u8, tables: &[Vec<Vec<u8>>]) -> Vec<u8> {
    let itc = 2u16;
    let mut b = vec![0u8; BS];
    b[0] = 0x06;
    b[1] = 0xA2;
    b[0x14] = 1;
    put32(&mut b, 0x18, 0x0001_611F, ord);
    b[0x24] = itc as u8; // ktbbhict is one byte (LV-047)
    let kdbh = 0x2C + 24 * usize::from(itc) + if layout == Layout::Assm { 8 } else { 0 };
    let ntab = tables.len();
    let nrow: usize = tables.iter().map(Vec::len).sum();
    let fsbo = 14 + 4 * ntab + 2 * nrow;
    let mut top = BS - 4;
    let mut offs = Vec::new();
    for rows in tables {
        for r in rows {
            top -= r.len();
            b[top..top + r.len()].copy_from_slice(r);
            offs.push((top - kdbh) as u16);
        }
    }
    let free_end = (top - kdbh) as u16;
    b[kdbh] = kflag;
    b[kdbh + 1] = ntab as u8;
    put16(&mut b, kdbh + 2, nrow as u16, ord);
    put16(&mut b, kdbh + 4, 0xFFFF, ord);
    put16(&mut b, kdbh + 6, fsbo as u16, ord);
    put16(&mut b, kdbh + 8, free_end, ord);
    put16(&mut b, kdbh + 10, free_end - fsbo as u16, ord);
    put16(&mut b, kdbh + 12, free_end - fsbo as u16, ord);
    let mut first = 0u16;
    for (t, rows) in tables.iter().enumerate() {
        put16(&mut b, kdbh + 14 + 4 * t, first, ord);
        put16(&mut b, kdbh + 16 + 4 * t, rows.len() as u16, ord);
        first += rows.len() as u16;
    }
    for (i, o) in offs.iter().enumerate() {
        put16(&mut b, kdbh + 14 + 4 * ntab + 2 * i, *o, ord);
    }
    b
}

const SMITH: [&[u8]; 6] = [
    &[0xC2, 0x4A, 0x46],
    b"SMITH",
    b"CLERK",
    &[0xC2, 0x50, 0x03],
    &[0x77, 0xB4, 0x0C, 0x11, 0x01, 0x01, 0x01],
    &[0xC2, 0x09],
];

/// The deptx/empx block (file 4 block 555) from the zhizuobiao study:
/// key 20 (`c1 15`), deptx RESEARCH/DALLAS, empx SMITH + JONES (deleted: `0x7C`, cc 0).
fn deptx_block(ord: ByteOrder, layout: Layout) -> Vec<u8> {
    let rid = (0x0100_022B, 0);
    build(
        ord,
        layout,
        kdbh_flag::CLUSTER,
        &[
            vec![key_row(6, 5, rid, &[&[0xC1, 0x15]], ord)],
            vec![member_row(0x6C, 0, &[b"RESEARCH", b"DALLAS"])],
            vec![member_row(0x6C, 0, &SMITH), member_row(0x7C, 0, &[])],
        ],
    )
}

#[test]
fn published_cluster_rows_have_published_lengths() {
    for ord in [ByteOrder::Little, ByteOrder::Big] {
        for layout in [Layout::Mssm, Layout::Assm] {
            let b = deptx_block(ord, layout);
            let db = DataBlock::parse(&b, ord).unwrap();
            assert_eq!(db.layout, layout);
            assert_eq!(db.tables.len(), 3);

            let key = db.row(0).unwrap().unwrap();
            assert_eq!(key.len, 22, "dump: tl: 22 fb: K-H-FL--");
            assert!(key.is_cluster_key() && key.is_complete());
            assert_eq!(
                key.cluster,
                Some(ClusterPart::Key {
                    current: 6,
                    committed: 5,
                    prev: (0x0100_022B, 0),
                    next: (0x0100_022B, 0),
                })
            );
            assert_eq!(
                OraNumber::decode(key.columns[0].unwrap())
                    .unwrap()
                    .to_string(),
                "20"
            );

            let dept = db.row(1).unwrap().unwrap();
            assert_eq!(dept.len, 20, "dump: tl: 20 fb: -CH-FL-- cc: 2 cki: 0");
            assert_eq!(dept.cluster, Some(ClusterPart::Member { key_index: 0 }));
            assert_eq!(
                dept.columns,
                vec![Some(&b"RESEARCH"[..]), Some(&b"DALLAS"[..])]
            );

            let emp = db.row(2).unwrap().unwrap();
            assert_eq!(emp.len, 35, "dump: tl: 35 fb: -CH-FL-- cc: 6 cki: 0");
            assert_eq!(emp.columns.len(), 6);

            let gone = db.row(3).unwrap().unwrap();
            assert_eq!(gone.len, 4, "dump: tl: 4 fb: -CHDFL-- cc: 0 cki: 0");
            assert!(gone.is_deleted() && gone.columns.is_empty());

            assert_eq!(
                (0..4).map(|i| db.table_of(i)).collect::<Vec<_>>(),
                vec![Some(0), Some(1), Some(2), Some(2)]
            );
            assert_eq!(db.table_of(4), None);
        }
    }
}

#[test]
fn member_rows_resolve_to_their_key_and_segment_column_order() {
    let ord = ByteOrder::Little;
    let b = deptx_block(ord, Layout::Mssm);
    let db = DataBlock::parse(&b, ord).unwrap();
    let emp = db.row(2).unwrap().unwrap();
    let Some(ClusterPart::Member { key_index }) = emp.cluster else {
        unreachable!()
    };
    let key = db.cluster_key(key_index).unwrap();
    let cols = cluster_columns(&key, &emp, 1).unwrap();
    // Segment order: cluster key (DEPTNO) first, then the member's own columns.
    assert_eq!(cols.len(), 7);
    assert_eq!(cols[0], Some(&[0xC1, 0x15][..]));
    assert_eq!(cols[2], Some(&b"SMITH"[..]));
}

#[test]
fn null_trailing_key_column_is_padded_not_shifted() {
    let ord = ByteOrder::Big;
    let b = build(
        ord,
        Layout::Mssm,
        0,
        &[
            // Two-column cluster key whose second column is NULL (not stored).
            vec![key_row(1, 1, (0x0040_00D1, 0), &[&[0xC1, 0x02]], ord)],
            vec![member_row(0x6C, 0, &[b"A"])],
        ],
    );
    let db = DataBlock::parse(&b, ord).unwrap();
    let key = db.cluster_key(0).unwrap();
    let m = db.row(1).unwrap().unwrap();
    let cols = cluster_columns(&key, &m, 2).unwrap();
    assert_eq!(cols, vec![Some(&[0xC1, 0x02][..]), None, Some(&b"A"[..])]);
    // More stored key columns than CLUCOLS is corruption, not a guess.
    assert!(cluster_columns(&key, &m, 0).is_err());
    // Arguments in the wrong role are rejected.
    assert!(cluster_columns(&m, &key, 2).is_err());
}

#[test]
fn bad_cki_is_rejected() {
    let ord = ByteOrder::Little;
    let b = deptx_block(ord, Layout::Mssm);
    let db = DataBlock::parse(&b, ord).unwrap();
    assert!(db.cluster_key(1).is_err(), "only one key row in kdbt[0]");
}

#[test]
fn undocumented_cluster_combinations_are_unsupported_not_guessed() {
    let ord = ByteOrder::Little;
    let mut heap = member_row(0x6C, 0, &[b"X"]);
    heap[0] = flag::C | flag::H | flag::F; // chained member (no L)
    assert!(matches!(
        piece::parse_piece(&heap, 0, 0, ord, PieceFormat::default()),
        Err(DecodeError::Unsupported(_))
    ));
    heap[0] = flag::K | flag::C | flag::H | flag::F | flag::L;
    assert!(matches!(
        piece::parse_piece(&heap, 0, 0, ord, PieceFormat::default()),
        Err(DecodeError::Malformed { .. })
    ));
    let rd = PieceFormat {
        row_dependencies: true,
    };
    let m = member_row(0x6C, 0, &[b"X"]);
    assert!(matches!(
        piece::parse_piece(&m, 0, 0, ord, rd),
        Err(DecodeError::Unsupported(_))
    ));
}

/// `ROWDEPENDENCIES` block from the aliyun BBED study (file 8 block 143,
/// little-endian): `2c 00 01 | 00 00 0e 33 70 b3 | 02 c1 02`, `tl: 12`,
/// `dscn 0x0000.b370330e`, kdbh `flag=--R-----`.
#[test]
fn row_dependency_scn_is_skipped_and_reported() {
    let raw: [&[u8]; 2] = [
        &[
            0x2C, 0x00, 0x01, 0x00, 0x00, 0x0E, 0x33, 0x70, 0xB3, 0x02, 0xC1, 0x02,
        ],
        &[
            0x2C, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xC1, 0x04,
        ],
    ];
    let b = build(
        ByteOrder::Little,
        Layout::Mssm,
        kdbh_flag::ROW_DEPENDENCIES,
        &[raw.iter().map(|r| r.to_vec()).collect()],
    );
    let db = DataBlock::parse(&b, ByteOrder::Little).unwrap();
    assert!(db.piece_format().row_dependencies);
    let r0 = db.row(0).unwrap().unwrap();
    assert_eq!(r0.len, 12, "dump: tl: 12");
    assert_eq!(r0.dscn.unwrap().0, 0xB370_330E);
    assert_eq!(r0.columns, vec![Some(&[0xC1, 0x02][..])]);
    let r1 = db.row(1).unwrap().unwrap();
    assert_eq!(r1.dscn.unwrap().0, 0, "commit SCN still in the ITL");
    assert_eq!(r1.columns, vec![Some(&[0xC1, 0x04][..])]);

    // The same bytes read WITHOUT the block flag decode to different columns:
    // this is the silent-corruption case the flag prevents.
    let wrong = piece::parse_piece(raw[0], 0, 0, ByteOrder::Little, PieceFormat::default());
    assert!(wrong.map_or(true, |p| p.columns != r0.columns));
}

#[test]
fn rowdep_chained_piece_is_unsupported() {
    let rd = PieceFormat {
        row_dependencies: true,
    };
    // H F without L: would need nrid placement relative to dscn (undocumented).
    let p = [
        flag::H | flag::F,
        0,
        1,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        1,
        0x41,
    ];
    assert!(matches!(
        piece::parse_piece(&p, 0, 0, ByteOrder::Big, rd),
        Err(DecodeError::Unsupported(_))
    ));
}

/// Basic-compression header from a published dump: `flag=-0------ ntab=2
/// nrow=700 fsbo=0x5b0` (the `9ir2` fields make `fsbo` 34 bytes larger than
/// `14 + 4·ntab + 2·nrow`). Must be refused, never decoded as plain rows.
#[test]
fn compressed_blocks_are_refused() {
    for ord in [ByteOrder::Little, ByteOrder::Big] {
        let mut b = build(
            ord,
            Layout::Mssm,
            0,
            &[vec![vec![0x2C, 0, 0]], vec![vec![0x2C, 0, 0]]],
        );
        let kdbh = 0x2C + 48;
        b[kdbh] = kdbh_flag::COMPRESSED;
        assert!(matches!(
            DataBlock::parse(&b, ord),
            Err(DecodeError::Unsupported(_))
        ));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]
    /// Arbitrary bytes never panic and never report a length past the input.
    #[test]
    fn parse_piece_total(data in proptest::collection::vec(any::<u8>(), 0..300), big in any::<bool>(), rd in any::<bool>()) {
        let ord = if big { ByteOrder::Big } else { ByteOrder::Little };
        if let Ok(p) = piece::parse_piece(&data, 0, 0, ord, PieceFormat { row_dependencies: rd }) {
            prop_assert!(p.len <= data.len());
            prop_assert!(p.cluster.is_none() || p.dscn.is_none());
        }
    }
}

proptest! {
    /// Random cluster blocks: every key/member header field and column
    /// round-trips in both byte orders and both layouts.
    #[test]
    fn cluster_roundtrip(
        big in any::<bool>(),
        assm in any::<bool>(),
        curc in any::<u16>(),
        comc in any::<u16>(),
        dba in any::<u32>(),
        sl in any::<u16>(),
        keys in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..20), 1..3),
        members in proptest::collection::vec(
            proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..40), 0..6),
            1..5,
        ),
    ) {
        let ord = if big { ByteOrder::Big } else { ByteOrder::Little };
        let layout = if assm { Layout::Assm } else { Layout::Mssm };
        let kr: Vec<&[u8]> = keys.iter().map(Vec::as_slice).collect();
        let mrows: Vec<Vec<u8>> = members
            .iter()
            .map(|m| member_row(0x6C, 0, &m.iter().map(Vec::as_slice).collect::<Vec<_>>()))
            .collect();
        let b = build(ord, layout, 0, &[vec![key_row(curc, comc, (dba, sl), &kr, ord)], mrows]);
        let db = DataBlock::parse(&b, ord).unwrap();
        let key = db.cluster_key(0).unwrap();
        prop_assert_eq!(key.cluster, Some(ClusterPart::Key {
            current: curc, committed: comc, prev: (dba, sl), next: (dba, sl),
        }));
        prop_assert_eq!(key.columns.len(), keys.len());
        for (i, m) in members.iter().enumerate() {
            let p = db.row(1 + i as u16).unwrap().unwrap();
            prop_assert_eq!(p.cluster, Some(ClusterPart::Member { key_index: 0 }));
            let got: Vec<&[u8]> = p.columns.iter().map(|c| c.unwrap()).collect();
            let want: Vec<&[u8]> = m.iter().map(Vec::as_slice).collect();
            prop_assert_eq!(got, want);
            let all = cluster_columns(&key, &p, keys.len()).unwrap();
            prop_assert_eq!(all.len(), keys.len() + m.len());
        }
    }

    /// Row-dependency rows round-trip any SCN and columns.
    #[test]
    fn rowdep_roundtrip(
        big in any::<bool>(),
        wrap in any::<u16>(),
        base in any::<u32>(),
        cols in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..60), 0..8),
    ) {
        let ord = if big { ByteOrder::Big } else { ByteOrder::Little };
        let mut r = vec![0x2C, 0, cols.len() as u8];
        r.extend(u16b(wrap, ord));
        r.extend(u32b(base, ord));
        for c in &cols { r.extend(col(c)); }
        let b = build(ord, Layout::Mssm, kdbh_flag::ROW_DEPENDENCIES, &[vec![r.clone()]]);
        let db = DataBlock::parse(&b, ord).unwrap();
        let p = db.row(0).unwrap().unwrap();
        prop_assert_eq!(p.len, r.len());
        prop_assert_eq!(p.dscn.unwrap().0, (u64::from(wrap) << 32) | u64::from(base));
        let got: Vec<&[u8]> = p.columns.iter().map(|c| c.unwrap()).collect();
        let want: Vec<&[u8]> = cols.iter().map(Vec::as_slice).collect();
        prop_assert_eq!(got, want);
    }
}
