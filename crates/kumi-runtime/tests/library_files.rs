//! Live files the library reads, plus differential parser cases.
use base64::{engine::general_purpose::STANDARD, Engine};
use kumi_common::js::json::stringify;
use kumi_runtime::library::{
    presets::{plugin_preset_facts, read_live_preset, read_max_device},
    sets::{device_name, read_set, time_signature},
    xml::{attribute, scan_tags, scan_xml_file, xml_head, ScanOptions, TagHandler, XmlError},
};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write;
fn normalized<T: Serialize>(value: T) -> Value {
    serde_json::from_str(&stringify(&serde_json::to_value(value).unwrap())).unwrap()
}
fn reference() -> Value {
    serde_json::from_str(include_str!("support/library-files-oracle.json")).unwrap()
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
