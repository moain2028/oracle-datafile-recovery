-- Golden truth export (ADR-0004). Produced by Oracle itself, independent of ODR's
-- decoders. For every corpus table and every column it writes one line per value:
--   TABLE|ID|COLUMN|ROWID|DUMP(col,1016)|canonical text
-- DUMP(...,1016) gives "Typ=.. Len=..: <hex bytes>" plus the charset: the exact
-- bytes stored in the block, which is what ODR must reproduce. The canonical text
-- uses fixed NLS settings so every rebuild produces identical output.
-- Also writes: database/charset/version facts, DBA_SEGMENTS/DBA_EXTENTS (where each
-- table lives, block by block), datafile list, and row counts.
-- Usage (inside the container): sqlplus -s / as sysdba @20_export_golden.sql <PDB> <outdir>
WHENEVER SQLERROR EXIT FAILURE
SET FEEDBACK OFF HEADING OFF PAGESIZE 0 LINESIZE 32767 LONG 2000000 TRIMSPOOL ON TERMOUT OFF VERIFY OFF ECHO OFF
ALTER SESSION SET CONTAINER = &1;
ALTER SESSION SET NLS_DATE_FORMAT = 'SYYYY-MM-DD HH24:MI:SS';
ALTER SESSION SET NLS_TIMESTAMP_FORMAT = 'SYYYY-MM-DD HH24:MI:SS.FF9';
ALTER SESSION SET NLS_TIMESTAMP_TZ_FORMAT = 'SYYYY-MM-DD HH24:MI:SS.FF9 TZH:TZM';
ALTER SESSION SET NLS_NUMERIC_CHARACTERS = '.,';
ALTER SESSION SET TIME_ZONE = '+00:00';

SPOOL &2/meta/database.txt
SELECT 'version|' || banner_full FROM v$version;
SELECT 'param|' || name || '|' || value FROM v$parameter WHERE name IN ('compatible','db_block_size','db_name');
SELECT 'nls|' || parameter || '|' || value FROM nls_database_parameters ORDER BY parameter;
SELECT 'pdb|' || name || '|' || con_id || '|' || dbid FROM v$pdbs ORDER BY con_id;
SELECT 'tde|' || COUNT(*) FROM dba_tablespaces WHERE encrypted = 'YES';
SPOOL OFF

SPOOL &2/meta/datafiles.txt
SELECT file_id || '|' || relative_fno || '|' || tablespace_name || '|' || file_name || '|' || blocks || '|' ||
       (SELECT bigfile FROM dba_tablespaces t WHERE t.tablespace_name = f.tablespace_name)
FROM dba_data_files f ORDER BY file_id;
SPOOL OFF

SPOOL &2/meta/segments.txt
SELECT o.object_name || '|' || o.object_id || '|' || o.data_object_id || '|' || s.segment_type || '|' ||
       s.tablespace_name || '|' || s.header_file || '|' || s.header_block
FROM dba_objects o JOIN dba_segments s ON s.owner = o.owner AND s.segment_name = o.object_name
WHERE o.owner = 'ODR' AND o.object_type IN ('TABLE','CLUSTER') ORDER BY o.object_name;
SPOOL OFF

SPOOL &2/meta/extents.txt
SELECT segment_name || '|' || extent_id || '|' || file_id || '|' || block_id || '|' || blocks
FROM dba_extents WHERE owner = 'ODR' ORDER BY segment_name, extent_id;
SPOOL OFF

SPOOL &2/meta/columns.txt
SELECT table_name || '|' || column_id || '|' || column_name || '|' || data_type || '|' ||
       NVL(TO_CHAR(data_length),'') || '|' || NVL(TO_CHAR(data_precision),'') || '|' ||
       NVL(TO_CHAR(data_scale),'') || '|' || nullable || '|' || NVL(TO_CHAR(segment_column_id),'') || '|' ||
       hidden_column || '|' || virtual_column
FROM dba_tab_cols WHERE owner = 'ODR' ORDER BY table_name, internal_column_id;
SPOOL OFF

-- Row-by-row, column-by-column truth. Generated SQL keeps this script schema-agnostic.
-- DUMP(col,1016) is limited to 4000 bytes of output in SQL (ORA-01489 on a 4000-byte
-- VARCHAR2), so the hex of the stored bytes is produced in PL/SQL from
-- DUMP(col,1016,1,N) pieces of at most 1000 source bytes each and emitted as a CLOB
-- line: TABLE|ID|COLUMN|ROWID|Typ=..|Len=..|CharacterSet=..|<hex bytes>|<canonical text>
SET SERVEROUTPUT ON SIZE UNLIMITED
SPOOL &2/rows.psv
DECLARE
  v_sql    CLOB;
  v_cur    SYS_REFCURSOR;
  v_id     VARCHAR2(100);
  v_rowid  VARCHAR2(40);
  v_head   VARCHAR2(400);
  v_len    PLS_INTEGER;
  v_hex    CLOB;
  v_piece  VARCHAR2(32767);
  v_text   CLOB;
  v_out    CLOB;
  v_pos    PLS_INTEGER;
  v_n      PLS_INTEGER;
  PROCEDURE put_clob(c IN CLOB) IS
    l PLS_INTEGER := DBMS_LOB.GETLENGTH(c); o PLS_INTEGER := 1;
  BEGIN
    IF l IS NULL OR l = 0 THEN DBMS_OUTPUT.PUT_LINE(''); RETURN; END IF;
    WHILE o <= l LOOP
      IF o + 32000 > l THEN DBMS_OUTPUT.PUT_LINE(DBMS_LOB.SUBSTR(c, l - o + 1, o));
      ELSE DBMS_OUTPUT.PUT(DBMS_LOB.SUBSTR(c, 32000, o)); END IF;
      o := o + 32000;
    END LOOP;
  END;
BEGIN
  FOR t IN (SELECT table_name FROM dba_tables WHERE owner = 'ODR' ORDER BY table_name) LOOP
    FOR c IN (SELECT column_name, data_type FROM dba_tab_cols
              WHERE owner = 'ODR' AND table_name = t.table_name AND hidden_column = 'NO'
              ORDER BY column_id) LOOP
      -- DUMP() refuses LOB types (ORA-22849): for CLOB/BLOB the head is synthesised
      -- (Typ=112/113, Len = DBMS_LOB.GETLENGTH), the hex column stays empty (the stored
      -- locator bytes are in the block dumps), and the canonical text carries the data.
      IF c.data_type IN ('CLOB', 'BLOB', 'NCLOB') THEN
        v_sql := 'SELECT TO_CHAR(ID), ROWIDTOCHAR(ROWID), ' ||
                 'CASE WHEN "' || c.column_name || '" IS NULL THEN ''NULL'' ELSE ''Typ=' ||
                 CASE c.data_type WHEN 'BLOB' THEN '113' ELSE '112' END ||
                 ' Len='' || DBMS_LOB.GETLENGTH("' || c.column_name || '") END, 0, '''', ';
      ELSE
      v_sql := 'SELECT TO_CHAR(ID), ROWIDTOCHAR(ROWID), ' ||
               'CASE WHEN "' || c.column_name || '" IS NULL THEN ''NULL'' ELSE ' ||
               'REGEXP_SUBSTR(DUMP("' || c.column_name || '",1016,1,1), ''^Typ=[0-9]+ Len=[0-9]+'') END, ' ||
               'NVL(TO_NUMBER(REGEXP_SUBSTR(DUMP("' || c.column_name || '",1016,1,1), ''Len=([0-9]+)'', 1, 1, NULL, 1)), 0), ' ||
               'NVL(REGEXP_SUBSTR(DUMP("' || c.column_name || '",1016,1,1), ''CharacterSet=[A-Z0-9]+''), ''''), ';
      END IF;
      v_sql := v_sql ||
               CASE
                 WHEN c.data_type LIKE 'RAW%' THEN 'TO_CLOB(RAWTOHEX("' || c.column_name || '"))'
                 -- LOBs: canonical text = the LOB *data* (CLOB as ASCIISTR, BLOB as hex) prefixed
                 -- with its length; the hex column still carries DUMP() of the stored bytes, i.e.
                 -- the in-row locator (+ inline data) — exactly what ODR must reproduce from the block.
                 WHEN c.data_type = 'CLOB' THEN 'TO_CLOB(''len='' || DBMS_LOB.GETLENGTH("' || c.column_name || '") || '':'') || TO_CLOB(ASCIISTR(DBMS_LOB.SUBSTR("' || c.column_name || '", 1000, 1))) || TO_CLOB(ASCIISTR(DBMS_LOB.SUBSTR("' || c.column_name || '", 1000, 1001))) || TO_CLOB(ASCIISTR(DBMS_LOB.SUBSTR("' || c.column_name || '", 1000, 2001))) || TO_CLOB(ASCIISTR(DBMS_LOB.SUBSTR("' || c.column_name || '", 1000, 3001)))'
                 WHEN c.data_type = 'BLOB' THEN 'TO_CLOB(''len='' || DBMS_LOB.GETLENGTH("' || c.column_name || '") || '':'') || TO_CLOB(RAWTOHEX(DBMS_LOB.SUBSTR("' || c.column_name || '", 2000, 1))) || TO_CLOB(RAWTOHEX(DBMS_LOB.SUBSTR("' || c.column_name || '", 2000, 2001)))'
                 WHEN c.data_type LIKE '%CHAR%' THEN 'TO_CLOB(ASCIISTR("' || c.column_name || '"))'
                 WHEN c.data_type IN ('BINARY_FLOAT','BINARY_DOUBLE') THEN 'TO_CLOB(TO_CHAR("' || c.column_name || '",''TM9''))'
                 ELSE 'TO_CLOB(TO_CHAR("' || c.column_name || '"))'
               END ||
               ' FROM ODR."' || t.table_name || '" ORDER BY ID';
      OPEN v_cur FOR v_sql;
      LOOP
        FETCH v_cur INTO v_id, v_rowid, v_head, v_len, v_piece, v_text;
        EXIT WHEN v_cur%NOTFOUND;
        DBMS_LOB.CREATETEMPORARY(v_hex, TRUE);
        IF v_head <> 'NULL' THEN
          v_pos := 1;
          WHILE v_pos <= v_len LOOP
            v_n := LEAST(1000, v_len - v_pos + 1);
            EXECUTE IMMEDIATE 'SELECT REGEXP_REPLACE(DUMP("' || c.column_name || '",1016,' || v_pos || ',' || v_n ||
                              '), ''^.*: '', '''') FROM ODR."' || t.table_name || '" WHERE ROWID = CHARTOROWID(:r)'
              INTO v_sql USING v_rowid;
            -- strip trailing " CharacterSet=..." if present, keep "hh,hh,..."
            v_sql := REGEXP_REPLACE(v_sql, ' CharacterSet=.*$', '');
            IF v_pos > 1 THEN DBMS_LOB.WRITEAPPEND(v_hex, 1, ','); END IF;
            DBMS_LOB.APPEND(v_hex, v_sql);
            v_pos := v_pos + v_n;
          END LOOP;
        END IF;
        DBMS_LOB.CREATETEMPORARY(v_out, TRUE);
        DBMS_LOB.APPEND(v_out, TO_CLOB(t.table_name || '|' || v_id || '|' || c.column_name || '|' || v_rowid || '|' ||
                                       v_head || '|' || v_piece || '|'));
        DBMS_LOB.APPEND(v_out, v_hex);
        DBMS_LOB.APPEND(v_out, TO_CLOB('|'));
        IF v_text IS NOT NULL THEN DBMS_LOB.APPEND(v_out, v_text); END IF;
        put_clob(v_out);
        DBMS_LOB.FREETEMPORARY(v_out);
        DBMS_LOB.FREETEMPORARY(v_hex);
      END LOOP;
      CLOSE v_cur;
    END LOOP;
  END LOOP;
END;
/
SPOOL OFF

SPOOL &2/meta/rowcounts.txt
DECLARE n NUMBER;
BEGIN
  FOR t IN (SELECT table_name FROM dba_tables WHERE owner = 'ODR' ORDER BY table_name) LOOP
    EXECUTE IMMEDIATE 'SELECT COUNT(*) FROM ODR."' || t.table_name || '"' INTO n;
    DBMS_OUTPUT.PUT_LINE(t.table_name || '|' || n);
  END LOOP;
END;
/
SPOOL OFF

-- Physical location of every row (file, block, slot) — lets tests compare ODR's
-- provenance with Oracle's own ROWID decoding.
SPOOL &2/meta/rowids.psv
DECLARE
  v_cur SYS_REFCURSOR; v_line VARCHAR2(4000);
BEGIN
  FOR t IN (SELECT table_name FROM dba_tables WHERE owner = 'ODR' ORDER BY table_name) LOOP
    OPEN v_cur FOR 'SELECT ''' || t.table_name || '|'' || ID || ''|'' || DBMS_ROWID.ROWID_OBJECT(ROWID) || ''|'' || ' ||
                   'DBMS_ROWID.ROWID_RELATIVE_FNO(ROWID) || ''|'' || DBMS_ROWID.ROWID_BLOCK_NUMBER(ROWID) || ''|'' || ' ||
                   'DBMS_ROWID.ROWID_ROW_NUMBER(ROWID) FROM ODR."' || t.table_name || '" ORDER BY ID';
    LOOP FETCH v_cur INTO v_line; EXIT WHEN v_cur%NOTFOUND; DBMS_OUTPUT.PUT_LINE(v_line); END LOOP;
    CLOSE v_cur;
  END LOOP;
END;
/
SPOOL OFF
EXIT
