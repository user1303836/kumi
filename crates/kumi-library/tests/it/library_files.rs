//! Live files the library reads, plus differential parser cases.
use base64::{engine::general_purpose::STANDARD, Engine};
use kumi_common::js::json::stringify;
use kumi_library::{
    presets::{plugin_preset_facts, read_live_preset, read_max_device},
    sets::{device_name, read_set, time_signature},
    xml::{attribute, decode, scan_tags, scan_xml_file, scan_xml_stream, xml_head, ScanOptions, TagHandler, XmlError},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write;
fn normalized<T: Serialize>(value: T) -> Value {
    serde_json::from_str(&stringify(&serde_json::to_value(value).unwrap())).unwrap()
}
fn reference() -> Value {
    serde_json::from_str(&kumi_testkit::oracle("kumi-library/library-files-oracle.json")).unwrap()
}
#[tokio::test]
async fn sets_presets_and_max_headers_match_typescript() {
    let root = tempfile::tempdir().unwrap();
    let fixture = reference();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let path = root.path().join(name);
        std::fs::write(&path, STANDARD.decode(case["body"].as_str().unwrap()).unwrap()).unwrap();
        let path = path.to_str().unwrap();
        let result = match case["kind"].as_str().unwrap() {
            "set" => read_set(path, None).await.map(normalized).map_err(|e| e.to_string()),
            "max" => read_max_device(path).await.map(normalized).map_err(|e| e.to_string()),
            _ => read_live_preset(path).await.map(normalized).map_err(|e| e.to_string()),
        };
        if let Some(error) = case.get("error") {
            assert_eq!(result.unwrap_err(), error.as_str().unwrap(), "{name}");
        } else {
            assert_eq!(result.unwrap(), case["expected"], "{name}");
        }
    }
    assert!(read_set(root.path().join("missing.als").to_str().unwrap(), None).await.is_err());
}
#[test]
fn names_signatures_and_plugin_folders_match_typescript() {
    let fixture = reference();
    for case in fixture["names"].as_array().unwrap() {
        assert_eq!(device_name(case["tag"].as_str().unwrap()), case["name"]);
    }
    for case in fixture["signatures"].as_array().unwrap() {
        assert_eq!(json!(time_signature(case["value"].as_f64().unwrap())), case["expected"]);
    }
    for case in fixture["pluginPaths"].as_array().unwrap() {
        assert_eq!(normalized(plugin_preset_facts(case["path"].as_str().unwrap())), case["expected"]);
    }
    assert_eq!(time_signature(f64::INFINITY), None);
}
#[derive(Default)]
struct Handler {
    seen: Vec<String>,
    stop: bool,
}
impl TagHandler for Handler {
    fn open(&mut self, name: &str, attrs: &str, _: bool) {
        self.seen.push(format!("{name}:{}", attribute(attrs, "Value").or_else(|| attribute(attrs, "Name")).unwrap_or_default()));
    }
    fn close(&mut self, name: &str) {
        self.seen.push(format!("/{name}"));
    }
    fn stop(&mut self) -> bool {
        self.stop
    }
}
#[test]
fn streaming_tags_keep_unfinished_tags_quotes_and_entities() {
    let text = "<?xml version=\"1.0\"?><!-- note --><A><B Value='1 > 0' /><C UserName=\"x\" Name=\"R&amp;B &#x263A;\"></C><D";
    let mut handler = Handler::default();
    let end = scan_tags(text, &mut handler);
    assert_eq!(handler.seen, vec!["A:", "B:1 > 0", "/B", "C:R&B ☺", "/C"]);
    assert_eq!(&text[end..], "<D");
    assert_eq!(
        attribute("UserName='wrong' Name='&lt;&gt;&quot;&apos;&#65;&#0;&#x110000;&other;'", "Name").as_deref(),
        Some("<>\"'A&other;")
    );
    assert_eq!(attribute("Name = 'no'", "Name"), None);
    assert_eq!(attribute("Name='unfinished", "Name"), None);
    let mut handler = Handler::default();
    assert_eq!(scan_tags("<A/><![CDATA[<NotA/>]]><!--<AlsoNot/>--><B/>", &mut handler), 44);
    assert_eq!(handler.seen, vec!["A:", "/A", "B:", "/B"]);
}
fn gzip(text: &str) -> Vec<u8> {
    let mut zip = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
    zip.write_all(text.as_bytes()).unwrap();
    zip.finish().unwrap()
}
/// Elements' values as text, whichever pieces a stream cuts them into.
#[derive(Default)]
struct Texts {
    text: String,
    values: Vec<(String, String)>,
}
impl TagHandler for Texts {
    fn open(&mut self, _: &str, _: &str, _: bool) {
        self.text.clear();
    }
    fn close(&mut self, name: &str) {
        self.values.push((name.into(), decode(self.text.trim())));
        self.text.clear();
    }
    fn text(&mut self, text: &str) {
        self.text.push_str(text);
    }
}
/// A stream that gives `size` bytes at a time.
struct Pieces<'a>(&'a [u8], usize);
impl std::io::Read for Pieces<'_> {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        let count = self.0.len().min(self.1).min(into.len());
        into[..count].copy_from_slice(&self.0[..count]);
        self.0 = &self.0[count..];
        Ok(count)
    }
}
#[test]
fn a_stream_gives_its_text_whatever_cuts_it_within_the_same_bounds() {
    let xml = "<?xml version=\"1.0\"?><Song><!-- not this --><Name>Caf\u{e9} &amp; Bar ☺</Name><Data><![CDATA[not this]]></Data><Empty/><Tempo> 120 </Tempo></Song>";
    let want = [("Name", "Caf\u{e9} & Bar ☺"), ("Data", ""), ("Empty", ""), ("Tempo", "120"), ("Song", "")];
    for size in [1, 2, 3, 7, 1024] {
        let mut texts = Texts::default();
        scan_xml_stream(Pieces(xml.as_bytes(), size), &mut texts, ScanOptions::default()).unwrap();
        let values: Vec<_> = texts.values.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        assert_eq!(values, want, "{size} bytes at a time");
    }
    let bound = ScanOptions { max_bytes: Some(10), ..Default::default() };
    assert!(matches!(scan_xml_stream(xml.as_bytes(), &mut Texts::default(), bound), Err(XmlError::TooLarge)));
    let signal = kumi_common::abort::Signal::new();
    signal.cancel();
    let stopped = ScanOptions { signal: Some(signal), ..Default::default() };
    assert!(matches!(scan_xml_stream(xml.as_bytes(), &mut Texts::default(), stopped), Err(XmlError::Aborted(_))));
    // A CDATA section or a comment past a tag's bound, whatever cuts it: nothing holds it, and what follows it reads.
    for (open, close) in [("<![CDATA[", "]]>"), ("<!--", "-->")] {
        let long = format!("<A>1</A>{open}{}☺]{close}<B>2</B>", "x]".repeat(5 * 1024 * 1024));
        for size in [64 * 1024 + 1, 256 * 1024] {
            let mut texts = Texts::default();
            scan_xml_stream(Pieces(long.as_bytes(), size), &mut texts, ScanOptions::default()).unwrap();
            let values: Vec<_> = texts.values.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            assert_eq!(values, [("A", "1"), ("B", "2")], "{open} {size} bytes at a time");
        }
        // The closer cut by every small piece.
        let short = format!("<A>1</A>{open}abc{close}<B>2</B>");
        let mut texts = Texts::default();
        scan_xml_stream(Pieces(short.as_bytes(), 1), &mut texts, ScanOptions::default()).unwrap();
        assert_eq!(texts.values.len(), 2, "{open}");
    }
    // Its read ends as the handler says.
    let mut handler = Handler { stop: true, ..Default::default() };
    let first = format!("<First/>{}<Last/>", " ".repeat(700_000));
    scan_xml_stream(first.as_bytes(), &mut handler, ScanOptions::default()).unwrap();
    assert_eq!(handler.seen, vec!["First:", "/First"]);
}
#[tokio::test]
async fn streamed_files_limit_size_stop_cancel_and_keep_split_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.als");
    let prefix = " ".repeat(256 * 1024 - 11);
    let text = format!("{prefix}<X Value='☺'/><Y/>");
    std::fs::write(&path, &text).unwrap();
    let mut handler = Handler::default();
    scan_xml_file(&path, &mut handler, ScanOptions::default()).await.unwrap();
    assert_eq!(handler.seen, vec!["X:☺", "/X", "Y:", "/Y"]);
    assert_eq!(xml_head(&path, 3).await.unwrap(), "   ");
    std::fs::write(&path, gzip(&text)).unwrap();
    let mut handler = Handler::default();
    scan_xml_file(&path, &mut handler, ScanOptions::default()).await.unwrap();
    assert_eq!(handler.seen, vec!["X:☺", "/X", "Y:", "/Y"]);
    assert!(matches!(
        scan_xml_file(&path, &mut Handler::default(), ScanOptions { max_bytes: Some(100), signal: None }).await,
        Err(XmlError::TooLarge)
    ));
    let signal = kumi_common::abort::Signal::new();
    signal.cancel();
    assert!(matches!(
        scan_xml_file(&path, &mut Handler::default(), ScanOptions { signal: Some(signal), max_bytes: None }).await,
        Err(XmlError::Aborted(_))
    ));
    // A tag that never ends is read no further than 8 MiB of it (each piece read it again from its start).
    std::fs::write(&path, format!("<A/><B Value=\"{}", "x".repeat(9 * 1024 * 1024))).unwrap();
    let mut handler = Handler::default();
    assert!(matches!(scan_xml_file(&path, &mut handler, ScanOptions::default()).await, Err(XmlError::TagTooLong)));
    assert_eq!(handler.seen, vec!["A:", "/A"]);
    let text = format!("<First/>{}<Last/>", " ".repeat(700000));
    std::fs::write(&path, &text).unwrap();
    let mut handler = Handler { stop: true, ..Default::default() };
    scan_xml_file(&path, &mut handler, ScanOptions::default()).await.unwrap();
    assert_eq!(handler.seen, vec!["First:", "/First"]);
}
#[tokio::test]
async fn relative_samples_require_existing_files_and_default_scales_stay_unset() {
    let dir = tempfile::tempdir().unwrap();
    let sample = dir.path().join("used.wav");
    std::fs::write(&sample, "").unwrap();
    let path = dir.path().join("sample.als");
    let xml = r#"<Ableton><LiveSet><Tracks><AudioTrack Id="1"><Name><EffectiveName Value="Audio"/></Name><Freeze Value="true"/><DeviceChain><MainSequencer><ClipSlotList><AudioClip><SampleRef><FileRef><RelativePath Value="used.wav"/></FileRef></SampleRef><SampleRef><FileRef><RelativePath Value="missing.wav"/></FileRef></SampleRef><SampleRef><FileRef><Path Value="/absolute.wav"/><RelativePath Value="used.wav"/></FileRef></SampleRef></AudioClip></ClipSlotList></MainSequencer></DeviceChain></AudioTrack></Tracks><ScaleInformation><Root Value="9"/><Name Value="Minor"/></ScaleInformation><InKey Value="false"/></LiveSet></Ableton>"#;
    std::fs::write(&path, xml).unwrap();
    let set = read_set(path.to_str().unwrap(), None).await.unwrap();
    assert_eq!(set.tracks[0].samples, vec![sample.to_string_lossy().into_owned(), "/absolute.wav".into()]);
    assert_eq!(set.tracks[0].frozen, Some(true));
    assert_eq!(set.key, None);
}
