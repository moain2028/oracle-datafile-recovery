//! DB3F — Database Forensic File Format (Wagner, Rasin, Heart, Jacob,
//! Grier; DFRWS USA 2019, *Digital Investigation* 29:S42–S50).
//!
//! Line 1: a JSON header object. Every following line: one JSON object per
//! database page. ODR emits the standard header fields plus an `odr`
//! namespace (declared in `context`) carrying what the standard has no slot
//! for: the per-block damage verdict, the layout witnesses and the file
//! hashes. Records are **not** carved here (`odr extract` / `scavenge` do
//! that with their verified / inferred / rejected discipline); the
//! `records` array is therefore empty and `odr.damage` tells a DB3F consumer
//! why a page cannot be trusted.
//!
//! Scalability rule from the paper is kept: one page per line, so a 1 TB
//! file streams.

use super::layout::ReadAt;
use super::{Report, Verdict, verdict};
use crate::bytes::ByteOrder;
use std::io::Write;

/// Namespace URI for ODR's custom fields.
pub const ODR_URI: &str = "https://github.com/moain2028/oracle-datafile-recovery/db3f";

fn page_type(v: &Verdict) -> &'static str {
    use super::BlockKind as K;
    match v.kind {
        K::Data => "table",
        K::Index => "index",
        K::Lob => "lob",
        K::Empty => "empty",
        K::FileHeader => "file_header",
        K::OsHeader => "os_header",
        K::Undo | K::UndoHeader => "undo",
        K::SegmentHeader => "segment_header",
        K::Space => "space_management",
        K::LobChunk => "lob_chunk",
        K::DataOther(_) | K::Other(_) => "other",
        K::Unknown => "unknown",
    }
}

/// Write the DB3F header line.
///
/// # Errors
/// I/O errors on `out`.
pub fn write_header<W: Write>(
    out: &mut W,
    r: &Report,
    evidence_file: &str,
    sha_before: &str,
    carving_time: &str,
) -> std::io::Result<()> {
    let hdr = serde_json::json!({
        "context": [{"name": "odr", "uri": ODR_URI}],
        "evidence_file": evidence_file,
        "forensic_tool": format!("odr {} (diagnose)", env!("CARGO_PKG_VERSION")),
        "carving_time": carving_time,
        "dbms": "Oracle Database (datafile)",
        "page_size": r.layout.block_size,
        "odr": {
            "sha256_before": sha_before,
            "sha256_scanned": r.sha256_scanned,
            "unchanged": r.sha256_scanned == sha_before,
            "byte_order": format!("{:?}", r.layout.order),
            "layout_source": r.layout.source,
            "declared_blocks_header": r.layout.declared_blocks_header,
            "declared_blocks_os": r.layout.declared_blocks_os,
            "blocks_present": r.blocks_present,
            "last_block_partial": r.last_block_partial,
            "counters": r.counters,
            "findings": r.findings.iter().map(|f| serde_json::json!({
                "code": f.code, "severity": f.severity, "confidence_milli": f.confidence_milli,
                "blocks": f.blocks, "evidence": f.evidence, "oracle_errors": f.oracle_errors,
            })).collect::<Vec<_>>(),
            "triage": r.triage.as_ref().map(|t| serde_json::json!({
                "score": t.score, "band": t.band, "clean_used_milli": t.clean_used_milli,
            })),
        }
    });
    serde_json::to_writer(&mut *out, &hdr)?;
    out.write_all(b"\n")
}

/// Stream every page as a DB3F page object. Re-reads the source so the
/// export does not need the per-block verdicts kept in memory.
///
/// # Errors
/// I/O errors from the source or `out`.
pub fn write_pages<R: ReadAt + ?Sized, W: Write>(
    src: &mut R,
    r: &Report,
    out: &mut W,
) -> std::io::Result<u64> {
    let bs = r.layout.block_size.max(1);
    let order: ByteOrder = r.layout.order;
    let chunk = super::CHUNK_LEN
        .checked_div(bs)
        .unwrap_or(1)
        .saturating_mul(bs);
    let mut buf = vec![0u8; chunk.max(bs)];
    let mut offset: u64 = 0;
    let mut index: u64 = 0;
    loop {
        let got = src.read_at(offset, &mut buf)?;
        if got == 0 {
            break;
        }
        let Some(data) = buf.get(..got) else { break };
        let mut pos = 0usize;
        while pos < got {
            let end = pos.saturating_add(bs).min(got);
            let Some(blk) = data.get(pos..end) else { break };
            let v = verdict(blk, bs, order, index, r.options.depth);
            let page = serde_json::json!({
                "offset": offset.saturating_add(u64::try_from(pos).unwrap_or(0)),
                "page_id": index.to_string(),
                "object_id": v.objd.map(|o| o.to_string()),
                "page_type": page_type(&v),
                "schema": [],
                "records": [],
                "odr": {
                    "block": index,
                    "kind": v.kind,
                    "damage": v.damage,
                    "type_byte": v.type_byte,
                    "scn": v.scn,
                    "live_rows": v.live_rows,
                    "trusted": !v.damage.is_damaged(),
                }
            });
            serde_json::to_writer(&mut *out, &page)?;
            out.write_all(b"\n")?;
            index = index.saturating_add(1);
            pos = end;
        }
        offset = offset.saturating_add(u64::try_from(got).unwrap_or(u64::MAX));
        if got < buf.len() {
            break;
        }
    }
    Ok(index)
}
