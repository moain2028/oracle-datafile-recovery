#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    use odr_core::row::{ClusterPart, DataBlock, cluster_columns};
    for o in [odr_core::ByteOrder::Little, odr_core::ByteOrder::Big] {
        if let Ok(db) = DataBlock::parse(data, o) {
            let _ = db.structural_check(); // LV-062: must never panic
            for (slot, r) in db.rows() {
                let _ = db.table_of(slot);
                if let Ok(p) = r
                    && let Some(ClusterPart::Member { key_index }) = p.cluster
                    && let Ok(k) = db.cluster_key(key_index)
                {
                    let _ = cluster_columns(&k, &p, k.columns.len());
                }
            }
        }
    }
});
