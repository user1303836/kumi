//! Expand recorded library paths after JSON decoding, using the runner's separator.
kumi_testkit::fixture_paths!();
use serde_json::Value;
use std::path::Path;

pub fn load(source: &str, root: &Path) -> Value {
    let fixture: Value = serde_json::from_str(source).unwrap();
    let root = root.to_str().unwrap();
    // Fixtures contain whole path strings and one "couldn't listen to PATH: reason"
    // message. The colon ends that path; surrounding prose is kept byte-for-byte.
    let paths = regex::Regex::new(r"<ROOT>[^:\r\n]*").unwrap();
    fixture_paths::map_strings(&fixture, &|text| {
        paths
            .replace_all(text, |capture: &regex::Captures<'_>| {
                let suffix = capture[0].strip_prefix("<ROOT>").unwrap();
                format!("{root}{}", suffix.replace('/', std::path::MAIN_SEPARATOR_STR))
            })
            .into_owned()
    })
}
