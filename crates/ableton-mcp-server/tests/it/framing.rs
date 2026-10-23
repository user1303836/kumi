use ableton_mcp_server::framing::{FrameError, FrameEvent, NdjsonFramer, MAX_FRAME_BYTES};

#[test]
fn frames_arbitrary_chunks_crlf_multiple_records_and_eof() {
    let mut framer = NdjsonFramer::new();
    let mut events = framer.push(b"{\"a\":");
    events.extend(framer.push(b"1}\r\n{\"b\":2}\n"));
    events.extend(framer.end());
    assert_eq!(events, vec![FrameEvent::Record("{\"a\":1}".into()), FrameEvent::Record("{\"b\":2}".into())]);
}

#[test]
fn reports_invalid_utf8_and_discards_exactly_one_oversized_record() {
    // The bound is as large as a string can be (500 MiB); a framer with a small one shows the same handling
    // without allocating it: an oversized record, whole or arriving in chunks, is dropped and the next one read.
    assert_eq!(MAX_FRAME_BYTES, 500 * 1024 * 1024);
    let bound = 1024;
    let mut framer = NdjsonFramer::with_max_bytes(bound);
    assert_eq!(framer.push(&[0xc3, 0x28, 10]), vec![FrameEvent::Error(FrameError::InvalidUtf8)]);
    let mut oversized = vec![97u8; bound + 2];
    oversized[bound + 1] = 10;
    assert_eq!(framer.push(&oversized), vec![FrameEvent::Error(FrameError::Oversized)]);
    assert_eq!(framer.push(b"ok\n"), vec![FrameEvent::Record("ok".into())]);
    assert_eq!(framer.retained_bytes(), 0);
    let chunk = vec![98u8; bound / 4];
    for _ in 0..5 {
        assert_eq!(framer.push(&chunk), vec![]);
    }
    assert_eq!(framer.retained_bytes(), 0, "an oversized record is not retained while it arrives");
    assert_eq!(framer.push(b"tail\nnext\n"), vec![FrameEvent::Error(FrameError::Oversized), FrameEvent::Record("next".into())]);
    let mut exact = vec![99u8; bound + 1];
    exact[bound] = 10;
    assert_eq!(framer.push(&exact), vec![FrameEvent::Record("c".repeat(bound))], "a record of exactly the bound is read");
}

#[test]
fn strips_a_leading_byte_order_mark_as_the_text_decoder_did() {
    let mut framer = NdjsonFramer::new();
    assert_eq!(framer.push(b"\xef\xbb\xbf{\"a\":1}\n"), vec![FrameEvent::Record("{\"a\":1}".into())]);
    assert_eq!(framer.push(b"x\xef\xbb\xbfy\n"), vec![FrameEvent::Record("x\u{FEFF}y".into())]);
    assert_eq!(FrameError::InvalidUtf8.message(), "invalid-utf8");
    assert_eq!(FrameError::Oversized.message(), "oversized");
}
