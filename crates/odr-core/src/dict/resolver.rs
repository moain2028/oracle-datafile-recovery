//! The dictionary resolver: from a SYSTEM datafile to [`TableDef`]s.
//!
//! Chain (plan 2026-10-18 §5 step f):
//!
//! ```text
//! kcvfh (block 1) ──root dba──▶ bootstrap$ segment ──rows──▶ Catalog (DDL text)
//!   └─ column lists + storage of OBJ$ / TAB$ / COL$ / USER$ / TS$ / C_OBJ# …
//! OBJ$  (heap)                ──▶ obj# → (name, owner#, dataobj#, type#)
//! TAB$  (C_OBJ#, tabno 1)     ──▶ obj# → (ts#, file#, block#, bobj#, tab#, clucols, flags, property)
//! COL$  (C_OBJ#, tabno 5)     ──▶ obj# → columns (col#, segcol#, intcol#, name, type#, …, default$)
//! USER$ (C_USER#, tabno 1)    ──▶ user# → name
//! TS$   (C_TS#)               ──▶ ts# → name
//! ```
//!
//! Every column of every dictionary table is located **by name in the
//! bootstrap$ DDL** of the very file being read (LV-045: 26ai OBJ$ has 37
//! columns, 11g has 21 — fixed positions would silently mis-read one of
//! them). Nothing is assumed about the Oracle version.
//!
//! Golden rule: a row whose required columns do not decode is reported in
//! [`Dictionary::issues`] and dropped — never patched with a plausible value.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::bootstrap::{BootObject, Catalog, ObjectKind, Storage};
use super::kcvfh::FileHeader;
use super::rows::{RawRow, SegmentRows, read_cluster, read_heap};
use super::schema::{ColumnDef, TableDef};
use super::segment::walk_segment;
use super::source::{BlockAddr, BlockSource};
use crate::block::Rdba;
use crate::charset::{self, Charset};
use crate::error::{DecodeError, DecodeResult};
use crate::row::Provenance;
use crate::types::OraNumber;

/// Why a dictionary row or object was not used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum Issue {
    /// bootstrap$ does not define this object (name).
    MissingBootstrapObject(String),
    /// The bootstrap DDL of `table` lacks a column the resolver needs.
    MissingBootstrapColumn {
        /// Dictionary table.
        table: String,
        /// Column name.
        column: String,
    },
    /// A segment could not be walked.
    Segment {
        /// Dictionary table.
        table: String,
        /// Error.
        error: DecodeError,
    },
    /// A row of `table` was dropped: a required column did not decode.
    Row {
        /// Dictionary table.
        table: String,
        /// Row origin.
        provenance: Provenance,
        /// What was wrong.
        reason: String,
    },
    /// Two rows claim the same key (e.g. two OBJ$ rows for one obj#).
    Duplicate {
        /// Dictionary table.
        table: String,
        /// Key.
        key: u64,
    },
    /// Blocks of a dictionary segment were skipped (count by reason).
    Skipped {
        /// Dictionary table.
        table: String,
        /// Human-readable reason.
        reason: String,
        /// Number of blocks.
        blocks: usize,
    },
}

/// One `OBJ$` row (the columns ODR needs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjRow {
    /// `OBJ#`.
    pub obj: u32,
    /// `DATAOBJ#`.
    pub dataobj: Option<u32>,
    /// `OWNER#`.
    pub owner: u32,
    /// `NAME`.
    pub name: String,
    /// `TYPE#` (2 = table, 3 = cluster, 1 = index, …).
    pub type_code: u32,
}

/// One `TAB$` row (the columns ODR needs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabRow {
    /// `OBJ#`.
    pub obj: u32,
    /// `DATAOBJ#`.
    pub dataobj: Option<u32>,
    /// `TS#`.
    pub ts: u32,
    /// `FILE#`.
    pub file: u32,
    /// `BLOCK#`.
    pub block: u32,
    /// `BOBJ#`.
    pub bobj: Option<u32>,
    /// `TAB#`.
    pub tab: Option<u32>,
    /// `CLUCOLS`.
    pub clucols: Option<u32>,
    /// `FLAGS`.
    pub flags: u64,
    /// `PROPERTY`.
    pub property: u64,
}

/// One `CLU$` row (the columns ODR needs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CluRow {
    /// `OBJ#` of the cluster.
    pub obj: u32,
    /// `DATAOBJ#`.
    pub dataobj: Option<u32>,
    /// `TS#`.
    pub ts: u32,
    /// `FILE#` (relative within `TS#`).
    pub file: u32,
    /// `BLOCK#` of the segment header.
    pub block: u32,
    /// `COLS` — number of cluster key columns.
    pub cols: u32,
    /// `HASHKEYS` — 0 for an indexed cluster, > 0 for a hash cluster.
    pub hashkeys: u32,
}

/// The resolved dictionary of one SYSTEM datafile.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Dictionary {
    /// Header of the SYSTEM file.
    pub header: Option<FileHeader>,
    /// bootstrap$ version marker and object map.
    pub catalog: Catalog,
    /// `OBJ$` by obj#.
    pub objects: BTreeMap<u32, ObjRow>,
    /// `TAB$` by obj#.
    pub tabs: BTreeMap<u32, TabRow>,
    /// `CLU$` by obj# (cluster segments; members point here via `TAB$.BOBJ#`).
    pub clusters: BTreeMap<u32, CluRow>,
    /// `COL$` by obj#, each in `intcol#` order.
    pub cols: BTreeMap<u32, Vec<ColumnDef>>,
    /// `USER$`: user# → name.
    pub users: BTreeMap<u32, String>,
    /// `TS$`: ts# → name.
    pub tablespaces: BTreeMap<u32, String>,
    /// Everything that was dropped or could not be read, with the reason.
    pub issues: Vec<Issue>,
}

/// Database character set used for dictionary names. Dictionary identifiers
/// are stored in the database character set; names of the core objects are
/// ASCII, so any ASCII-compatible charset decodes them identically. The
/// caller may re-decode `NAME` columns with the real charset later.
const DICT_CS: Charset = Charset::Al32Utf8;

impl Dictionary {
    /// Resolve the dictionary from the SYSTEM datafile `system_file` in `src`.
    ///
    /// # Errors
    /// Only fatal errors (no readable `kcvfh`, no root dba, unreadable
    /// bootstrap$ header). Everything else degrades into [`Self::issues`].
    pub fn load<S: BlockSource + ?Sized>(src: &S, system_file: u32) -> DecodeResult<Self> {
        let order = src.order();
        let fatal = |what: &'static str| DecodeError::malformed("dictionary", 0, what);
        let hdr_block = src
            .block(BlockAddr {
                file: system_file,
                block: 1,
            })
            .ok_or_else(|| fatal("SYSTEM block 1 (kcvfh) unavailable"))?;
        let header = FileHeader::parse(&hdr_block, order)?;
        if !header.has_dictionary_root() {
            return Err(fatal("kcvfhrdb is null: not a SYSTEM datafile"));
        }
        // LV-053: file# is not 1 inside a PDB; SYSTEM is proven below by the
        // bootstrap$ rows actually decoding, not by any header field.
        let mut dict = Self {
            header: Some(header.clone()),
            ..Self::default()
        };

        // --- bootstrap$ ---------------------------------------------------
        let root = header.root_dba;
        let Some(root_addr) = src.resolve(root, system_file) else {
            return Err(fatal("root dba does not resolve inside the SYSTEM file"));
        };
        let root_block = src
            .block(root_addr)
            .ok_or_else(|| fatal("bootstrap$ segment header unavailable"))?;
        let seg = walk_segment(&root_block, order, root, |r| {
            src.resolve(r, system_file).and_then(|a| src.block(a))
        })?;
        let boot_rows = read_heap(src, &seg, system_file, seg.objd);
        dict.note_skipped("BOOTSTRAP$", &boot_rows);
        // Catalog::from_rows wants RowPieces; RawRow already holds the
        // assembled column bytes, so feed it a thin adapter.
        let pieces = boot_rows
            .rows
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|r| crate::row::RowPiece {
                slot: r.provenance.slot,
                flag: 0x2C,
                lock: 0,
                columns: r.columns.iter().map(|c| c.as_deref()).collect(),
                next: None,
                head: None,
                dscn: None,
                cluster: None,
                len: 0,
            });
        dict.catalog = Catalog::from_rows(pieces, DICT_CS);
        if dict.catalog.objects.is_empty() {
            return Err(fatal(
                "root segment holds no bootstrap$ rows: not a SYSTEM datafile",
            ));
        }

        dict.load_obj(src, system_file);
        dict.load_tab(src, system_file);
        dict.load_clu(src, system_file);
        dict.load_col(src, system_file);
        dict.load_names(src, system_file, "USER$", &["USER#", "NAME"], Which::Users);
        dict.load_names(
            src,
            system_file,
            "TS$",
            &["TS#", "NAME"],
            Which::Tablespaces,
        );
        Ok(dict)
    }

    fn load_obj<S: BlockSource + ?Sized>(&mut self, src: &S, system_file: u32) {
        let Some(rows) = self.read_object(src, system_file, "OBJ$") else {
            return;
        };
        let Some(ix) = self.columns_of("OBJ$", &["OBJ#", "DATAOBJ#", "OWNER#", "NAME", "TYPE#"])
        else {
            return;
        };
        for r in rows.rows.iter().filter_map(|r| r.as_ref().ok()) {
            let get = |k: usize| field(r, &ix, k);
            let (Some(obj), Some(owner), Some(type_code)) =
                (number_u32(get(0)), number_u32(get(2)), number_u32(get(4)))
            else {
                self.drop_row("OBJ$", r, "OBJ#/OWNER#/TYPE# not integers");
                continue;
            };
            let Some(name) = text(get(3)) else {
                self.drop_row("OBJ$", r, "NAME missing or undecodable");
                continue;
            };
            let row = ObjRow {
                obj,
                dataobj: number_u32(get(1)),
                owner,
                name,
                type_code,
            };
            if self.objects.insert(obj, row).is_some() {
                self.duplicate("OBJ$", u64::from(obj));
            }
        }
    }

    fn load_tab<S: BlockSource + ?Sized>(&mut self, src: &S, system_file: u32) {
        let Some(rows) = self.read_object(src, system_file, "TAB$") else {
            return;
        };
        let names = [
            "OBJ#", "DATAOBJ#", "TS#", "FILE#", "BLOCK#", "BOBJ#", "TAB#", "CLUCOLS", "FLAGS",
            "PROPERTY",
        ];
        let Some(ix) = self.columns_of("TAB$", &names) else {
            return;
        };
        for r in rows.rows.iter().filter_map(|r| r.as_ref().ok()) {
            let get = |k: usize| field(r, &ix, k);
            let (Some(obj), Some(ts), Some(file), Some(block)) = (
                number_u32(get(0)),
                number_u32(get(2)),
                number_u32(get(3)),
                number_u32(get(4)),
            ) else {
                self.drop_row("TAB$", r, "OBJ#/TS#/FILE#/BLOCK# not integers");
                continue;
            };
            let row = TabRow {
                obj,
                dataobj: number_u32(get(1)),
                ts,
                file,
                block,
                bobj: number_u32(get(5)),
                tab: number_u32(get(6)),
                clucols: number_u32(get(7)),
                flags: number_u64(get(8)).unwrap_or(0),
                property: number_u64(get(9)).unwrap_or(0),
            };
            if self.tabs.insert(obj, row).is_some() {
                self.duplicate("TAB$", u64::from(obj));
            }
        }
    }

    fn load_clu<S: BlockSource + ?Sized>(&mut self, src: &S, system_file: u32) {
        let Some(rows) = self.read_object(src, system_file, "CLU$") else {
            return;
        };
        let names = [
            "OBJ#", "DATAOBJ#", "TS#", "FILE#", "BLOCK#", "COLS", "HASHKEYS",
        ];
        let Some(ix) = self.columns_of("CLU$", &names) else {
            return;
        };
        for r in rows.rows.iter().filter_map(|r| r.as_ref().ok()) {
            let get = |k: usize| field(r, &ix, k);
            let (Some(obj), Some(ts), Some(file), Some(block), Some(cols)) = (
                number_u32(get(0)),
                number_u32(get(2)),
                number_u32(get(3)),
                number_u32(get(4)),
                number_u32(get(5)),
            ) else {
                self.drop_row("CLU$", r, "OBJ#/TS#/FILE#/BLOCK#/COLS not integers");
                continue;
            };
            let row = CluRow {
                obj,
                dataobj: number_u32(get(1)),
                ts,
                file,
                block,
                cols,
                hashkeys: number_u32(get(6)).unwrap_or(0),
            };
            if self.clusters.insert(obj, row).is_some() {
                self.duplicate("CLU$", u64::from(obj));
            }
        }
    }

    fn load_col<S: BlockSource + ?Sized>(&mut self, src: &S, system_file: u32) {
        let Some(rows) = self.read_object(src, system_file, "COL$") else {
            return;
        };
        let names = [
            "OBJ#",
            "COL#",
            "SEGCOL#",
            "NAME",
            "TYPE#",
            "LENGTH",
            "PRECISION#",
            "SCALE",
            "NULL$",
            "DEFAULT$",
            "INTCOL#",
            "PROPERTY",
            "CHARSETFORM",
        ];
        let Some(ix) = self.columns_of("COL$", &names) else {
            return;
        };
        for r in rows.rows.iter().filter_map(|r| r.as_ref().ok()) {
            let get = |k: usize| field(r, &ix, k);
            let (Some(obj), Some(col), Some(segcol), Some(intcol), Some(type_code)) = (
                number_u32(get(0)),
                number_u32(get(1)),
                number_u32(get(2)),
                number_u32(get(10)),
                number_u32(get(4)),
            ) else {
                self.drop_row("COL$", r, "OBJ#/COL#/SEGCOL#/INTCOL#/TYPE# not integers");
                continue;
            };
            let Some(name) = text(get(3)) else {
                self.drop_row("COL$", r, "NAME missing or undecodable");
                continue;
            };
            let Ok(type_code) = u16::try_from(type_code) else {
                self.drop_row("COL$", r, "TYPE# out of range");
                continue;
            };
            let def = ColumnDef {
                col,
                segcol,
                intcol,
                name,
                type_code,
                length: number_u32(get(5)).unwrap_or(0),
                precision: number_i32(get(6)),
                scale: number_i32(get(7)),
                not_null: number_u32(get(8)).is_some_and(|n| n > 0),
                charset_form: u8::try_from(number_u32(get(12)).unwrap_or(1)).unwrap_or(1),
                property: number_u64(get(11)).unwrap_or(0),
                // DEFAULT$ is a LONG: charset text inside the row piece.
                default_text: text(get(9)).filter(|t| !t.trim().is_empty()),
            };
            self.cols.entry(obj).or_default().push(def);
        }
        for v in self.cols.values_mut() {
            v.sort_by_key(|c| c.intcol);
        }
    }

    /// `USER$` / `TS$`: number → name maps.
    fn load_names<S: BlockSource + ?Sized>(
        &mut self,
        src: &S,
        system_file: u32,
        table: &str,
        names: &[&str],
        which: Which,
    ) {
        let Some(rows) = self.read_object(src, system_file, table) else {
            return;
        };
        let Some(ix) = self.columns_of(table, names) else {
            return;
        };
        for r in rows.rows.iter().filter_map(|r| r.as_ref().ok()) {
            let get = |k: usize| field(r, &ix, k);
            match (number_u32(get(0)), text(get(1))) {
                (Some(k), Some(n)) => {
                    let map = match which {
                        Which::Users => &mut self.users,
                        Which::Tablespaces => &mut self.tablespaces,
                    };
                    if map.insert(k, n).is_some() {
                        self.duplicate(table, u64::from(k));
                    }
                }
                _ => self.drop_row(table, r, "key/NAME undecodable"),
            }
        }
    }

    fn duplicate(&mut self, table: &str, key: u64) {
        self.issues.push(Issue::Duplicate {
            table: table.into(),
            key,
        });
    }

    /// All tables (`OBJ$.TYPE# = 2`) that have a `TAB$` row, as [`TableDef`]s
    /// with their `COL$` columns, owner and tablespace names resolved.
    /// Tables without `COL$` rows are returned with an empty column list.
    pub fn tables(&self) -> Vec<TableDef> {
        let mut out = Vec::new();
        for (obj, t) in &self.tabs {
            let Some(o) = self.objects.get(obj) else {
                continue;
            };
            if o.type_code != 2 {
                continue;
            }
            out.push(TableDef {
                obj: *obj,
                dataobj: o.dataobj.or(t.dataobj),
                owner: o.owner,
                owner_name: self.users.get(&o.owner).cloned(),
                name: o.name.clone(),
                ts: t.ts,
                file: t.file,
                block: t.block,
                bobj: t.bobj,
                tab: t.tab,
                clucols: t.clucols,
                flags: t.flags,
                property: t.property,
                columns: self.cols.get(obj).cloned().unwrap_or_default(),
            });
        }
        out
    }

    /// Find a table by `OWNER.NAME` (exact, upper-case).
    pub fn table(&self, owner: &str, name: &str) -> Option<TableDef> {
        self.tables()
            .into_iter()
            .find(|t| t.name == name && t.owner_name.as_deref() == Some(owner))
    }

    /// The cluster segment a clustered table lives in (`TAB$.BOBJ#` → `CLU$`).
    pub fn cluster_of(&self, table: &TableDef) -> Option<&CluRow> {
        self.clusters.get(&table.bobj?)
    }

    /// Tablespace name of `ts#`.
    pub fn tablespace_name(&self, ts: u32) -> Option<&str> {
        self.tablespaces.get(&ts).map(String::as_str)
    }

    // ----------------------------------------------------------------------

    /// Indices (in the stored row) of `names` inside the bootstrap DDL column
    /// list of `table`. Bootstrap tables have no hidden/virtual columns, so
    /// DDL order = `segcol#` order (LV-045) — with one exception:
    ///
    /// **LV-054**: a `LONG` column's bytes are stored **after all other
    /// columns**: its DDL slot keeps a NULL (so no other position shifts) and
    /// the value occupies index `ddl_len − 1`… but only in rows that have a
    /// value — the DDL list already counts the LONG once, so a row with a
    /// LONG stores exactly `ddl_len` columns (26ai COL$: 37 DDL columns;
    /// `T_ADDCOL.NN` row has cc = 37, slot 13 NULL, `'7 '` at index 36; rows
    /// without a default have cc ≤ 33 after trailing-NULL trimming). The
    /// marker [`LONG_LAST`] is resolved per row by [`field`] with that
    /// exact-length rule, so a shorter row can never hand back a `SPARE`
    /// column as the default.
    ///
    /// Cluster members are read with their key columns **prepended**, and
    /// `sql.bsq` writes every core cluster table with the key columns first
    /// (`OBJ#` in TAB$/COL$, `USER#` in USER$, `TS#` in TS$) — checked here,
    /// not assumed.
    fn columns_of(&mut self, table: &str, names: &[&str]) -> Option<ColumnIndex> {
        let Some(obj) = self.catalog.by_name(table) else {
            self.issues
                .push(Issue::MissingBootstrapObject(table.into()));
            return None;
        };
        if let Storage::Clustered { key_columns, .. } = &obj.storage {
            for (i, k) in key_columns.iter().enumerate() {
                if obj.columns.get(i).map(|c| c.name.as_str()) != Some(k.as_str()) {
                    self.issues.push(Issue::MissingBootstrapColumn {
                        table: table.into(),
                        column: format!("{k} (cluster key not leading in DDL)"),
                    });
                    return None;
                }
            }
        }
        let is_long = |n: &str| n == "LONG" || n == "LONG RAW";
        let long_pos = obj.columns.iter().position(|c| is_long(&c.type_name));
        let stored_index = |ddl: usize| -> usize {
            match long_pos {
                Some(lp) if ddl == lp => LONG_LAST,
                _ => ddl,
            }
        };
        let mut out = Vec::with_capacity(names.len());
        for n in names {
            let Some(i) = obj.columns.iter().position(|c| c.name == *n) else {
                self.issues.push(Issue::MissingBootstrapColumn {
                    table: table.into(),
                    column: (*n).into(),
                });
                return None;
            };
            out.push(stored_index(i));
        }
        Some(ColumnIndex {
            ix: out,
            ddl_len: obj.columns.len(),
        })
    }

    /// Read all rows of bootstrap object `name` (heap or cluster member).
    fn read_object<S: BlockSource + ?Sized>(
        &mut self,
        src: &S,
        system_file: u32,
        name: &str,
    ) -> Option<SegmentRows> {
        let order = src.order();
        let Some(obj) = self.catalog.by_name(name).cloned() else {
            self.issues.push(Issue::MissingBootstrapObject(name.into()));
            return None;
        };
        let (seg_obj, cluster): (BootObject, Option<(usize, usize)>) = match &obj.storage {
            Storage::Segment { .. } => (obj.clone(), None),
            Storage::Clustered {
                cluster,
                tabno,
                key_columns,
            } => {
                let Some(c) = self.catalog.by_name(cluster).cloned() else {
                    self.issues
                        .push(Issue::MissingBootstrapObject(cluster.clone()));
                    return None;
                };
                let Ok(tab) = usize::try_from(*tabno) else {
                    return None;
                };
                (c, Some((tab, key_columns.len())))
            }
        };
        let Storage::Segment { file, block } = &seg_obj.storage else {
            self.issues.push(Issue::Segment {
                table: name.into(),
                error: DecodeError::malformed("bootstrap$", 0, "cluster without own segment"),
            });
            return None;
        };
        // LV-042: bootstrap$ writes FILE 1 (smallfile) or FILE 1024 (bigfile)
        // for the SYSTEM file itself; both mean "this file".
        let file = if *file == 1 || *file == 1024 {
            system_file
        } else {
            match src.absolute_file(Some(*file), system_file) {
                Some(f) => f,
                None => system_file,
            }
        };
        let Some(hdr) = src.block(BlockAddr {
            file,
            block: *block,
        }) else {
            self.issues.push(Issue::Segment {
                table: name.into(),
                error: DecodeError::malformed("segment", 0, "header block unavailable"),
            });
            return None;
        };
        let seg = match walk_segment(&hdr, order, Rdba(0), |r| {
            src.resolve(r, file).and_then(|a| src.block(a))
        }) {
            Ok(s) => s,
            Err(e) => {
                self.issues.push(Issue::Segment {
                    table: name.into(),
                    error: e,
                });
                return None;
            }
        };
        let objd = seg.objd;
        let rows = match cluster {
            None => read_heap(src, &seg, file, objd),
            Some((tab, clucols)) => read_cluster(src, &seg, file, objd, tab, clucols),
        };
        self.note_skipped(name, &rows);
        for (prov, e) in rows.rows.iter().filter_map(|r| r.as_ref().err()) {
            self.issues.push(Issue::Row {
                table: name.into(),
                provenance: *prov,
                reason: format!("{e:?}"),
            });
        }
        let _ = ObjectKind::Table; // kind is informational here
        Some(rows)
    }

    fn note_skipped(&mut self, table: &str, rows: &SegmentRows) {
        let mut by_reason: HashMap<String, usize> = HashMap::new();
        for (_, s) in &rows.skipped {
            let key = match s {
                super::rows::Skip::Unavailable => "unavailable".to_owned(),
                super::rows::Skip::NotDataBlock(_) => "not a data block".to_owned(),
                super::rows::Skip::ForeignObject(o) => format!("foreign object {o}"),
                super::rows::Skip::Malformed(e) => format!("malformed: {e}"),
            };
            let n = by_reason.entry(key).or_default();
            *n = n.saturating_add(1);
        }
        for (reason, blocks) in by_reason {
            // Non-data blocks (bitmap/extent-map) are normal inside a segment.
            if reason == "not a data block" {
                continue;
            }
            self.issues.push(Issue::Skipped {
                table: table.into(),
                reason,
                blocks,
            });
        }
    }

    fn drop_row(&mut self, table: &str, r: &RawRow, reason: &str) {
        self.issues.push(Issue::Row {
            table: table.into(),
            provenance: r.provenance,
            reason: reason.into(),
        });
    }
}

/// Which name map `load_names` fills.
#[derive(Clone, Copy)]
enum Which {
    Users,
    Tablespaces,
}

/// Marker index for a `LONG` column (LV-054); resolved per row by [`field`].
const LONG_LAST: usize = usize::MAX;

/// Column indices of `names` plus the DDL column count (for LV-054).
#[derive(Debug, Clone)]
struct ColumnIndex {
    ix: Vec<usize>,
    ddl_len: usize,
}

/// Column `k` of `ix` inside a raw row (`None` if absent / NULL).
fn field<'a>(r: &'a RawRow, ix: &ColumnIndex, k: usize) -> Option<&'a [u8]> {
    let i = *ix.ix.get(k)?;
    if i == LONG_LAST {
        // LV-054: present iff the row stores exactly ddl_len columns.
        if r.columns.len() != ix.ddl_len {
            return None;
        }
        return r
            .columns
            .get(ix.ddl_len.checked_sub(1)?)
            .and_then(|c| c.as_deref());
    }
    r.columns.get(i).and_then(|c| c.as_deref())
}

fn number_i64(raw: Option<&[u8]>) -> Option<i64> {
    OraNumber::decode(raw?).ok()?.to_i64()
}
fn number_u32(raw: Option<&[u8]>) -> Option<u32> {
    u32::try_from(number_i64(raw)?).ok()
}
fn number_u64(raw: Option<&[u8]>) -> Option<u64> {
    u64::try_from(number_i64(raw)?).ok()
}
fn number_i32(raw: Option<&[u8]>) -> Option<i32> {
    i32::try_from(number_i64(raw)?).ok()
}
fn text(raw: Option<&[u8]>) -> Option<String> {
    let d = charset::decode(raw?, DICT_CS);
    (d.invalid == 0).then(|| d.text.into_owned())
}
