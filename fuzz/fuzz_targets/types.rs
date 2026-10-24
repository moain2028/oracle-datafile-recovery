#![no_main]
use odr_core::types::*;
libfuzzer_sys::fuzz_target!(|d: &[u8]| {
    let _ = OraTimestamp::decode(d).map(|t| t.to_string());
    let _ = OraTimestamp::decode_tz(d).map(|t| t.to_string());
    let _ = IntervalYm::decode(d).map(|t| t.to_string());
    let _ = IntervalDs::decode(d).map(|t| t.to_string());
    let _ = decode_binary_float(d);
    let _ = decode_binary_double(d);
    let _ = OraRowid::decode(d).map(|r| r.to_string());
    // LV-068: LOB locator (inline / out-of-line / malformed), never panics.
    let _ = LobLocator::parse(d);
});
