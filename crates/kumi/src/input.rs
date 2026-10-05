use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Node's "data" listener: a chunk of bytes.
pub type ByteListener = Rc<dyn Fn(&[u8])>;
/// Node's "error" listener.
pub type ErrorListener = Rc<dyn Fn(&std::io::Error)>;
/// Puts raw mode on or off from a crash or exit path, from any thread.
pub type RawModeRestorer = Arc<dyn Fn(bool) + Send + Sync>;

/// A terminal's input: Node's `Readable & { isTTY?, isRaw?, setRawMode? }`. Bytes flow to the
/// listener given to `resume` until `pause`; "end" and "error" have listeners of their own.
pub trait TerminalInput {
    fn is_tty(&self) -> bool {
        false
    }
    fn is_raw(&self) -> bool {
        false
    }
    /// Node's `setRawMode`, where the input has one; nothing happens where it hasn't.
    fn set_raw_mode(&self, enabled: bool) -> std::io::Result<()> {
        let _ = enabled;
        Ok(())
    }
    /// Node's `on("data", listener)` then `resume()`.
    fn resume(&self, listener: ByteListener);
    /// Node's `removeListener("data")` then `pause()`.
    fn pause(&self);
    /// Node's `once("end", listener)`: the input is finished.
    fn on_end(&self, listener: Rc<dyn Fn()>) {
        let _ = listener;
    }
    /// Node's `on("error", listener)`.
    fn on_error(&self, listener: ErrorListener) {
        let _ = listener;
    }
    /// Puts raw mode back from a crash or exit path, from any thread; `None` where there's no raw mode.
    fn emergency_raw_mode(&self) -> Option<RawModeRestorer> {
        None
    }
}

/// Node's `StringDecoder("utf8")`: text from byte chunks that may split a character between them.
#[derive(Debug, Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn new() -> Utf8Decoder {
        Utf8Decoder::default()
    }

    /// The complete characters so far; a character cut at the end waits for the next chunk.
    pub fn write(&mut self, bytes: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let mut out = String::new();
        let mut rest: &[u8] = &data;
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(std::str::from_utf8(&rest[..valid]).expect("valid up to here"));
                    match error.error_len() {
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            rest = &rest[valid + bad..];
                        }
                        None => {
                            self.pending = rest[valid..].to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// What's left at the end of the input: a replacement character for a character cut short.
    pub fn end(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            "\u{FFFD}".to_string()
        }
    }
}

/// Node 24's batched-paste fast path can lose wrapped cursor rows. Deliver complete
/// Unicode code points as distinct input chunks, retaining readline's normal editor.
pub struct KeyInput {
    source: Rc<dyn TerminalInput>,
    decoder: Rc<RefCell<Utf8Decoder>>,
    listener: Rc<RefCell<Option<ByteListener>>>,
}

impl KeyInput {
    pub fn new(source: Rc<dyn TerminalInput>) -> KeyInput {
        KeyInput { source, decoder: Rc::new(RefCell::new(Utf8Decoder::new())), listener: Rc::new(RefCell::new(None)) }
    }
}

impl TerminalInput for KeyInput {
    fn is_tty(&self) -> bool {
        true
    }

    fn is_raw(&self) -> bool {
        self.source.is_raw()
    }

    fn set_raw_mode(&self, enabled: bool) -> std::io::Result<()> {
        self.source.set_raw_mode(enabled)
    }

    fn resume(&self, listener: ByteListener) {
        *self.listener.borrow_mut() = Some(listener.clone());
        let decoder = Rc::clone(&self.decoder);
        self.source.resume(Rc::new(move |chunk| {
            let text = decoder.borrow_mut().write(chunk);
            let mut buffer = [0u8; 4];
            for character in text.chars() {
                listener(character.encode_utf8(&mut buffer).as_bytes());
            }
        }));
    }

    fn pause(&self) {
        self.listener.borrow_mut().take();
        self.source.pause();
    }

    fn on_end(&self, listener: Rc<dyn Fn()>) {
        let decoder = Rc::clone(&self.decoder);
        let data = self.listener.clone();
        self.source.on_end(Rc::new(move || {
            let tail = decoder.borrow_mut().end();
            if let Some(data) = data.borrow().clone() {
                for character in tail.chars() {
                    data(character.to_string().as_bytes());
                }
            }
            listener();
        }));
    }

    fn on_error(&self, listener: ErrorListener) {
        self.source.on_error(listener);
    }

    fn emergency_raw_mode(&self) -> Option<RawModeRestorer> {
        self.source.emergency_raw_mode()
    }
}

impl Drop for KeyInput {
    fn drop(&mut self) {
        self.source.pause();
    }
}
