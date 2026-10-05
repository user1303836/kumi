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
