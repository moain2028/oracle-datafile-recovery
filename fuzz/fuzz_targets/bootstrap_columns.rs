#![no_main]
//! bootstrap$ DDL column-list parser on arbitrary text: never panics; every
//! accepted column has a non-empty name, and a `LONG` column (if any) maps
//! to the LONG decoder type.
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    use odr_core::charset::Charset;
    use odr_core::dict::bootstrap::Catalog;
    use odr_core::row::{RowPiece, flag};
    let Ok(text) = std::str::from_utf8(data) else { return };
    // Wrap the fuzzed body in a plausible CREATE TABLE so the column parser
    // is reached regardless of what the fuzzer puts in the body.
    let ddl = format!(
        "CREATE TABLE T$({text}) STORAGE (  OBJNO 4 TABNO 1) CLUSTER C_OBJ#(OBJ#)"
    );
    let obj: &[u8] = &[0xC1, 0x05];
    let row = RowPiece {
        slot: 0,
        flag: flag::H | flag::F | flag::L,
        lock: 0,
        columns: vec![Some(obj), Some(obj), Some(ddl.as_bytes())],
        next: None,
        head: None,
        dscn: None,
        cluster: None,
        len: 0,
    };
    let cat = Catalog::from_rows([row], Charset::Al32Utf8);
    for o in cat.objects.values() {
        for c in &o.columns {
            assert!(!c.name.is_empty());
            if c.type_name == "LONG" {
                assert_eq!(c.column_type(), Some(odr_core::value::ColumnType::Long));
            }
        }
    }
});
