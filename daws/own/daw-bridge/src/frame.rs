//! NDJSON framing: one JSON object per line, with a byte limit the caller may change between lines (tight before auth).

/// The longest line accepted before a connection authenticates (without its newline).
pub const MAX_PREAUTH_LINE_BYTES: usize = 1024;
/// The longest line accepted after it does.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Why a line could not be read. Either one ends the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    InvalidUtf8,
    Oversized,
}

impl FrameError {
    pub fn code(self) -> &'static str {
        match self {
            FrameError::InvalidUtf8 => "invalid-utf8",
            FrameError::Oversized => "oversized",
        }
    }
}

/// Splits a byte stream into lines. Feed it what the socket read, then take lines with [`Framer::next_line`] until it gives
/// none; a trailing `\r` is not part of a line and empty lines are skipped, though they count toward the limit of the
/// line after them (so a stream of empty lines is bounded too). It holds at most one line plus one read.
#[derive(Debug)]
pub struct Framer {
    max_bytes: usize,
    buffer: Vec<u8>,
    // Where the next line starts in `buffer`: taking a line moves it, and the next feed drops what's behind it, so a
    // read of many short lines costs one pass.
    start: usize,
    // How far past `start` `buffer` is known to hold no newline, so a large line arriving in many reads is scanned once.
    scanned: usize,
    // Bytes of the empty lines skipped since the last line, newlines included.
    blank: usize,
    failed: Option<FrameError>,
}

impl Framer {
    pub fn new(max_bytes: usize) -> Self {
        Self { max_bytes, buffer: Vec::new(), start: 0, scanned: 0, blank: 0, failed: None }
    }

    /// The limit for the next lines (the bytes already fed are measured against it too).
    pub fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes;
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if self.failed.is_none() {
            self.buffer.drain(..self.start);
            self.start = 0;
            self.buffer.extend_from_slice(bytes);
        }
    }

    /// The next whole line, an error (after which it gives the same error forever), or None until more is fed. A line
    /// and the empty lines before it may be `max_bytes` long together, the line's own `\r` and `\n` left out.
    pub fn next_line(&mut self) -> Option<Result<String, FrameError>> {
        if let Some(error) = self.failed {
            return Some(Err(error));
        }
        loop {
            let from = self.start + self.scanned;
            let Some(found) = memchr::memchr(b'\n', &self.buffer[from..]) else {
                self.scanned = self.buffer.len() - self.start;
                // A carriage return may be the end of a line whose newline hasn't arrived yet.
                let held = self.scanned - usize::from(self.scanned > 0 && self.buffer.last() == Some(&b'\r'));
                if self.blank + held > self.max_bytes {
                    return Some(Err(self.fail(FrameError::Oversized)));
                }
                return None;
            };
            let newline = from + found;
            let begin = self.start;
            let end = if newline > begin && self.buffer[newline - 1] == b'\r' { newline - 1 } else { newline };
            self.start = newline + 1;
            self.scanned = 0;
            if end == begin {
                self.blank += newline + 1 - begin;
                if self.blank > self.max_bytes {
                    return Some(Err(self.fail(FrameError::Oversized)));
                }
                continue;
            }
            if self.blank + (end - begin) > self.max_bytes {
                return Some(Err(self.fail(FrameError::Oversized)));
            }
            self.blank = 0;
            let line = self.buffer[begin..end].to_vec();
            return Some(String::from_utf8(line).map_err(|_| self.fail(FrameError::InvalidUtf8)));
        }
    }

    /// Bytes fed that are not yet part of a whole line.
    pub fn pending_bytes(&self) -> usize {
        self.buffer.len() - self.start
    }

    fn fail(&mut self, error: FrameError) -> FrameError {
        self.failed = Some(error);
        self.buffer = Vec::new();
        self.start = 0;
        error
    }
}

/// Whether a connection's first bytes may be Kumi's: the very first byte must open a JSON object. Anything else (an
/// HTTP request line from a browser or a DNS-rebinding page, a byte order mark, whitespace) is closed without a reply.
pub fn first_byte_allowed(byte: u8) -> bool {
    byte == b'{'
}

/// Whether a line is an HTTP request line or the HTTP/2 preface: the case [`first_byte_allowed`] exists for, named
/// for logs.
pub fn looks_like_http(line: &[u8]) -> bool {
    const METHODS: [&[u8]; 9] = [b"GET ", b"POST ", b"PUT ", b"HEAD ", b"DELETE ", b"OPTIONS ", b"PATCH ", b"CONNECT ", b"TRACE "];
    METHODS.iter().any(|method| line.starts_with(method)) || line.starts_with(b"PRI * HTTP/2")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(framer: &mut Framer) -> Vec<Result<String, FrameError>> {
        let mut out = vec![];
        while let Some(line) = framer.next_line() {
            let stop = line.is_err();
            out.push(line);
            if stop {
                break;
            }
        }
        out
    }

    #[test]
    fn lines_split_across_reads_and_carriage_returns_and_blank_lines() {
        let mut framer = Framer::new(64);
        framer.feed(b"{\"a\":1}\r\n\n{\"b\"");
        assert_eq!(lines(&mut framer), vec![Ok("{\"a\":1}".to_string())]);
        framer.feed(b":2}\n\r\n");
        assert_eq!(lines(&mut framer), vec![Ok("{\"b\":2}".to_string())]);
        assert_eq!(framer.pending_bytes(), 0);
    }

    #[test]
    fn the_limit_holds_without_a_newline_and_can_change_between_lines() {
        let mut framer = Framer::new(8);
        framer.feed(b"12345678\n123456789");
        assert_eq!(lines(&mut framer), vec![Ok("12345678".to_string()), Err(FrameError::Oversized)]);
        assert_eq!(framer.next_line(), Some(Err(FrameError::Oversized)));

        let mut framer = Framer::new(4);
        framer.feed(b"1234\n123456\n");
        assert_eq!(framer.next_line(), Some(Ok("1234".to_string())));
        framer.set_max_bytes(6);
        assert_eq!(framer.next_line(), Some(Ok("123456".to_string())));
    }

    #[test]
    fn empty_lines_count_toward_the_next_lines_limit() {
        let mut framer = Framer::new(8);
        framer.feed(b"\n\r\n{\"a\":1}\n");
        assert_eq!(lines(&mut framer), vec![Err(FrameError::Oversized)]);
        let mut framer = Framer::new(8);
        framer.feed(b"\n{\"a\":1}\n\n\n{}\n");
        assert_eq!(lines(&mut framer), vec![Ok("{\"a\":1}".to_string()), Ok("{}".to_string())]);
        let mut framer = Framer::new(8);
        framer.feed(&[b'\n'; 8]);
        assert_eq!(lines(&mut framer), vec![]);
        framer.feed(b"\n");
        assert_eq!(lines(&mut framer), vec![Err(FrameError::Oversized)]);
        let mut framer = Framer::new(8);
        framer.feed(b"\n\n\n\n\n{\"a\":");
        assert_eq!(lines(&mut framer), vec![Err(FrameError::Oversized)]);
    }

    #[test]
    fn many_short_lines_in_one_read_take_one_pass() {
        let mut framer = Framer::new(MAX_FRAME_BYTES);
        framer.feed(&b"{}\n\n".repeat(200_000));
        let started = std::time::Instant::now();
        assert_eq!(std::iter::from_fn(|| framer.next_line()).count(), 200_000);
        // Draining each line from the front made this quadratic: seconds, not milliseconds.
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
        assert_eq!(framer.pending_bytes(), 0);
    }

    #[test]
    fn invalid_utf8_ends_the_stream() {
        let mut framer = Framer::new(64);
        framer.feed(b"\xff\xfe\n{}\n");
        assert_eq!(lines(&mut framer), vec![Err(FrameError::InvalidUtf8)]);
    }

    #[test]
    fn http_first_lines_are_recognized() {
        assert!(looks_like_http(b"GET / HTTP/1.1"));
        assert!(looks_like_http(b"PRI * HTTP/2.0"));
        assert!(!looks_like_http(b"{\"type\":\"hello\"}"));
        assert!(!first_byte_allowed(b'G') && !first_byte_allowed(0xEF) && !first_byte_allowed(b' ') && first_byte_allowed(b'{'));
    }
}
