//! Minimal dependency-free read-only SQLite 3 file reader.
//!
//! Scope (deliberately bounded for the Live library-database surface, issue #54):
//! - Parses the 100-byte database header ("SQLite format 3"), the schema table
//!   (sqlite_master), and table b-trees (interior 0x05 / leaf 0x0d pages) with
//!   full record decoding (varints, serial types 0-9, text/blob payloads) and
//!   overflow-page chains.
//! - Full table scans only; indexes, freelist handling, and pointer-map pages
//!   are not needed and not implemented.
//! - WAL: a database whose read version is 2 (WAL mode) is only read when its
//!   -wal file is absent or empty (fully checkpointed); otherwise the caller
//!   must fail closed. This reader never writes, never creates journals, and
//!   never replays WAL frames.
//! - Every structural bound (page count, cells per page, payload size, b-tree
//!   depth, row count) is enforced so malformed or hostile files fail closed
//!   with explicit errors instead of crashing or looping.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

/// A structural or bound failure while reading; the text is the TypeScript's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SqliteError(pub String);

fn fail(message: impl Into<String>) -> SqliteError {
    SqliteError(message.into())
}

#[derive(Debug, Clone, PartialEq)]
pub struct SqliteTable {
    pub name: String,
    pub root_page: i64,
    pub sql: String,
}

/// One column value. Integers past the safe JavaScript range never appear: the
/// reader fails closed on them, as the TypeScript did.
#[derive(Debug, Clone, PartialEq)]
pub enum SqliteValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqliteValue {
    /// The value where the TypeScript saw a `number` (integers and reals alike).
    pub fn as_number(&self) -> Option<f64> {
        match self {
            SqliteValue::Integer(value) => Some(*value as f64),
            SqliteValue::Real(value) => Some(*value),
            _ => None,
        }
    }

    /// The value where the TypeScript saw a `string`.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            SqliteValue::Text(value) => Some(value),
            _ => None,
        }
    }
}

pub type SqliteRow = Vec<SqliteValue>;

/// A scanned row with its cell rowid (INTEGER PRIMARY KEY alias columns read as NULL).
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedRow {
    pub row: SqliteRow,
    pub row_id: i64,
}

const SQLITE_HEADER_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const MAX_DATABASE_BYTES: usize = 512 * 1024 * 1024;
const MAX_PAGES: u32 = 1_000_000;
const MAX_CELLS_PER_PAGE: usize = 500;
const MAX_BTREE_DEPTH: usize = 64;
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// SQLite's own hard limit on a table's columns.
const MAX_COLUMNS: usize = 32_767;
/// The schema table's columns: type, name, tbl_name, rootpage and sql.
const SCHEMA_COLUMNS: usize = 5;
/// The most definitions a scanned table may have. Live's library tables have at most 21, so this leaves room for those
/// it adds. It isn't the file's to raise: each value a record names is a byte in the file but 32 decoded, so a table
/// declared with 32,767 columns would let a 128 MiB file decode to about 4 GiB.
const MAX_SCANNED_COLUMNS: usize = 64;
const MAX_ROWS: usize = 1_000_000;
/// `scanTable`'s default row bound.
pub const DEFAULT_SCAN_MAX_ROWS: usize = 100_000;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

struct Varint {
    value: u64,
    bytes: usize,
}

fn read_varint(buffer: &[u8], offset: usize) -> Result<Varint, SqliteError> {
    let mut value: u64 = 0;
    for index in 0..9 {
        let byte = *buffer.get(offset + index).ok_or_else(|| fail("sqlite varint extends past the buffer"))?;
        if index == 8 {
            value = (value << 8) | byte as u64;
            return Ok(Varint { value, bytes: 9 });
        }
        value = (value << 7) | (byte & 0x7f) as u64;
        if byte & 0x80 == 0 {
            return Ok(Varint { value, bytes: index + 1 });
        }
    }
    Err(fail("sqlite varint is malformed"))
}

/// Convert a possibly >2^53 integer defensively (SQLite ints are signed 64-bit;
/// values beyond Number.MAX_SAFE_INTEGER fail closed rather than losing precision).
fn to_safe_integer(value: i64) -> Result<i64, SqliteError> {
    if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value) {
        return Err(fail("sqlite integer exceeds the safe JavaScript range"));
    }
    Ok(value)
}

/// The most values a record of the table `sql` creates can hold: its definitions (the columns, and any table
/// constraints, which only add room), counted outside quotes and comments. SQLite never writes more, and fewer is fine
/// (a row from before ADD COLUMN). A statement that doesn't read as one gets SQLite's own limit.
fn record_bound(sql: &str) -> usize {
    let mut chars = sql.chars().peekable();
    let (mut definitions, mut depth) = (1, 0usize);
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' | '`' | '[' => {
                let end = if c == '[' { ']' } else { c };
                // A doubled quote inside is two quotes in a row: the scan closes and reopens.
                if !chars.by_ref().any(|next| next == end) {
                    break;
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                chars.by_ref().find(|&next| next == '\n');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                if !chars.by_ref().any(|next| std::mem::replace(&mut last, next) == '*' && next == '/') {
                    break;
                }
            }
            '(' => depth += 1,
            ')' if depth == 1 => return definitions.min(MAX_COLUMNS),
            ')' if depth == 0 => break,
            ')' => depth -= 1,
            ',' if depth == 1 => definitions += 1,
            _ => {}
        }
    }
    MAX_COLUMNS
}

fn be_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn be_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}

/// JavaScript's default string order: UTF-16 code units.
fn js_str_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// JavaScript's `\s`.
fn is_js_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

struct Walk {
    visited: usize,
    seen_pages: HashSet<i64>,
    /// The most values a record of the table holds (`record_bound`).
    values: usize,
}

pub struct SqliteReader {
    pages: Vec<u8>,
    pub page_size: usize,
    usable_size: usize,
    pub page_count: u32,
    pub wal_mode: bool,
    tables: HashMap<String, SqliteTable>,
}

impl SqliteReader {
    pub fn new(bytes: Vec<u8>) -> Result<Self, SqliteError> {
        if bytes.len() > MAX_DATABASE_BYTES {
            return Err(fail("sqlite database exceeds the 512 MiB bound"));
        }
        if bytes.len() < 100 {
            return Err(fail("sqlite database is truncated"));
        }
        if &bytes[0..16] != SQLITE_HEADER_MAGIC {
            return Err(fail("not a SQLite 3 database (bad header magic)"));
        }
        let page_size_raw = be_u16(&bytes, 16) as usize;
        let page_size = if page_size_raw == 1 { 65536 } else { page_size_raw };
        if page_size < 512 || page_size > 65536 || page_size & (page_size - 1) != 0 {
            return Err(fail("sqlite page size is invalid"));
        }
        let write_version = bytes[18];
        let read_version = bytes[19];
        if write_version != 1 && write_version != 2 {
            return Err(fail("sqlite file-format write version is unsupported"));
        }
        if read_version != 1 && read_version != 2 {
            return Err(fail("sqlite file-format read version is unsupported"));
        }
        let wal_mode = read_version == 2;
        let usable_size = page_size - bytes[20] as usize;
        if usable_size < 480 {
            return Err(fail("sqlite reserved bytes leave an invalid usable page size"));
        }
        if be_u32(&bytes, 56) != 1 {
            return Err(fail("sqlite text encoding is unsupported; UTF-8 is required"));
        }
        let page_count = be_u32(&bytes, 28);
        if page_count < 1 || page_count > MAX_PAGES {
            return Err(fail("sqlite page count is invalid"));
        }
        let expected = page_count as usize * page_size;
        if bytes.len() < expected {
            return Err(fail("sqlite database is truncated relative to its header page count"));
        }
        let mut pages = bytes;
        pages.truncate(expected);
        let mut reader = SqliteReader { pages, page_size, usable_size, page_count, wal_mode, tables: HashMap::new() };
        for table in reader.read_schema()? {
            reader.tables.insert(table.name.clone(), table);
        }
        Ok(reader)
    }

    pub fn table_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tables.keys().cloned().collect();
        names.sort_by(|a, b| js_str_cmp(a, b));
        names
    }

    pub fn table(&self, name: &str) -> Option<&SqliteTable> {
        self.tables.get(name)
    }

    fn page_offset(&self, page: i64) -> Result<usize, SqliteError> {
        if page < 1 || page > self.page_count as i64 {
            return Err(fail(format!("sqlite page {page} is outside the database")));
        }
        Ok((page as usize - 1) * self.page_size)
    }

    /// Parse one b-tree cell's record payload, following the overflow chain. `pages` holds every page this walk has
    /// used, b-tree and overflow alike: a page is any one cell's overflow at most, so cells can't share one chain.
    fn read_cell_payload(&self, page: i64, cell_offset: usize, pages: &mut HashSet<i64>) -> Result<(Vec<u8>, i64), SqliteError> {
        let base = self.page_offset(page)?;
        let usable = self.usable_size;
        let page_bytes = &self.pages[base..base + usable];
        let mut cursor = cell_offset;
        let payload_length = read_varint(page_bytes, cursor)?;
        cursor += payload_length.bytes;
        let row_id_start = cursor;
        let row_id = read_varint(page_bytes, cursor)?;
        cursor += row_id.bytes;
        let mut signed_row_id: u64 = 0;
        for index in 0..row_id.bytes {
            let byte = page_bytes[row_id_start + index];
            signed_row_id = if index == 8 { (signed_row_id << 8) | byte as u64 } else { (signed_row_id << 7) | (byte & 0x7f) as u64 };
        }
        if payload_length.value > MAX_PAYLOAD_BYTES {
            return Err(fail("sqlite cell payload exceeds its bound"));
        }
        let length = payload_length.value as usize;
        // SQLite file-format table leaf rule: payloads up to maxLocal stay inline;
        // larger payloads keep K = minLocal + (P - minLocal) % (usable - 4) bytes
        // inline when K <= maxLocal, else minLocal, and overflow the rest.
        let max_local = usable - 35;
        let min_local = ((usable - 12) * 32) / 255 - 23;
        let mut local = length;
        if length > max_local {
            let k = min_local + ((length - min_local) % (usable - 4));
            local = if k <= max_local { k } else { min_local };
        }
        if cursor + local > usable {
            return Err(fail("sqlite cell payload overruns its page"));
        }
        let mut chunks: Vec<&[u8]> = vec![&page_bytes[cursor..cursor + local]];
        let mut remaining = length - local;
        if remaining > 0 {
            if cursor + local + 4 > usable {
                return Err(fail("sqlite overflow pointer overruns its page"));
            }
            let mut overflow_page = be_u32(&self.pages, base + cursor + local) as i64;
            while remaining > 0 {
                if overflow_page == 0 || !pages.insert(overflow_page) {
                    return Err(fail("sqlite overflow chain is malformed"));
                }
                let overflow_base = self.page_offset(overflow_page)?;
                let next = be_u32(&self.pages, overflow_base) as i64;
                let take = remaining.min(usable - 4);
                chunks.push(&self.pages[overflow_base + 4..overflow_base + 4 + take]);
                remaining -= take;
                overflow_page = next;
            }
            if overflow_page != 0 {
                return Err(fail("sqlite overflow chain has trailing pages"));
            }
        }
        let payload = chunks.concat();
        Ok((payload, to_safe_integer(signed_row_id as i64)?))
    }

    fn decode_record(&self, payload: &[u8], values: usize) -> Result<SqliteRow, SqliteError> {
        let header_length = read_varint(payload, 0)?;
        if header_length.value < 1 || header_length.value > payload.len() as u64 {
            return Err(fail("sqlite record header is invalid"));
        }
        let header_end = header_length.value as usize;
        let mut serial_types: Vec<u64> = Vec::new();
        let mut cursor = header_length.bytes;
        while cursor < header_end {
            // Each NULL or empty value is a byte here and a whole value decoded: only the table's columns are read.
            if serial_types.len() == values {
                return Err(fail("sqlite record names more columns than its table has"));
            }
            let serial = read_varint(payload, cursor)?;
            cursor += serial.bytes;
            serial_types.push(serial.value);
        }
        if cursor != header_end {
            return Err(fail("sqlite serial type overruns the record header"));
        }
        let mut row: SqliteRow = Vec::with_capacity(serial_types.len());
        let mut body = header_end;
        for serial in serial_types {
            let take = |body: usize, bytes: u64| -> Result<(), SqliteError> {
                match usize::try_from(bytes).ok().and_then(|bytes| body.checked_add(bytes)) {
                    Some(end) if end <= payload.len() => Ok(()),
                    _ => Err(fail("sqlite record body is truncated")),
                }
            };
            match serial {
                0 => row.push(SqliteValue::Null),
                1 => {
                    take(body, 1)?;
                    row.push(SqliteValue::Integer(payload[body] as i8 as i64));
                    body += 1;
                }
                2 => {
                    take(body, 2)?;
                    row.push(SqliteValue::Integer(i16::from_be_bytes([payload[body], payload[body + 1]]) as i64));
                    body += 2;
                }
                3 => {
                    take(body, 3)?;
                    let value = ((payload[body] as i64) << 16) | ((payload[body + 1] as i64) << 8) | payload[body + 2] as i64;
                    row.push(SqliteValue::Integer(if value >= 1 << 23 { value - (1 << 24) } else { value }));
                    body += 3;
                }
                4 => {
                    take(body, 4)?;
                    row.push(SqliteValue::Integer(be_u32(payload, body) as i32 as i64));
                    body += 4;
                }
                5 => {
                    take(body, 6)?;
                    let high = i16::from_be_bytes([payload[body], payload[body + 1]]) as i64;
                    let low = be_u32(payload, body + 2) as i64;
                    row.push(SqliteValue::Integer(to_safe_integer(high * (1i64 << 32) + low)?));
                    body += 6;
                }
                6 => {
                    take(body, 8)?;
                    let mut raw = [0u8; 8];
                    raw.copy_from_slice(&payload[body..body + 8]);
                    row.push(SqliteValue::Integer(to_safe_integer(i64::from_be_bytes(raw))?));
                    body += 8;
                }
                7 => {
                    take(body, 8)?;
                    let mut raw = [0u8; 8];
                    raw.copy_from_slice(&payload[body..body + 8]);
                    row.push(SqliteValue::Real(f64::from_be_bytes(raw)));
                    body += 8;
                }
                8 => row.push(SqliteValue::Integer(0)),
                9 => row.push(SqliteValue::Integer(1)),
                _ => {
                    if serial < 12 {
                        return Err(fail(format!("sqlite serial type {serial} is reserved")));
                    }
                    let length = if serial >= 13 && serial % 2 == 1 { (serial - 13) / 2 } else { (serial - 12) / 2 };
                    take(body, length)?;
                    let length = length as usize;
                    let slice = &payload[body..body + length];
                    row.push(if serial % 2 == 1 {
                        SqliteValue::Text(String::from_utf8_lossy(slice).into_owned())
                    } else {
                        SqliteValue::Blob(slice.to_vec())
                    });
                    body += length;
                }
            }
        }
        Ok(row)
    }

    fn walk_table_btree<F>(&self, root_page: i64, mut visit: F, max_rows: usize, values: usize) -> Result<usize, SqliteError>
    where
        F: FnMut(SqliteRow, i64) -> Result<(), SqliteError>,
    {
        let mut walk = Walk { visited: 0, seen_pages: HashSet::new(), values };
        self.walk_page(root_page, 0, &mut walk, &mut visit, max_rows)?;
        Ok(walk.visited)
    }

    fn walk_page<F>(&self, page: i64, depth: usize, walk: &mut Walk, visit: &mut F, max_rows: usize) -> Result<(), SqliteError>
    where
        F: FnMut(SqliteRow, i64) -> Result<(), SqliteError>,
    {
        if !walk.seen_pages.insert(page) {
            return Err(fail("sqlite b-tree contains a repeated page"));
        }
        if depth > MAX_BTREE_DEPTH {
            return Err(fail("sqlite b-tree depth exceeds its bound"));
        }
        let base = self.page_offset(page)?;
        let header_offset = if page == 1 { 100 } else { 0 };
        let kind = self.pages[base + header_offset];
        let cell_count = be_u16(&self.pages, base + header_offset + 3) as usize;
        if cell_count > MAX_CELLS_PER_PAGE {
            return Err(fail("sqlite page cell count exceeds its bound"));
        }
        let pointer_array = base + header_offset + if kind == 0x05 { 12 } else { 8 };
        if pointer_array + cell_count * 2 > base + self.usable_size {
            return Err(fail("sqlite cell pointer array overruns its page"));
        }
        let mut seen_cells: HashSet<usize> = HashSet::new();
        let cell_at = |index: usize, seen_cells: &mut HashSet<usize>| -> Result<usize, SqliteError> {
            let offset = be_u16(&self.pages, pointer_array + index * 2) as usize;
            if offset < pointer_array - base + cell_count * 2
                || offset + if kind == 0x05 { 5 } else { 2 } > self.usable_size
                || seen_cells.contains(&offset)
            {
                return Err(fail("sqlite cell pointer is invalid or repeated"));
            }
            seen_cells.insert(offset);
            Ok(offset)
        };
        if kind == 0x05 {
            for index in 0..cell_count {
                let cell_offset = cell_at(index, &mut seen_cells)?;
                let child_page = be_u32(&self.pages, base + cell_offset) as i64;
                self.walk_page(child_page, depth + 1, walk, visit, max_rows)?;
                // interior cells carry a key (varint rowid) that separates subtrees; no payload to visit
            }
            let rightmost = be_u32(&self.pages, base + header_offset + 8) as i64;
            self.walk_page(rightmost, depth + 1, walk, visit, max_rows)?;
        } else if kind == 0x0d {
            for index in 0..cell_count {
                if walk.visited >= max_rows {
                    return Err(fail(format!("sqlite table scan exceeds its {max_rows}-row bound")));
                }
                let cell_offset = cell_at(index, &mut seen_cells)?;
                let (payload, row_id) = self.read_cell_payload(page, cell_offset, &mut walk.seen_pages)?;
                visit(self.decode_record(&payload, walk.values)?, row_id)?;
                walk.visited += 1;
            }
        } else {
            return Err(fail(format!("sqlite page {page} is not a table b-tree page (type {kind})")));
        }
        Ok(())
    }

    fn read_schema(&self) -> Result<Vec<SqliteTable>, SqliteError> {
        let mut tables: Vec<SqliteTable> = Vec::new();
        self.walk_table_btree(
            1,
            |row, _| {
                if row.len() < 5 {
                    return Err(fail("sqlite schema row is malformed"));
                }
                if let (SqliteValue::Text(kind), SqliteValue::Text(name), root_page, SqliteValue::Text(sql)) =
                    (&row[0], &row[1], &row[3], &row[4])
                {
                    // TS: `typeof rootPage === "number"`; a REAL root page that isn't a whole number
                    // cannot address a page, so its table is left out instead of failing at scan time.
                    let root_page = match root_page {
                        SqliteValue::Integer(value) => Some(*value),
                        SqliteValue::Real(value) if value.fract() == 0.0 && value.is_finite() => Some(*value as i64),
                        _ => None,
                    };
                    if let Some(root_page) = root_page {
                        if kind == "table" {
                            tables.push(SqliteTable { name: name.clone(), root_page, sql: sql.clone() });
                        }
                    }
                }
                Ok(())
            },
            MAX_ROWS,
            SCHEMA_COLUMNS,
        )?;
        Ok(tables)
    }

    /// Column names parsed from a table's CREATE TABLE statement (best-effort:
    /// top-level comma split, first token of each definition; table constraints
    /// are skipped). Used for fail-closed schema-profile checks.
    pub fn table_columns(&self, name: &str) -> Option<Vec<String>> {
        static CONSTRAINT: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)^(PRIMARY|UNIQUE|CHECK|FOREIGN|CONSTRAINT)$").unwrap());
        let table = self.tables.get(name)?;
        let open = table.sql.find('(')?;
        let close = table.sql.rfind(')')?;
        if close <= open {
            return None;
        }
        let body = &table.sql[open + 1..close];
        let mut columns: Vec<String> = Vec::new();
        let mut depth: i64 = 0;
        let mut current = String::new();
        for char in body.chars() {
            if char == '(' {
                depth += 1;
            }
            if char == ')' {
                depth -= 1;
            }
            if char == ',' && depth == 0 {
                columns.push(std::mem::take(&mut current));
            } else {
                current.push(char);
            }
        }
        if !kumi_common::js::string::trim(&current).is_empty() {
            columns.push(current);
        }
        let names = columns
            .iter()
            .filter_map(|definition| {
                let token = kumi_common::js::string::trim(definition).split(is_js_whitespace).next()?;
                let token = token.strip_prefix(['"', '\'', '`', '[']).unwrap_or(token);
                let token = token.strip_suffix(['"', '\'', '`', ']']).unwrap_or(token);
                (!token.is_empty() && !CONSTRAINT.is_match(token)).then(|| token.to_string())
            })
            .collect();
        Some(names)
    }

    /// Full bounded table scan. Rows are raw positional values matching the
    /// CREATE TABLE column order (see tableColumns); the cell rowid is surfaced
    /// separately because INTEGER PRIMARY KEY alias columns read as NULL.
    pub fn scan_table(&self, name: &str, max_rows: usize) -> Result<Vec<ScannedRow>, SqliteError> {
        let table = self.tables.get(name).ok_or_else(|| fail(format!("sqlite table is not present: {name}")))?;
        let values = record_bound(&table.sql);
        if values > MAX_SCANNED_COLUMNS {
            return Err(fail(format!("sqlite table {name} has more than the {MAX_SCANNED_COLUMNS} columns a scan reads")));
        }
        let mut rows: Vec<ScannedRow> = Vec::new();
        self.walk_table_btree(
            table.root_page,
            |row, row_id| {
                rows.push(ScannedRow { row, row_id });
                Ok(())
            },
            max_rows,
            values,
        )?;
        if rows.len() > max_rows {
            return Err(fail(format!("sqlite table {name} exceeds its scan bound")));
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_holds_at_most_its_tables_definitions() {
        for (sql, bound) in [
            ("CREATE TABLE t(v)", 1),
            ("CREATE TABLE ancestors (file_id INTEGER, ancestor_id INTEGER, UNIQUE (file_id, ancestor_id) ON CONFLICT IGNORE)", 3),
            ("CREATE TABLE \"a(b)\" (x, y DEFAULT 'it''s, (here', z /* a, b */, -- c, d\n w)", 4),
            ("CREATE TABLE [t,u] (x, `y,z`)", 2),
            ("CREATE TABLE t AS SELECT 1", MAX_COLUMNS),
            ("CREATE TABLE t(x, 'unclosed)", MAX_COLUMNS),
        ] {
            assert_eq!(record_bound(sql), bound, "{sql}");
        }
    }
}
