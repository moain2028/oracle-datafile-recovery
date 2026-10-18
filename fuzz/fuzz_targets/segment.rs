#![no_main]
//! Extent-map parser and segment walker on arbitrary bytes: never panics,
//! never allocates beyond the documented bounds, always terminates even when
//! every "next map block" points back at itself.
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    use odr_core::ByteOrder;
    use odr_core::block::Rdba;
    use odr_core::dict::segment::{ExtentMap, MAX_MAP_BLOCKS, MAX_SEGMENT_BLOCKS, walk_segment};
    for order in [ByteOrder::Little, ByteOrder::Big] {
        if let Ok(m) = ExtentMap::parse(data, order) {
            assert!(m.extents.len() <= 508);
            let total: u64 = m.extents.iter().map(|e| u64::from(e.blocks)).sum();
            assert!(total <= MAX_SEGMENT_BLOCKS);
        }
        // Walker: every fetch returns the same block (self-loop), which must
        // terminate through the visited set / MAX_MAP_BLOCKS bound.
        let mut fetches = 0usize;
        if let Ok(s) = walk_segment(data, order, Rdba(0), |_| {
            fetches += 1;
            Some(data.to_vec())
        }) {
            assert!(s.len() <= MAX_SEGMENT_BLOCKS);
        }
        assert!(fetches <= MAX_MAP_BLOCKS + 1);
    }
});
