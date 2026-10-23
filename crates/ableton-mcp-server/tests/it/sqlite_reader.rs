use ableton_mcp_server::sqlite_reader::{SqliteReader, SqliteValue, DEFAULT_SCAN_MAX_ROWS};

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

// Two small SQLite table-leaf pages, built from the public file format.
fn database(serial: u8, body: &[u8], row_id: &[u8], reserved: u8) -> Vec<u8> {
    let mut bytes = vec![0u8; 1024];
    bytes[0..16].copy_from_slice(b"SQLite format 3\0");
    write_u16(&mut bytes, 16, 512);
    bytes[18] = 1;
    bytes[19] = 1;
    bytes[20] = reserved;
    write_u32(&mut bytes, 28, 2);
    write_u32(&mut bytes, 56, 1);
    let leaf = |bytes: &mut Vec<u8>, base: usize, header: usize, record: &[u8], id: &[u8]| {
        let cell = [&[record.len() as u8][..], id, record].concat();
        let offset = 512 - reserved as usize - cell.len();
        bytes[base + header] = 0x0d;
        write_u16(bytes, base + header + 3, 1);
        write_u16(bytes, base + header + 5, offset as u16);
        write_u16(bytes, base + header + 8, offset as u16);
        bytes[base + offset..base + offset + cell.len()].copy_from_slice(&cell);
    };
    let sql = "CREATE TABLE t(v)";
    let schema = [&[6, 23, 15, 15, 1, 13 + sql.len() as u8 * 2][..], b"tablett", &[2], sql.as_bytes()].concat();
    leaf(&mut bytes, 0, 100, &schema, &[1]);
    leaf(&mut bytes, 512, 0, &[&[2, serial][..], body].concat(), row_id);
    bytes
}

fn first_value(bytes: Vec<u8>) -> SqliteValue {
    SqliteReader::new(bytes).unwrap().scan_table("t", DEFAULT_SCAN_MAX_ROWS).unwrap()[0].row[0].clone()
}

fn scan_error(bytes: Vec<u8>) -> String {
    match SqliteReader::new(bytes) {
        Err(error) => error.to_string(),
        Ok(reader) => reader.scan_table("t", DEFAULT_SCAN_MAX_ROWS).expect_err("the scan must fail").to_string(),
    }
}

#[test]
fn sqlite_decodes_signed_48_bit_and_64_bit_integers_without_overlapping_words_or_rounding() {
    for value in [(1i64 << 40) + 123, -(1i64 << 40) + 123, (1i64 << 47) - 1, -(1i64 << 47)] {
        let body = &value.to_be_bytes()[2..8];
        assert_eq!(first_value(database(5, body, &[1], 0)), SqliteValue::Integer(value));
    }
    for value in [(1i64 << 48) + 123, -(1i64 << 48) + 123, 9_007_199_254_740_991, -9_007_199_254_740_991] {
        assert_eq!(first_value(database(6, &value.to_be_bytes(), &[1], 0)), SqliteValue::Integer(value));
    }
    for value in [1i64 << 53, -(1i64 << 53), i64::MAX, i64::MIN] {
        let error = scan_error(database(6, &value.to_be_bytes(), &[1], 0));
        assert!(error.contains("safe JavaScript range"), "{error}");
    }
    assert_eq!(SqliteReader::new(database(9, &[], &[0xff; 9], 0)).unwrap().scan_table("t", DEFAULT_SCAN_MAX_ROWS).unwrap()[0].row_id, -1);
}

#[test]
fn sqlite_honors_reserved_page_bytes_and_refuses_unsupported_text_encodings() {
    assert_eq!(first_value(database(9, &[], &[1], 12)), SqliteValue::Integer(1));
    let mut reserved = database(9, &[], &[1], 0);
    reserved[20] = 33;
    let error = SqliteReader::new(reserved).err().expect("reserved bytes must be refused").to_string();
    assert!(error.contains("invalid usable page size"), "{error}");
    for encoding in [0, 2, 3] {
        let mut bytes = database(9, &[], &[1], 0);
        write_u32(&mut bytes, 56, encoding);
        let error = SqliteReader::new(bytes).err().expect("the encoding must be refused").to_string();
        assert!(error.contains("text encoding"), "{error}");
    }
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut groups = vec![(value & 0x7f) as u8];
    value >>= 7;
    while value > 0 {
        groups.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    groups.reverse();
    groups
}
/// A database of `page_size` pages: the schema (table t, root page 2), a table leaf holding `cells`, then `rest`.
fn built(page_size: usize, cells: &[Vec<u8>], rest: &[Vec<u8>]) -> Vec<u8> {
    built_with("CREATE TABLE t(v)", page_size, cells, rest)
}
/// As built, with table t created by `sql` (its schema cell on the first page, so up to about `page_size` bytes).
fn built_with(sql: &str, page_size: usize, cells: &[Vec<u8>], rest: &[Vec<u8>]) -> Vec<u8> {
    let count = 2 + rest.len();
    let mut bytes = vec![0u8; page_size * count];
    bytes[0..16].copy_from_slice(b"SQLite format 3\0");
    write_u16(&mut bytes, 16, if page_size == 65536 { 1 } else { page_size as u16 });
    bytes[18] = 1;
    bytes[19] = 1;
    write_u32(&mut bytes, 28, count as u32);
    write_u32(&mut bytes, 56, 1);
    let leaf = |bytes: &mut Vec<u8>, base: usize, header: usize, cells: &[Vec<u8>]| {
        bytes[base + header] = 0x0d;
        write_u16(bytes, base + header + 3, cells.len() as u16);
        let mut end = page_size;
        for (index, cell) in cells.iter().enumerate() {
            end -= cell.len();
            bytes[base + end..base + end + cell.len()].copy_from_slice(cell);
            write_u16(bytes, base + header + 8 + index * 2, end as u16);
        }
        write_u16(bytes, base + header + 5, end as u16);
    };
    let types = [&[23, 15, 15, 1][..], &varint(13 + 2 * sql.len() as u64)].concat();
    let schema = [&[1 + types.len() as u8][..], &types, b"tablett", &[2], sql.as_bytes()].concat();
    leaf(&mut bytes, 0, 100, &[[varint(schema.len() as u64), vec![1], schema].concat()]);
    leaf(&mut bytes, page_size, 0, cells);
    for (index, page) in rest.iter().enumerate() {
        bytes[page_size * (2 + index)..][..page.len()].copy_from_slice(page);
    }
    bytes
}
#[test]
fn sqlite_refuses_cells_that_share_an_overflow_chain_and_records_past_their_tables_columns() {
    // 600-byte blobs on 512-byte pages: each keeps its first bytes in the cell and the rest on one overflow page.
    let (usable, length) = (512usize, 600usize);
    let (max_local, min_local) = (usable - 35, ((usable - 12) * 32) / 255 - 23);
    let k = min_local + (length - min_local) % (usable - 4);
    let local = if k <= max_local { k } else { min_local };
    let payload = |byte: u8| [&[3][..], &varint(2 * 597 + 12), &[byte; 597]].concat();
    let cell = |row: u8, overflow: u32| {
        [varint(length as u64), vec![row], payload(row)[..local].to_vec(), overflow.to_be_bytes().to_vec()].concat()
    };
    let overflow = |row: u8| [&[0u8; 4][..], &payload(row)[local..]].concat();
    // Each its own chain: two rows.
    let separate = SqliteReader::new(built(512, &[cell(1, 3), cell(2, 4)], &[overflow(1), overflow(2)])).unwrap();
    let rows = separate.scan_table("t", DEFAULT_SCAN_MAX_ROWS).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.row[0].clone()).collect::<Vec<_>>(),
        [SqliteValue::Blob(vec![1; 597]), SqliteValue::Blob(vec![2; 597])]
    );
    // One chain for both: every cell could repeat a 64 MB chain, so it's malformed.
    let error = scan_error(built(512, &[cell(1, 3), cell(1, 3)], &[overflow(1)]));
    assert!(error.contains("overflow chain is malformed"), "{error}");
    // Records of NULLs in `t(v)`. Each NULL is a byte here and a whole value decoded, so a record naming more values
    // than the table's one column is malformed: SQLite never writes one. Fewer is a row from before ADD COLUMN.
    let nulls = |count: usize| {
        let mut length = count + 1;
        while varint(length as u64).len() + count != length {
            length += 1;
        }
        let header = [varint(length as u64), vec![0; count]].concat();
        built(65536, &[[varint(header.len() as u64), vec![1], header].concat()], &[])
    };
    for count in [0, 1] {
        let rows = SqliteReader::new(nulls(count)).unwrap().scan_table("t", DEFAULT_SCAN_MAX_ROWS).unwrap();
        assert_eq!(rows[0].row, vec![SqliteValue::Null; count]);
    }
    for count in [2, 1_000, 40_000] {
        let error = scan_error(nulls(count));
        assert!(error.contains("more columns than its table has"), "{count}: {error}");
    }
}
#[test]
fn sqlite_scans_no_table_wider_than_it_reads_whatever_the_file_declares() {
    // A record of `count` NULLs in a table the file declares with `columns` columns.
    let table = |columns: usize, count: usize| {
        let sql = format!("CREATE TABLE t({})", (0..columns).map(|at| format!("c{at}")).collect::<Vec<_>>().join(","));
        let mut length = count + 1;
        while varint(length as u64).len() + count != length {
            length += 1;
        }
        let header = [varint(length as u64), vec![0; count]].concat();
        built_with(&sql, 65536, &[[varint(header.len() as u64), vec![1], header].concat()], &[])
    };
    let rows = SqliteReader::new(table(64, 64)).unwrap().scan_table("t", DEFAULT_SCAN_MAX_ROWS).unwrap();
    assert_eq!(rows[0].row, vec![SqliteValue::Null; 64]);
    // The file's own declaration can't raise that: each NULL is a byte here and 32 decoded, so 5,000 declared columns
    // let a record decode 32 times its size (the parent read it), and 32,767 a 128 MiB file about 4 GiB.
    for (columns, count) in [(65, 1), (5_000, 0), (5_000, 5_000)] {
        let error = scan_error(table(columns, count));
        assert!(error.contains("more than the 64 columns a scan reads"), "{columns}, {count}: {error}");
    }
}
#[test]
fn sqlite_rejects_repeated_b_tree_pages_and_cross_page_cell_pointers() {
    let mut cycle = database(9, &[], &[1], 0);
    cycle[512] = 0x05;
    write_u16(&mut cycle, 515, 0);
    write_u32(&mut cycle, 520, 2);
    let error = scan_error(cycle);
    assert!(error.contains("repeated page"), "{error}");
    let mut pointers = database(9, &[], &[1], 0);
    write_u16(&mut pointers, 515, 300);
    let error = scan_error(pointers);
    assert!(error.contains("pointer array overruns"), "{error}");
    let mut cell = database(9, &[], &[1], 0);
    write_u16(&mut cell, 520, 511);
    let error = scan_error(cell);
    assert!(error.contains("cell pointer"), "{error}");
}
