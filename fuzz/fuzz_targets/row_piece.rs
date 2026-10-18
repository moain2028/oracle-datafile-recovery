#![no_main]
//! Row-piece decoder on arbitrary bytes: every flag combination (heap,
//! cluster key/member, chained, migrated) with and without row dependencies.
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    use odr_core::row::{PieceFormat, piece::parse_piece};
    for order in [odr_core::ByteOrder::Little, odr_core::ByteOrder::Big] {
        for row_dependencies in [false, true] {
            for dscn_len in [6u8, 7, 8] {
                let fmt = PieceFormat {
                    row_dependencies,
                    dscn_len,
                };
                if let Ok(p) = parse_piece(data, 0, 0, order, fmt) {
                    // Decoded length can never exceed the input.
                    assert!(p.len <= data.len());
                }
            }
        }
    }
});
