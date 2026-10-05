use memchr::memchr_iter;

// One MCP message as large as a string can be: V8 stops strings at about 512 MiB, and a record becomes
// one string when it is decoded. Big Sets make big messages; the bound is JavaScript's, not the Set's.
pub const MAX_FRAME_BYTES: usize = 500 * 1024 * 1024;

/// Why a record could not be read: the TypeScript `message` of an error event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    InvalidUtf8,
    Oversized,
}

impl FrameError {
    /// The message as the TypeScript spelled it: `"invalid-utf8"` or `"oversized"`.
    pub fn message(self) -> &'static str {
        match self {
            FrameError::InvalidUtf8 => "invalid-utf8",
            FrameError::Oversized => "oversized",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameEvent {
    Record(String),
    Error(FrameError),
}

/// Incremental JSON-lines framing. Retains at most one bounded record.
#[derive(Debug)]
pub struct NdjsonFramer {
    max_bytes: usize,
    retained: Vec<u8>,
    discarding: bool,
}

impl Default for NdjsonFramer {
    fn default() -> Self {
        Self::new()
    }
}

impl NdjsonFramer {
    /// A framer bounded at [`MAX_FRAME_BYTES`].
    pub fn new() -> Self {
        Self::with_max_bytes(MAX_FRAME_BYTES)
    }

    /// `maxBytes` bounds one record (MAX_FRAME_BYTES unless a test asks for less).
    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self { max_bytes, retained: Vec::new(), discarding: false }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<FrameEvent> {
        let mut events = Vec::new();
        let mut start = 0;
        for index in memchr_iter(10, chunk) {
            let part = &chunk[start..index];
            start = index + 1;
            if self.discarding {
                self.discarding = false;
                self.clear();
                events.push(FrameEvent::Error(FrameError::Oversized));
                continue;
            }
            let record_part = match part.split_last() {
                Some((13, head)) => head,
                _ => part,
            };
            if !self.append(record_part) {
                self.discarding = false;
                self.clear();
                events.push(FrameEvent::Error(FrameError::Oversized));
            } else {
                events.push(self.emit_record());
            }
        }
        if start < chunk.len() && !self.discarding {
            self.append(&chunk[start..]);
        }
        events
    }

    pub fn end(&mut self) -> Vec<FrameEvent> {
        if self.discarding {
            self.discarding = false;
            self.clear();
            return vec![FrameEvent::Error(FrameError::Oversized)];
        }
        if self.retained.is_empty() {
            return Vec::new();
        }
        vec![self.emit_record()]
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained.len()
    }

    fn append(&mut self, part: &[u8]) -> bool {
        if part.is_empty() {
            return true;
        }
        if self.retained.len() + part.len() > self.max_bytes {
            self.clear();
            self.discarding = true;
            return false;
        }
        self.retained.extend_from_slice(part);
        true
    }

    /// Decodes the retained bytes as `TextDecoder("utf-8", { fatal: true })` did: invalid UTF-8 is an
    /// error, and a leading byte order mark is not part of the record.
    fn emit_record(&mut self) -> FrameEvent {
        let bytes = std::mem::take(&mut self.retained);
        match String::from_utf8(bytes) {
            Ok(mut text) => {
                if text.starts_with('\u{FEFF}') {
                    text.drain(..'\u{FEFF}'.len_utf8());
                }
                FrameEvent::Record(text)
            }
            Err(_) => FrameEvent::Error(FrameError::InvalidUtf8),
        }
    }

    fn clear(&mut self) {
        self.retained = Vec::new();
    }
}
