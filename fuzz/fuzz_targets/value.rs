#![no_main]
use odr_core::charset::Charset;
use odr_core::value::{Charsets, ColumnType, decode_value};
libfuzzer_sys::fuzz_target!(|d: &[u8]| {
    let Some((&code, rest)) = d.split_first() else { return };
    let cs = Charsets { database: Charset::Ar8MsWin1256, national: Charset::Al16Utf16 };
    let codes = [1u16, 2, 12, 23, 69, 96, 100, 101, 112, 113, 180, 181, 182, 183, 231];
    let ty = ColumnType::from_oracle(codes[usize::from(code) % codes.len()], 1 + (code & 1));
    let _ = decode_value(Some(rest), ty, cs).canonical();
});
