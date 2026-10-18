//! `bootstrap$` rows → catalog of core dictionary objects.
//!
//! `bootstrap$(LINE# NUMBER, OBJ# NUMBER, SQL_TEXT VARCHAR2(4000))` holds the
//! DDL Oracle replays at open, with bootstrap-only storage syntax
//! (`lab-verify` LV-032/033). Published examples (11g, identical since 8.0):
//!
//! ```text
//! CREATE TABLE OBJ$("OBJ#" NUMBER NOT NULL, …) PCTFREE 10 … STORAGE (  INITIAL 16K …
//!     OBJNO 18 EXTENTS (FILE 1 BLOCK 121))
//! CREATE CLUSTER C_OBJ#("OBJ#" NUMBER) PCTFREE 5 … STORAGE (  INITIAL 136K …
//!     OBJNO 2 EXTENTS (FILE 1 BLOCK 144)) SIZE 800
//! CREATE TABLE TAB$("OBJ#" NUMBER NOT NULL, …) STORAGE (  OBJNO 4 TABNO 1) CLUSTER C_OBJ#(OBJ#)
//! CREATE INDEX I_OBJ# ON CLUSTER C_OBJ# … OBJNO 3 EXTENTS (FILE 1 BLOCK 168))
//! CREATE ROLLBACK SEGMENT SYSTEM STORAGE (  INITIAL 112K … OBJNO 0 EXTENTS (FILE 1 BLOCK 9))
//! 8.0.0.0.0                                    (LINE# = OBJ# = -1: version marker)
//! ```
//!
//! Rules (a wrong dictionary is worse than none):
//! * Only `OBJNO`, `TABNO`, `EXTENTS (FILE f BLOCK b)`, the object kind/name
//!   and `CLUSTER name(cols)` are extracted; everything else is ignored.
//! * Keywords are matched as whole words **outside** double-quoted
//!   identifiers, so a column named `"OBJNO"` can never be mistaken for the
//!   storage clause.
//! * A statement that names a value twice with different numbers, a table
//!   that has both `EXTENTS` and `CLUSTER`, a clustered table without
//!   `TABNO`, or an `OBJNO` that disagrees with the row's `OBJ#` is
//!   rejected for that row (reported, never silently fixed).
//! * Duplicate `OBJ#` across rows with different definitions is a catalog
//!   conflict: both are kept out and reported.
//!
//! Sources: dbi-services (Pachot, 2016) for `TAB$`/`C_OBJ#`; askmaclean and
//! chinaunix for `OBJ$`; cnblogs (tianlesoftware 2011) for the full 10g/11g
//! object list and a BBED dump of the first `bootstrap$` row.

// Index arithmetic below is over token positions and byte offsets of one
// `SQL_TEXT` value (VARCHAR2(4000) ⇒ < 2^13): sums of such values with small
// constants cannot overflow `usize`; every access goes through `get`.
#![allow(clippy::arithmetic_side_effects)]

use crate::charset::{self, Charset};
use crate::row::RowPiece;
use crate::types::OraNumber;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Kind of a bootstrap object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObjectKind {
    /// `CREATE TABLE`.
    Table,
    /// `CREATE CLUSTER`.
    Cluster,
    /// `CREATE [UNIQUE] INDEX`.
    Index,
    /// `CREATE ROLLBACK SEGMENT`.
    RollbackSegment,
}

/// Where an object's data lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Storage {
    /// Own segment whose header is at (`FILE`, `BLOCK`).
    Segment {
        /// Absolute file number (bootstrap objects are in file 1).
        file: u32,
        /// Segment-header block number.
        block: u32,
    },
    /// Member of an indexed cluster: rows are in `cluster`'s segment, in
    /// `kdbt[tabno]` of each cluster block.
    Clustered {
        /// Cluster name, e.g. `C_OBJ#`.
        cluster: String,
        /// `TABNO` (= `TAB$.TAB#`).
        tabno: u32,
        /// Cluster-key column names as written in the DDL.
        key_columns: Vec<String>,
    },
}

/// One core object defined in `bootstrap$`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootObject {
    /// `OBJ#` (= `OBJNO`).
    pub obj: u32,
    /// Kind.
    pub kind: ObjectKind,
    /// Name, upper-case as in the DDL, e.g. `TAB$`.
    pub name: String,
    /// Storage.
    pub storage: Storage,
    /// `LINE#` of the defining row.
    pub line: i64,
    /// Column list of a `CREATE TABLE` / `CREATE CLUSTER`, in DDL order
    /// (= `COL#` order for bootstrap tables, which have no virtual, hidden
    /// or dropped columns). Empty for indexes and rollback segments.
    pub columns: Vec<BootColumn>,
}

/// One column of a bootstrap `CREATE TABLE` as written in `SQL_TEXT`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootColumn {
    /// Column name, upper-case / as quoted.
    pub name: String,
    /// Type name as written (`NUMBER`, `VARCHAR2`, `DATE`, `RAW`, `LONG`,
    /// `TIMESTAMP`…), upper-case, without the length.
    pub type_name: String,
    /// Declared length / precision, if written (`VARCHAR2(30)` → 30).
    pub length: Option<u32>,
    /// `NOT NULL` present.
    pub not_null: bool,
}

impl BootColumn {
    /// The [`crate::value::ColumnType`] for this column, if the type name is
    /// one ODR decodes. Bootstrap tables use only `NUMBER`, `VARCHAR2`,
    /// `DATE`, `RAW`, `LONG`, `TIMESTAMP` (and `BLOB`/`CLOB` in 12c+ for a few
    /// spare columns, which are reported unsupported).
    pub fn column_type(&self) -> Option<crate::value::ColumnType> {
        use crate::value::ColumnType as T;
        Some(match self.type_name.as_str() {
            "NUMBER" | "INTEGER" | "FLOAT" => T::Number,
            "VARCHAR2" | "VARCHAR" => T::Varchar2 { national: false },
            "NVARCHAR2" => T::Varchar2 { national: true },
            "CHAR" => T::Char { national: false },
            "NCHAR" => T::Char { national: true },
            "DATE" => T::Date,
            "RAW" => T::Raw,
            "LONG" => T::Long,
            "TIMESTAMP" => T::Timestamp,
            "BINARY_FLOAT" => T::BinaryFloat,
            "BINARY_DOUBLE" => T::BinaryDouble,
            "ROWID" => T::Rowid,
            _ => return None,
        })
    }
}

/// Why a `bootstrap$` row was not used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RowIssue {
    /// Wrong number of columns, NULL, or a column that does not decode.
    BadColumns(String),
    /// `SQL_TEXT` has bytes that are not valid in the database charset.
    BadText {
        /// Number of invalid sequences.
        invalid: usize,
    },
    /// DDL not understood.
    Unparsed(String),
    /// `OBJNO` in the text differs from the row's `OBJ#`.
    ObjMismatch {
        /// `OBJ#` column.
        column: i64,
        /// `OBJNO` in `SQL_TEXT`.
        text: u32,
    },
}

/// Result of reading all `bootstrap$` rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    /// Objects by `OBJ#`.
    pub objects: BTreeMap<u32, BootObject>,
    /// Version marker row text (`LINE# = OBJ# = -1`), e.g. `8.0.0.0.0`.
    pub version: Option<String>,
    /// Rows that were rejected, with their `LINE#` when known.
    pub issues: Vec<(Option<i64>, RowIssue)>,
    /// `OBJ#`s defined by two rows with different content (all removed).
    pub conflicts: Vec<u32>,
}

impl Catalog {
    /// Look up an object by name (exact, upper-case).
    pub fn by_name(&self, name: &str) -> Option<&BootObject> {
        self.objects.values().find(|o| o.name == name)
    }

    /// Build a catalog from decoded `bootstrap$` row pieces (`LINE#`,
    /// `OBJ#`, `SQL_TEXT`). `cs` is the database character set (the DDL is
    /// ASCII, so any ASCII-compatible charset decodes it identically).
    pub fn from_rows<'a, I>(rows: I, cs: Charset) -> Self
    where
        I: IntoIterator<Item = RowPiece<'a>>,
    {
        let mut cat = Self::default();
        let mut seen: BTreeMap<u32, BootObject> = BTreeMap::new();
        for row in rows {
            if row.is_deleted() {
                continue;
            }
            match decode_row(&row, cs) {
                Err((line, issue)) => cat.issues.push((line, issue)),
                Ok(Decoded::Version(v)) => cat.version = Some(v),
                Ok(Decoded::Object(o)) => {
                    if cat.conflicts.contains(&o.obj) {
                        continue;
                    }
                    match seen.get(&o.obj) {
                        Some(prev) if *prev == o => {}
                        Some(_) => {
                            seen.remove(&o.obj);
                            cat.conflicts.push(o.obj);
                        }
                        None => {
                            seen.insert(o.obj, o);
                        }
                    }
                }
            }
        }
        cat.objects = seen;
        cat
    }
}

enum Decoded {
    Version(String),
    Object(BootObject),
}

/// Column `i` of `row` as an exact integer (`None` if absent, NULL or not integral).
fn number(row: &RowPiece<'_>, i: usize) -> Option<i64> {
    OraNumber::decode(row.columns.get(i).copied().flatten()?)
        .ok()?
        .to_i64()
}

fn decode_row(row: &RowPiece<'_>, cs: Charset) -> Result<Decoded, (Option<i64>, RowIssue)> {
    if row.columns.len() != 3 {
        return Err((None, RowIssue::BadColumns("expected 3 columns".into())));
    }
    let line = number(row, 0);
    let Some(line) = line else {
        return Err((None, RowIssue::BadColumns("LINE# is not an integer".into())));
    };
    let fail = |i| Err((Some(line), i));
    let Some(obj) = number(row, 1) else {
        return fail(RowIssue::BadColumns("OBJ# is not an integer".into()));
    };
    let Some(Some(raw)) = row.columns.get(2) else {
        return fail(RowIssue::BadColumns("SQL_TEXT is NULL".into()));
    };
    let text = charset::decode(raw, cs);
    if text.invalid != 0 {
        return fail(RowIssue::BadText {
            invalid: text.invalid,
        });
    }
    if line == -1 && obj == -1 {
        return Ok(Decoded::Version(text.text.trim().to_owned()));
    }
    let o = match parse_ddl(&text.text) {
        Ok(o) => o,
        Err(why) => return fail(RowIssue::Unparsed(why.into())),
    };
    if i64::from(o.obj) != obj {
        return fail(RowIssue::ObjMismatch {
            column: obj,
            text: o.obj,
        });
    }
    Ok(Decoded::Object(BootObject { line, ..o }))
}

/// Token of the bootstrap DDL: a bare word, a quoted identifier, a number,
/// or a single punctuation character.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok<'a> {
    Word(&'a str),
    Quoted(&'a str),
    Punct(char),
}

fn tokenize(s: &str) -> Result<Vec<Tok<'_>>, &'static str> {
    let mut out = Vec::new();
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '"' {
            let start = i + 1;
            let mut end = None;
            for (j, d) in it.by_ref() {
                if d == '"' {
                    end = Some(j);
                    break;
                }
            }
            let end = end.ok_or("unterminated quoted identifier")?;
            out.push(Tok::Quoted(s.get(start..end).ok_or("bad quote")?));
        } else if c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '#' | '.') {
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = it.peek() {
                if d.is_ascii_alphanumeric() || matches!(d, '_' | '$' | '#' | '.') {
                    end = j + d.len_utf8();
                    it.next();
                } else {
                    break;
                }
            }
            out.push(Tok::Word(s.get(i..end).ok_or("bad word")?));
        } else {
            out.push(Tok::Punct(c));
        }
    }
    Ok(out)
}

fn kw(t: Option<&Tok<'_>>, k: &str) -> bool {
    matches!(t, Some(Tok::Word(w)) if w.eq_ignore_ascii_case(k))
}

fn uint(t: Option<&Tok<'_>>) -> Option<u32> {
    match t {
        Some(Tok::Word(w)) if !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) => {
            w.parse().ok()
        }
        _ => None,
    }
}

/// Set `slot` once; a second, different value is a contradiction.
fn once(slot: &mut Option<u32>, v: u32) -> Result<(), &'static str> {
    match *slot {
        Some(old) if old != v => Err("contradictory repeated storage value"),
        _ => {
            *slot = Some(v);
            Ok(())
        }
    }
}

/// Decide where the object lives; every inconsistent combination is an error.
fn storage_of(
    cluster: Option<(String, Vec<String>)>,
    tabno: Option<u32>,
    file: Option<u32>,
    block: Option<u32>,
) -> Result<Storage, &'static str> {
    match (cluster, tabno, file, block) {
        (Some((cluster, key_columns)), Some(tabno), None, None) => Ok(Storage::Clustered {
            cluster,
            tabno,
            key_columns,
        }),
        (Some(_), None, _, _) => Err("clustered table without TABNO"),
        (Some(_), _, _, _) => Err("clustered table with its own EXTENTS"),
        (None, Some(_), _, _) => Err("TABNO without CLUSTER"),
        (None, None, Some(file), Some(block)) => Ok(Storage::Segment { file, block }),
        (None, None, _, _) => Err("no EXTENTS clause"),
    }
}

/// Parse one bootstrap DDL statement. `line` is filled in by the caller.
fn parse_ddl(text: &str) -> Result<BootObject, &'static str> {
    let toks = tokenize(text)?;
    let at = |k: usize| toks.get(k);
    if !kw(at(0), "CREATE") {
        return Err("not a CREATE statement");
    }
    let (kind, name_at) = if kw(at(1), "TABLE") {
        (ObjectKind::Table, 2)
    } else if kw(at(1), "CLUSTER") {
        (ObjectKind::Cluster, 2)
    } else if kw(at(1), "INDEX") {
        (ObjectKind::Index, 2)
    } else if kw(at(1), "UNIQUE") && kw(at(2), "INDEX") {
        (ObjectKind::Index, 3)
    } else if kw(at(1), "ROLLBACK") && kw(at(2), "SEGMENT") {
        (ObjectKind::RollbackSegment, 3)
    } else {
        return Err("unknown object kind");
    };
    let name = match at(name_at) {
        Some(Tok::Word(w)) => w.to_ascii_uppercase(),
        Some(Tok::Quoted(q)) => (*q).to_owned(),
        _ => return Err("missing object name"),
    };

    // Column list `( col type[(n)] [NOT NULL] [DEFAULT …] , … )` directly
    // after the name for tables and clusters. It is parsed, not skipped,
    // because the dictionary schema (which column of OBJ$ is NAME, how many
    // columns TAB$ has) is taken from here, never hard-coded (LV-038).
    let (columns, after_cols) = if matches!(kind, ObjectKind::Table | ObjectKind::Cluster)
        && at(name_at + 1) == Some(&Tok::Punct('('))
    {
        parse_columns(&toks, name_at + 2)?
    } else {
        (Vec::new(), name_at + 1)
    };
    let (mut objno, mut tabno, mut file, mut block) = (None, None, None, None);
    let mut cluster: Option<(String, Vec<String>)> = None;
    let mut pos = after_cols;
    while pos < toks.len() {
        if kw(at(pos), "OBJNO") {
            once(&mut objno, uint(at(pos + 1)).ok_or("OBJNO without number")?)?;
            pos += 2;
        } else if kw(at(pos), "TABNO") {
            once(&mut tabno, uint(at(pos + 1)).ok_or("TABNO without number")?)?;
            pos += 2;
        } else if kw(at(pos), "EXTENTS") {
            // EXTENTS ( FILE f BLOCK b )
            if at(pos + 1) != Some(&Tok::Punct('('))
                || !kw(at(pos + 2), "FILE")
                || !kw(at(pos + 4), "BLOCK")
                || at(pos + 6) != Some(&Tok::Punct(')'))
            {
                return Err("malformed EXTENTS clause");
            }
            once(
                &mut file,
                uint(at(pos + 3)).ok_or("EXTENTS FILE not a number")?,
            )?;
            once(
                &mut block,
                uint(at(pos + 5)).ok_or("EXTENTS BLOCK not a number")?,
            )?;
            pos += 7;
        } else if kind == ObjectKind::Table && kw(at(pos), "CLUSTER") {
            // CLUSTER name ( col [, col]* )
            if cluster.is_some() {
                return Err("two CLUSTER clauses");
            }
            let Some(Tok::Word(cname)) = at(pos + 1) else {
                return Err("CLUSTER without name");
            };
            if at(pos + 2) != Some(&Tok::Punct('(')) {
                return Err("CLUSTER without column list");
            }
            let mut cols = Vec::new();
            let mut cur = pos + 3;
            loop {
                match at(cur) {
                    Some(Tok::Word(word)) => cols.push(word.to_ascii_uppercase()),
                    Some(Tok::Quoted(quoted)) => cols.push((*quoted).to_owned()),
                    _ => return Err("bad CLUSTER column list"),
                }
                match at(cur + 1) {
                    Some(Tok::Punct(',')) => cur += 2,
                    Some(Tok::Punct(')')) => break,
                    _ => return Err("bad CLUSTER column list"),
                }
            }
            cluster = Some((cname.to_ascii_uppercase(), cols));
            pos = cur + 2;
        } else {
            pos += 1;
        }
    }

    let obj = objno.ok_or("no OBJNO")?;
    let storage = storage_of(cluster, tabno, file, block)?;
    Ok(BootObject {
        obj,
        kind,
        name,
        storage,
        line: 0,
        columns,
    })
}

/// Parse `col type[(n[,m])] [NOT NULL] [DEFAULT expr] {, …} )` starting at
/// token `start` (just after the opening parenthesis). Returns the columns
/// and the index of the token after the closing parenthesis. Everything
/// between a column's type and the next top-level comma is skipped with
/// parenthesis depth tracking, so `DEFAULT` expressions and constraints
/// cannot derail the parse.
fn parse_columns(toks: &[Tok<'_>], start: usize) -> Result<(Vec<BootColumn>, usize), &'static str> {
    let at = |k: usize| toks.get(k);
    let mut cols = Vec::new();
    let mut pos = start;
    loop {
        let name = match at(pos) {
            Some(Tok::Word(w)) => w.to_ascii_uppercase(),
            Some(Tok::Quoted(q)) => (*q).to_owned(),
            _ => return Err("bad column list: expected a column name"),
        };
        if name.is_empty() {
            // `""` tokenises as an empty quoted identifier (fuzz finding
            // 2026-10-18); Oracle never writes one.
            return Err("bad column list: empty column name");
        }
        // Table-level constraints are not columns; bootstrap DDL has none,
        // so a keyword here is a parse failure rather than something to skip.
        if matches!(
            name.as_str(),
            "CONSTRAINT" | "PRIMARY" | "UNIQUE" | "FOREIGN" | "CHECK"
        ) {
            return Err("table-level constraint in bootstrap column list");
        }
        let Some(Tok::Word(ty)) = at(pos + 1) else {
            return Err("bad column list: expected a type");
        };
        let mut type_name = ty.to_ascii_uppercase();
        pos += 2;
        // Multi-word types: `LONG RAW`, `TIMESTAMP WITH [LOCAL] TIME ZONE`,
        // `INTERVAL YEAR …`, `DOUBLE PRECISION`, `CHARACTER VARYING`.
        if type_name == "LONG" && kw(at(pos), "RAW") {
            type_name = "LONG RAW".into();
            pos += 1;
        }
        let mut length = None;
        if at(pos) == Some(&Tok::Punct('(')) {
            length = uint(at(pos + 1));
            // skip to matching ')'
            let mut depth = 0usize;
            while let Some(t) = at(pos) {
                match t {
                    Tok::Punct('(') => depth += 1,
                    Tok::Punct(')') => {
                        depth -= 1;
                        if depth == 0 {
                            pos += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                pos += 1;
            }
            if depth != 0 {
                return Err("bad column list: unbalanced type parentheses");
            }
        }
        if type_name == "TIMESTAMP" && kw(at(pos), "WITH") {
            // consume WITH [LOCAL] TIME ZONE
            let local = kw(at(pos + 1), "LOCAL");
            let base = if local { pos + 2 } else { pos + 1 };
            if !(kw(at(base), "TIME") && kw(at(base + 1), "ZONE")) {
                return Err("bad column list: malformed TIMESTAMP WITH TIME ZONE");
            }
            type_name = if local {
                "TIMESTAMP WITH LOCAL TIME ZONE".into()
            } else {
                "TIMESTAMP WITH TIME ZONE".into()
            };
            pos = base + 2;
        }
        // Column attributes until the next top-level ',' or the closing ')'.
        let mut not_null = false;
        let mut depth = 0usize;
        loop {
            match at(pos) {
                None => return Err("bad column list: unterminated"),
                Some(Tok::Punct('(')) => depth += 1,
                Some(Tok::Punct(')')) => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                Some(Tok::Punct(',')) if depth == 0 => break,
                Some(Tok::Word(w))
                    if depth == 0 && w.eq_ignore_ascii_case("NOT") && kw(at(pos + 1), "NULL") =>
                {
                    not_null = true;
                    pos += 1;
                }
                _ => {}
            }
            pos += 1;
        }
        cols.push(BootColumn {
            name,
            type_name,
            length,
            not_null,
        });
        match at(pos) {
            Some(Tok::Punct(',')) => pos += 1,
            Some(Tok::Punct(')')) => return Ok((cols, pos + 1)),
            _ => return Err("bad column list: expected ',' or ')'"),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    // Verbatim published bootstrap$ texts (see module docs for sources).
    const OBJ: &str = r#"CREATE TABLE OBJ$("OBJ#" NUMBER NOT NULL,"DATAOBJ#" NUMBER,"OWNER#" NUMBER NOT NULL,"NAME" VARCHAR2(30) NOT NULL,"NAMESPACE" NUMBER NOT NULL,"SUBNAME" VARCHAR2(30),"TYPE#" NUMBER NOT NULL,"CTIME" DATE NOT NULL,"MTIME" DATE NOT NULL,"STIME" DATE NOT NULL,"STATUS" NUMBER NOT NULL,"REMOTEOWNER" VARCHAR2(30),"LINKNAME" VARCHAR2(128),"FLAGS" NUMBER,"OID$" RAW(16),"SPARE1" NUMBER,"SPARE2" NUMBER,"SPARE3" NUMBER,"SPARE4" VARCHAR2(1000),"SPARE5" VARCHAR2(1000),"SPARE6" DATE) PCTFREE 10 PCTUSED 40 INITRANS 1 MAXTRANS 255 STORAGE (  INITIAL 16K NEXT 1024K MINEXTENTS 1 MAXEXTENTS 2147483645 PCTINCREASE 0 OBJNO 18 EXTENTS (FILE 1 BLOCK 121))"#;
    const C_OBJ: &str = r#"CREATE CLUSTER C_OBJ#("OBJ#" NUMBER) PCTFREE 5 PCTUSED 40 INITRANS 2 MAXTRANS 255 STORAGE (  INITIAL 136K NEXT 200K MINEXTENTS 1 MAXEXTENTS 2147483645 PCTINCREASE 0 OBJNO 2 EXTENTS (FILE 1 BLOCK 144)) SIZE 800"#;
    const TAB: &str = r#"CREATE TABLE TAB$("OBJ#" NUMBER NOT NULL,"DATAOBJ#" NUMBER,"TS#" NUMBER NOT NULL,"FILE#" NUMBER NOT NULL,"BLOCK#" NUMBER NOT NULL,"BOBJ#" NUMBER,"TAB#" NUMBER,"COLS" NUMBER NOT NULL,"CLUCOLS" NUMBER,"PCTFREE$" NUMBER NOT NULL,"PCTUSED$" NUMBER NOT NULL,"INITRANS" NUMBER NOT NULL,"MAXTRANS" NUMBER NOT NULL,"FLAGS" NUMBER NOT NULL,"AUDIT$" VARCHAR2(38) NOT NULL,"ROWCNT" NUMBER,"BLKCNT" NUMBER,"EMPCNT" NUMBER,"AVGSPC" NUMBER,"CHNCNT" NUMBER,"AVGRLN" NUMBER,"AVGSPC_FLB" NUMBER,"FLBCNT" NUMBER,"ANALYZETIME" DATE,"SAMPLESIZE" NUMBER,"DEGREE" NUMBER,"INSTANCES" NUMBER,"INTCOLS" NUMBER NOT NULL,"KERNELCOLS" NUMBER NOT NULL,"PROPERTY" NUMBER NOT NULL,"TRIGFLAG" NUMBER,"SPARE1" NUMBER,"SPARE2" NUMBER,"SPARE3" NUMBER,"SPARE4" VARCHAR2(1000),"SPARE5" VARCHAR2(1000),"SPARE6" DATE) STORAGE (  OBJNO 4 TABNO 1) CLUSTER C_OBJ#(OBJ#)"#;
    const RBS: &str = "CREATE ROLLBACK SEGMENT SYSTEM STORAGE (  INITIAL 112K NEXT 1024K MINEXTENTS 1 MAXEXTENTS 32765 OBJNO 0 EXTENTS (FILE 1 BLOCK 9))";

    #[test]
    fn published_statements() {
        let o = parse_ddl(OBJ).unwrap();
        assert_eq!(
            (o.obj, o.kind, o.name.as_str()),
            (18, ObjectKind::Table, "OBJ$")
        );
        assert_eq!(
            o.storage,
            Storage::Segment {
                file: 1,
                block: 121
            }
        );
        let c = parse_ddl(C_OBJ).unwrap();
        assert_eq!((c.obj, c.kind), (2, ObjectKind::Cluster));
        assert_eq!(
            c.storage,
            Storage::Segment {
                file: 1,
                block: 144
            }
        );
        let t = parse_ddl(TAB).unwrap();
        assert_eq!(t.obj, 4);
        assert_eq!(
            t.storage,
            Storage::Clustered {
                cluster: "C_OBJ#".into(),
                tabno: 1,
                key_columns: vec!["OBJ#".into()],
            }
        );
        let r = parse_ddl(RBS).unwrap();
        assert_eq!(
            (r.obj, r.kind, r.name.as_str()),
            (0, ObjectKind::RollbackSegment, "SYSTEM")
        );
        assert_eq!(r.storage, Storage::Segment { file: 1, block: 9 });
        assert_eq!(r.columns, Vec::new());

        // Column lists (LV-038): OBJ$ has 21 columns in 11g, TAB$ 37.
        let names: Vec<&str> = o.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names.len(), 21);
        assert_eq!(&names[..4], &["OBJ#", "DATAOBJ#", "OWNER#", "NAME"]);
        assert_eq!(names[14], "OID$");
        assert_eq!(names[20], "SPARE6");
        assert_eq!(o.columns[3].type_name, "VARCHAR2");
        assert_eq!(o.columns[3].length, Some(30));
        assert!(o.columns[3].not_null);
        assert!(!o.columns[1].not_null, "DATAOBJ# is nullable");
        assert_eq!(o.columns[14].type_name, "RAW");
        assert_eq!(o.columns[14].length, Some(16));
        assert_eq!(o.columns[20].type_name, "DATE");
        assert_eq!(
            o.columns[3].column_type(),
            Some(crate::value::ColumnType::Varchar2 { national: false })
        );
        assert_eq!(t.columns.len(), 37);
        assert_eq!(t.columns[7].name, "COLS");
        assert_eq!(t.columns[8].name, "CLUCOLS");
        assert_eq!(c.columns.len(), 1);
        assert_eq!(c.columns[0].name, "OBJ#");
    }

    #[test]
    fn column_list_handles_defaults_long_and_multiword_types() {
        let s = r#"CREATE TABLE X$("A" NUMBER DEFAULT 0 NOT NULL,"B" LONG,"C" LONG RAW,"D" TIMESTAMP(6) WITH LOCAL TIME ZONE,"E" VARCHAR2(10) DEFAULT (NVL(NULL,'x,y')) ,"F" NUMBER(10,2)) STORAGE ( OBJNO 7 EXTENTS (FILE 1 BLOCK 50))"#;
        let o = parse_ddl(s).unwrap();
        let v: Vec<(&str, &str, Option<u32>, bool)> = o
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.type_name.as_str(), c.length, c.not_null))
            .collect();
        assert_eq!(
            v,
            vec![
                ("A", "NUMBER", None, true),
                ("B", "LONG", None, false),
                ("C", "LONG RAW", None, false),
                ("D", "TIMESTAMP WITH LOCAL TIME ZONE", Some(6), false),
                ("E", "VARCHAR2", Some(10), false),
                ("F", "NUMBER", Some(10), false),
            ]
        );
        assert_eq!(o.obj, 7);
        for bad in [
            r#"CREATE TABLE X$("A" NUMBER, STORAGE ( OBJNO 7 EXTENTS (FILE 1 BLOCK 50))"#,
            r#"CREATE TABLE X$("A") STORAGE ( OBJNO 7 EXTENTS (FILE 1 BLOCK 50))"#,
            r#"CREATE TABLE X$("A" NUMBER(10) STORAGE ( OBJNO 7 EXTENTS (FILE 1 BLOCK 50))"#,
        ] {
            assert!(parse_ddl(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn quoted_identifiers_never_match_keywords() {
        let s = r#"CREATE TABLE X$("OBJNO" NUMBER,"EXTENTS" NUMBER,"TABNO" NUMBER) STORAGE ( OBJNO 7 EXTENTS (FILE 1 BLOCK 50))"#;
        let o = parse_ddl(s).unwrap();
        assert_eq!(o.obj, 7);
        assert_eq!(o.storage, Storage::Segment { file: 1, block: 50 });
    }

    #[test]
    fn contradictions_and_gaps_are_rejected() {
        for bad in [
            "CREATE TABLE A$(X NUMBER) STORAGE ( OBJNO 7 OBJNO 8 EXTENTS (FILE 1 BLOCK 5))",
            "CREATE TABLE A$(X NUMBER) STORAGE ( EXTENTS (FILE 1 BLOCK 5))",
            "CREATE TABLE A$(X NUMBER) STORAGE ( OBJNO 7)",
            "CREATE TABLE A$(X NUMBER) STORAGE ( OBJNO 7) CLUSTER C_OBJ#(OBJ#)",
            "CREATE TABLE A$(X NUMBER) STORAGE ( OBJNO 7 TABNO 2)",
            "CREATE TABLE A$(X NUMBER) STORAGE ( OBJNO 7 TABNO 2 EXTENTS (FILE 1 BLOCK 5)) CLUSTER C(X)",
            "CREATE TABLE A$(X NUMBER) STORAGE ( OBJNO 7 EXTENTS (FILE 1 5))",
            "CREATE TABLE A$(X NUMBER) STORAGE ( OBJNO -7 EXTENTS (FILE 1 BLOCK 5))",
            "CREATE TABLE A$(\"X NUMBER) STORAGE ( OBJNO 7 EXTENTS (FILE 1 BLOCK 5))",
            "CREATE VIEW V AS SELECT 1 FROM DUAL",
            "8.0.0.0.0",
            "",
        ] {
            assert!(parse_ddl(bad).is_err(), "accepted: {bad}");
        }
        // Repeating the same value is harmless.
        assert!(
            parse_ddl(
                "CREATE TABLE A$(X NUMBER) STORAGE ( OBJNO 7 OBJNO 7 EXTENTS (FILE 1 BLOCK 5))"
            )
            .is_ok()
        );
    }

    #[test]
    fn tokenizer_is_total_on_unicode() {
        assert!(
            parse_ddl(
                "CREATE TABLE جدول(\"عمود\" NUMBER) STORAGE ( OBJNO 9 EXTENTS (FILE 1 BLOCK 2))"
            )
            .is_err()
        );
        let _ = tokenize("\u{0}\u{FFFD}\"");
    }
}
