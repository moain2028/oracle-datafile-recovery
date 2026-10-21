#![no_main]
//! Whole-segment reader over an arbitrary one-block image: integrity
//! classification, LV-061 untrusted enumeration, LV-062 structural check,
//! LV-065 torn frontier. Must never panic for any input.
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    use odr_core::block::{Rdba, RdbaScheme};
    use odr_core::dict::rows::read_heap;
    use odr_core::dict::segment::{Extent, SegmentBlocks};
    use odr_core::dict::source::{FileInfo, ImageSet};
    // Block size from the first byte (2 KiB … 32 KiB); the image is the
    // input padded/truncated to exactly one block.
    let Some(&sel) = data.first() else { return };
    let bs = 2048usize << (sel % 5);
    let mut image = data.get(1..).unwrap_or_default().to_vec();
    image.resize(bs, 0);
    // The data object id is read from the block itself so that the
    // "foreign object" path is not the only one ever taken.
    let objd = odr_core::row::data_object_id(&image, odr_core::ByteOrder::Little).unwrap_or(1);
    let Some(dba) = Rdba::smallfile(16, 0) else { return };
    let seg = SegmentBlocks {
        objd,
        extents: vec![Extent { dba, blocks: 1 }],
        issues: Vec::new(),
        hwm: None,
    };
    for o in [odr_core::ByteOrder::Little, odr_core::ByteOrder::Big] {
        let mut src = ImageSet::new(o);
        src.add(
            FileInfo {
                file: 16,
                relative: Some(16),
                block_size: bs,
                scheme: RdbaScheme::Smallfile,
                tablespace: None,
            },
            image.clone(),
        );
        let out = read_heap(&src, &seg, 16, objd);
        for r in out.rows.iter().flatten() {
            // span must stay inside the block
            assert!(r.span.0.saturating_add(r.span.1) <= bs);
        }
        for (_, f) in &out.torn_frontier {
            assert!(*f <= bs && f % 512 == 0);
        }
    }
});
