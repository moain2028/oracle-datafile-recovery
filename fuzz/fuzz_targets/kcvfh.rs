#![no_main]
//! Datafile header on arbitrary bytes: `parse` in both orders and `detect`
//! never panic; `detect` never returns a header that `parse` would not.
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    use odr_core::ByteOrder;
    use odr_core::dict::kcvfh::FileHeader;
    let le = FileHeader::parse(data, ByteOrder::Little).ok();
    let be = FileHeader::parse(data, ByteOrder::Big).ok();
    if let Ok(h) = FileHeader::detect(data) {
        // Detect is only allowed to succeed when exactly one order parses.
        assert!(le.is_some() != be.is_some(), "detect must refuse ambiguity");
        let same = if h.order == ByteOrder::Little { le.as_ref() } else { be.as_ref() };
        assert_eq!(same, Some(&h));
    }
    for h in [le, be].into_iter().flatten() {
        // Block size is one of Oracle's, file number fits the scheme used.
        assert!(odr_core::block::BLOCK_SIZES.contains(&(h.block_size as usize)));
        if let Some(r) = h.relative_file {
            assert!((1..=1023).contains(&r));
        }
    }
});
