//! Source terminal input/sanitizer cases and complete source text presentation oracles.
use kumi::{
    input::{ByteListener, KeyInput, TerminalInput},
    text::*,
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[test]
fn streaming_controls_credentials_web_and_library_words_match_the_source() {
    let fixture: Value = serde_json::from_str(include_str!("support/text/reference.json")).unwrap();
    for c in fixture["cases"].as_array().unwrap() {
        let mut text = StreamingText::new(&serde_json::from_value::<Vec<String>>(c["secrets"].clone()).unwrap());
        let output: Vec<_> = c["chunks"].as_array().unwrap().iter().map(|chunk| text.push(chunk.as_str().unwrap())).collect();
        assert_eq!(serde_json::to_value(output).unwrap(), c["output"]);
        assert_eq!(text.finish(), c["finish"]);
    }
    for c in fixture["events"].as_array().unwrap() {
        let event = serde_json::from_value(c["event"].clone()).unwrap();
        let words = web_words(&event, &|s, n| kumi_common::js::string::head(&sanitize_text(s, &[]), n));
        assert_eq!(serde_json::json!({"lead":words.lead,"title":words.title,"detail":words.detail}), c["expected"]);
    }
    for c in fixture["library"].as_array().unwrap() {
        let status: Option<kumi_runtime::core::contracts::LibraryStatus> = serde_json::from_value(c["status"].clone()).unwrap();
        assert_eq!(serde_json::to_value(library_line(status.as_ref())).unwrap(), c["expected"]);
    }
    assert_eq!(sanitize_text("bad\r\u{8}\0\u{9b}31m text\u{202e}", &[]), "bad text");
}
#[test]
fn interrupted_secret_prefixes_and_unterminated_sequences_are_not_replayed() {
    let mut text = StreamingText::new(&["private-token".into()]);
    assert_eq!(text.push("private-"), "");
    text.discard();
    assert_eq!(text.push("next"), "next");
    assert_eq!(text.finish(), "");
    assert_eq!(text.push("\x1b]0;hidden"), "");
    text.discard();
    assert_eq!(text.push("fresh"), "fresh");
}
struct Input {
    data: RefCell<Option<ByteListener>>,
    end: RefCell<Option<Rc<dyn Fn()>>>,
    paused: Cell<bool>,
    raw: Cell<bool>,
}
impl TerminalInput for Input {
    fn is_raw(&self) -> bool {
        self.raw.get()
    }
    fn set_raw_mode(&self, raw: bool) -> std::io::Result<()> {
        self.raw.set(raw);
        Ok(())
    }
    fn pause(&self) {
        self.paused.set(true);
        self.data.borrow_mut().take();
    }
    fn resume(&self, listener: ByteListener) {
        *self.data.borrow_mut() = Some(listener);
    }
    fn on_end(&self, listener: Rc<dyn Fn()>) {
        *self.end.borrow_mut() = Some(listener);
    }
}
#[test]
fn tty_input_keeps_split_utf8_as_complete_separate_codepoints() {
    let bytes = "a猫🎹b".as_bytes();
    let source = Rc::new(Input { data: RefCell::new(None), end: RefCell::new(None), paused: Cell::new(false), raw: Cell::new(false) });
    let keys = KeyInput::new(source.clone());
    assert!(keys.is_tty());
    keys.set_raw_mode(true).unwrap();
    assert!(keys.is_raw());
    let chunks = Rc::new(RefCell::new(Vec::new()));
    keys.resume(Rc::new({
        let chunks = chunks.clone();
        move |chunk| chunks.borrow_mut().push(String::from_utf8(chunk.to_vec()).unwrap())
    }));
    let ended = Rc::new(Cell::new(false));
    keys.on_end(Rc::new({
        let ended = ended.clone();
        move || ended.set(true)
    }));
    for chunk in [&bytes[..3], &bytes[3..], &[0xe2, 0x82]] {
        let data = source.data.borrow().clone().unwrap();
        data(chunk);
    }
    source.end.borrow().as_ref().unwrap()();
    assert!(ended.get());
    assert_eq!(*chunks.borrow(), ["a", "猫", "🎹", "b", "�"]);
    drop(keys);
    assert!(source.paused.get());
    assert!(source.data.borrow().is_none());
}

#[test]
fn newly_saved_secrets_redact_streamed_chunks_without_resetting_pending_controls() {
    let mut text = StreamingText::new(&[]);
    assert_eq!(text.push("hello "), "hello ");
    assert_eq!(text.push("\x1b]0;hidden"), "");
    text.add_secret("new-private-key".into());
    assert_eq!(text.push("\x07new-private-"), "");
    assert_eq!(text.push("key appeared"), "[redacted] appeared");
    assert_eq!(text.finish(), "");
}
