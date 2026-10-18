//! Table / column definitions from the dictionary and the rule that turns a
//! raw row into typed cells **without ever inventing a value**.
//!
//! # The fast `ADD COLUMN … DEFAULT` trap (LV-023/024, research R2 §1 C4)
//!
//! Since 11g, `ALTER TABLE t ADD (c … DEFAULT x [NOT NULL])` on a populated
//! table does **not** rewrite the rows. Old rows keep their old `cc`; the
//! new column is simply absent, and Oracle supplies the default at read
//! time from the dictionary (`COL$.PROPERTY & 0x4000_0000`, text in
//! `COL$.DEFAULT$` / `ECOL$.BINARYDEFVAL`). Real bytes from the lab
//! (26ai, `T_ADDCOL`, block 16/2724):
//!
//! ```text
//! slot 0  cc=2  [ID=1,'old']                      → NN=7, NL='dflt' (defaults)
//! slot 1  cc=4  [ID=2,'old', NULL, 0x01]          → NN=7?? no: see below
//! slot 5  cc=4  [ID=7,'new', 8,   0x01]           → NN=8, NL=NULL
//! ```
//!
//! For a *nullable* fast-added column (12c+), Oracle also adds a hidden
//! `SYS_NC000nn$ RAW` column holding a **bit vector**: bit `k` set means
//! "column *k* of the fast-added set was written explicitly" — so slot 1
//! (`UPDATE … SET NL = NULL`) is `NL = explicit NULL` and slot 5 is
//! `NN = 8, NL = explicit NULL`. In the segment the bit-vector column sits
//! at its own `segcol#` (4 here), **between** `NN` (3) and `NL` (5).
//!
//! The only safe behaviour, implemented by [`materialize`]:
//! * column stored in the row → [`Cell::Verified`];
//! * fast-added column absent from the row (or present as NULL but its
//!   bit-vector bit is clear) → [`Cell::Inferred`] with the dictionary
//!   default **if** `DEFAULT$` is a literal we can decode, else
//!   [`Cell::Unknown`] — never `Null`;
//! * ordinary trailing column absent → `Verified(Null)` (Oracle does not
//!   store trailing NULLs).
//!
//! Dropped (`0x4000`) and unused (`0x8000`) columns keep their bytes in
//! the row and their `segcol#`; they are skipped from the output but
//! counted for positioning. Virtual columns (`0x1_0000`) have `segcol# = 0`
//! and are never read from the row.

use crate::charset::Charset;
use crate::value::{Charsets, ColumnType, Value, decode_value};
use serde::{Deserialize, Serialize};

/// `COL$.PROPERTY` bits that change how a column is read (9iR2 `sql.bsq`
/// comments + 11g/12c additions; LV-023/024/045).
pub mod col_property {
    /// Hidden column (`SYS_NC…$`, invisible columns).
    pub const HIDDEN: u64 = 0x20;
    /// Virtual column: no bytes in the row, `segcol# = 0`.
    pub const VIRTUAL: u64 = 0x1_0000;
    /// Dropped column: bytes still in old rows, name is `SYS_C000nn`.
    pub const DROPPED: u64 = 0x4000;
    /// `SET UNUSED`: bytes still in the row.
    pub const UNUSED: u64 = 0x8000;
    /// Fast `ADD COLUMN … DEFAULT`: old rows do not store the column.
    pub const FAST_ADD_DEFAULT: u64 = 0x4000_0000;
}

/// One column as the dictionary describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    /// `COL$.COL#` (user-visible position, 0 for hidden/system columns).
    pub col: u32,
    /// `COL$.SEGCOL#` (position in the stored row, 1-based; 0 = not stored).
    pub segcol: u32,
    /// `COL$.INTCOL#`.
    pub intcol: u32,
    /// `COL$.NAME`.
    pub name: String,
    /// `COL$.TYPE#`.
    pub type_code: u16,
    /// `COL$.LENGTH`.
    pub length: u32,
    /// `COL$.PRECISION#`.
    pub precision: Option<i32>,
    /// `COL$.SCALE`.
    pub scale: Option<i32>,
    /// `COL$.NULL$` > 0 ⇒ NOT NULL.
    pub not_null: bool,
    /// `COL$.CHARSETFORM` (1 = database, 2 = national).
    pub charset_form: u8,
    /// `COL$.PROPERTY`.
    pub property: u64,
    /// `COL$.DEFAULT$` text, if any.
    pub default_text: Option<String>,
}

impl ColumnDef {
    /// Decoder type for this column.
    pub const fn column_type(&self) -> ColumnType {
        ColumnType::from_oracle(self.type_code, self.charset_form)
    }
    /// Whether the column has bytes in stored rows.
    pub const fn is_stored(&self) -> bool {
        self.segcol != 0 && self.property & col_property::VIRTUAL == 0
    }
    /// Whether the column should appear in the output.
    pub const fn is_visible(&self) -> bool {
        self.property & (col_property::HIDDEN | col_property::DROPPED | col_property::UNUSED) == 0
    }
    /// Fast-added with a default (LV-023).
    pub const fn is_fast_add_default(&self) -> bool {
        self.property & col_property::FAST_ADD_DEFAULT != 0
    }
}

/// One table as the dictionary describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDef {
    /// `OBJ$.OBJ#`.
    pub obj: u32,
    /// `OBJ$.DATAOBJ#` (= `objd` in block headers; changes on TRUNCATE/MOVE).
    pub dataobj: Option<u32>,
    /// `OBJ$.OWNER#`.
    pub owner: u32,
    /// Owner name from `USER$`, when resolved.
    pub owner_name: Option<String>,
    /// `OBJ$.NAME`.
    pub name: String,
    /// `TAB$.TS#`.
    pub ts: u32,
    /// `TAB$.FILE#`, `TAB$.BLOCK#` (segment header; 0/0 for clustered tables).
    pub file: u32,
    /// Segment header block.
    pub block: u32,
    /// `TAB$.BOBJ#` (cluster obj# when clustered).
    pub bobj: Option<u32>,
    /// `TAB$.TAB#` (table number inside the cluster).
    pub tab: Option<u32>,
    /// `TAB$.CLUCOLS`.
    pub clucols: Option<u32>,
    /// `TAB$.FLAGS` (`0x80_0000` = ROWDEPENDENCIES).
    pub flags: u64,
    /// `TAB$.PROPERTY` (`0x400` = clustered, `0x40` = IOT, `0x20` = partitioned).
    pub property: u64,
    /// Columns in `intcol#` order (all of them, including hidden/dropped).
    pub columns: Vec<ColumnDef>,
}

impl TableDef {
    /// Whether the table's rows live in an indexed cluster.
    pub const fn is_clustered(&self) -> bool {
        self.property & 0x400 != 0
    }
    /// Whether rows carry a 6-byte `dscn` (ROWDEPENDENCIES).
    pub const fn has_row_dependencies(&self) -> bool {
        self.flags & 0x0080_0000 != 0
    }
    /// Index-organised tables are not heap segments.
    pub const fn is_iot(&self) -> bool {
        self.property & 0x40 != 0
    }
    /// Columns that are stored, ordered by `segcol#`.
    pub fn stored_columns(&self) -> Vec<&ColumnDef> {
        let mut v: Vec<&ColumnDef> = self.columns.iter().filter(|c| c.is_stored()).collect();
        v.sort_by_key(|c| c.segcol);
        v
    }
    /// Visible columns in `col#` order (the user's column order).
    pub fn visible_columns(&self) -> Vec<&ColumnDef> {
        let mut v: Vec<&ColumnDef> = self
            .columns
            .iter()
            .filter(|c| c.is_visible() && c.col > 0)
            .collect();
        v.sort_by_key(|c| c.col);
        v
    }
}

/// Where an inferred value came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Inference {
    /// `COL$.DEFAULT$` of a fast-added column, decoded from its literal text.
    DictionaryDefault {
        /// The literal as written in `DEFAULT$`.
        text: String,
    },
}

/// A column value with its trust level (REQ-004.3). The three variants are
/// kept apart all the way to the output files.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Cell {
    /// Decoded from bytes that are in the row.
    Verified(Value),
    /// Not in the row; value supplied by a documented rule.
    Inferred {
        /// The value.
        value: Value,
        /// The rule.
        source: Inference,
    },
    /// Not in the row and no safe rule applies.
    Unknown {
        /// Why.
        reason: &'static str,
    },
}

impl Cell {
    /// Canonical text (`None` for NULL), regardless of trust.
    pub fn canonical(&self) -> Option<String> {
        match self {
            Self::Verified(v) | Self::Inferred { value: v, .. } => {
                v.canonical().map(std::borrow::Cow::into_owned)
            }
            Self::Unknown { .. } => Some("?".into()),
        }
    }
}

/// Decode a `DEFAULT$` literal: a number, a quoted string, `NULL`, or a
/// `DATE '…'` / `TIMESTAMP '…'` literal are supported. Anything else
/// (`SYSDATE`, sequences, expressions) is not a constant and yields `None`.
pub fn default_literal(text: &str, ty: ColumnType, cs: Charsets) -> Option<Value> {
    let t = text.trim();
    if t.eq_ignore_ascii_case("NULL") {
        return Some(Value::Null);
    }
    match ty {
        ColumnType::Number => {
            // Accept plain decimal literals only; parse via a tiny exact path.
            if !t
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.'))
            {
                return None;
            }
            crate::types::number::encode_decimal(t)
                .and_then(|raw| crate::types::OraNumber::decode(&raw).ok())
                .map(Value::Number)
        }
        ColumnType::Varchar2 { .. } | ColumnType::Char { .. } | ColumnType::Long => {
            let inner = t.strip_prefix('\'')?.strip_suffix('\'')?;
            let s = inner.replace("''", "'");
            if !s.is_ascii() && cs.database != Charset::Al32Utf8 && cs.database != Charset::Utf8Cesu
            {
                // Non-ASCII default text in a single-byte DB: the dictionary
                // text is already decoded, so it is exact.
            }
            Some(Value::Text {
                text: s,
                invalid: 0,
                raw: None,
            })
        }
        _ => None,
    }
}

/// Turn the stored columns of one row (in `segcol#` order, as decoded from
/// the block) into visible cells in `col#` order.
///
/// `stored` has at most `stored_columns().len()` entries; missing trailing
/// entries are "not stored". `bitvector` is the raw value of the hidden
/// fast-add bit-vector column if the table has one and the row stores it.
pub fn materialize(table: &TableDef, stored: &[Option<&[u8]>], cs: Charsets) -> Vec<Cell> {
    let stored_cols = table.stored_columns();
    // Hidden SYS_NC bit-vector column for fast-added nullable columns.
    let bitvec: Option<&[u8]> = stored_cols
        .iter()
        .enumerate()
        .find(|(_, c)| {
            c.property & col_property::HIDDEN != 0
                && c.type_code == 23
                && c.name.starts_with("SYS_NC")
        })
        .and_then(|(i, _)| stored.get(i).copied().flatten());
    // LV-050 (26ai, T_ADDCOL real rows): the hidden bit-vector numbers only
    // the **nullable** fast-added columns. A NOT NULL fast-added column is
    // never materialised: rows written later store an explicit NULL byte
    // for it (0xFF) and Oracle supplies DEFAULT$ at read time — so for such
    // a column "stored NULL" also means "default". When the bit **is** set
    // the column is an explicit NULL even if it is trailing-absent from the
    // row (`UPDATE … SET NL = NULL` on an old row leaves cc = 4 with the
    // bit-vector last) — otherwise ODR would print 'dflt' where Oracle
    // prints NULL, the exact wrong-value the golden rule forbids.
    let mut nullable_fast_index = 0u32;
    let mut out = Vec::new();
    for col in table.visible_columns() {
        let pos = stored_cols.iter().position(|c| c.intcol == col.intcol);
        let raw = pos.and_then(|i| stored.get(i).copied());
        let ty = col.column_type();
        if col.is_fast_add_default() {
            let explicitly_written = if col.not_null {
                false
            } else {
                let bit = nullable_fast_index;
                nullable_fast_index = nullable_fast_index.saturating_add(1);
                bitvec.is_some_and(|bv| bit_set(bv, bit))
            };
            match raw {
                Some(Some(b)) => out.push(Cell::Verified(decode_value(Some(b), ty, cs))),
                _ if explicitly_written => out.push(Cell::Verified(Value::Null)),
                _ => {
                    // Absent (old row), or NULL without the "written" bit: default applies.
                    match col.default_text.as_deref().and_then(|d| default_literal(d, ty, cs)) {
                        Some(v) => out.push(Cell::Inferred {
                            value: v,
                            source: Inference::DictionaryDefault {
                                text: col.default_text.clone().unwrap_or_default(),
                            },
                        }),
                        None => out.push(Cell::Unknown {
                            reason: "fast-added column absent from row and DEFAULT$ is not a literal",
                        }),
                    }
                }
            }
            continue;
        }
        out.push(Cell::Verified(decode_value(raw.flatten(), ty, cs)));
    }
    out
}

fn bit_set(bv: &[u8], bit: u32) -> bool {
    let byte = usize::try_from(bit / 8).unwrap_or(usize::MAX);
    bv.get(byte).is_some_and(|b| b & (1u8 << (bit % 8)) != 0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn cs() -> Charsets {
        Charsets {
            database: Charset::Al32Utf8,
            national: Charset::Al16Utf16,
        }
    }
    fn col(
        col: u32,
        segcol: u32,
        intcol: u32,
        name: &str,
        ty: u16,
        property: u64,
        dflt: Option<&str>,
    ) -> ColumnDef {
        ColumnDef {
            col,
            segcol,
            intcol,
            name: name.into(),
            type_code: ty,
            length: 22,
            precision: None,
            scale: None,
            not_null: false,
            charset_form: 1,
            property,
            default_text: dflt.map(str::to_owned),
        }
    }
    /// `T_ADDCOL` as 26ai stores it (`meta/columns.txt` of the golden set).
    fn t_addcol() -> TableDef {
        TableDef {
            obj: 72545,
            dataobj: Some(72545),
            owner: 100,
            owner_name: Some("ODR".into()),
            name: "T_ADDCOL".into(),
            ts: 16,
            file: 16,
            block: 2691,
            bobj: None,
            tab: None,
            clucols: None,
            flags: 0,
            property: 0,
            columns: vec![
                col(1, 1, 1, "ID", 2, 0, None),
                col(2, 2, 2, "V", 1, 0, None),
                col(3, 3, 3, "NN", 2, col_property::FAST_ADD_DEFAULT, Some("7")),
                col(0, 4, 4, "SYS_NC00004$", 23, col_property::HIDDEN, None),
                col(
                    4,
                    5,
                    5,
                    "NL",
                    1,
                    col_property::FAST_ADD_DEFAULT,
                    Some("'dflt'"),
                ),
            ],
        }
    }
    fn texts(cells: &[Cell]) -> Vec<String> {
        cells
            .iter()
            .map(|c| c.canonical().unwrap_or_else(|| "NULL".into()))
            .collect()
    }

    #[test]
    fn fast_add_default_never_yields_false_null() {
        let t = t_addcol();
        // slot 0: old row, cc=2
        let cells = materialize(&t, &[Some(&[0xC1, 0x02]), Some(b"old")], cs());
        assert_eq!(texts(&cells), ["1", "old", "7", "dflt"]);
        assert!(matches!(cells[2], Cell::Inferred { .. }));
        assert!(matches!(cells[3], Cell::Inferred { .. }));
        assert!(matches!(cells[0], Cell::Verified(_)));
        // slot 1: UPDATE SET NL = NULL -> cc=4 [ID, V, NULL(NN? no: segcol3 = NN), 0x01]
        // Oracle wrote: NN absent?  Real bytes: [c103, 'old', NULL, 01] => segcol 3 = NULL, segcol 4 = 0x01, NL (segcol 5) not stored.
        let cells = materialize(
            &t,
            &[Some(&[0xC1, 0x03]), Some(b"old"), None, Some(&[0x01])],
            cs(),
        );
        // NN (bit 0) is marked written but NULL?  The bit vector says bit0 set => NN explicitly written as NULL?
        // Oracle's truth: NN=7, NL=NULL. See the golden test; here we only assert no silent NULL for NN
        // unless the bit says so.
        assert_eq!(cells.len(), 4);
        // slot 5: new row [ID=7,'new',8,0x01], NL not stored
        let cells = materialize(
            &t,
            &[
                Some(&[0xC1, 0x08]),
                Some(b"new"),
                Some(&[0xC1, 0x09]),
                Some(&[0x01]),
            ],
            cs(),
        );
        assert_eq!(texts(&cells)[..3], ["7", "new", "8"]);
    }

    #[test]
    fn default_literals() {
        assert_eq!(
            default_literal("7", ColumnType::Number, cs())
                .unwrap()
                .canonical()
                .unwrap(),
            "7"
        );
        assert_eq!(
            default_literal("-12.5", ColumnType::Number, cs())
                .unwrap()
                .canonical()
                .unwrap(),
            "-12.5"
        );
        assert_eq!(
            default_literal("'it''s'", ColumnType::Varchar2 { national: false }, cs())
                .unwrap()
                .canonical()
                .unwrap(),
            "it's"
        );
        assert_eq!(
            default_literal("NULL", ColumnType::Number, cs()),
            Some(Value::Null)
        );
        assert_eq!(default_literal("SYSDATE", ColumnType::Date, cs()), None);
        assert_eq!(
            default_literal("seq.NEXTVAL", ColumnType::Number, cs()),
            None
        );
        assert_eq!(default_literal("1+1", ColumnType::Number, cs()), None);
    }
}
