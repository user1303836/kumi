kumi_testkit::fixture_paths!();
use fixture_paths::{map_strings, native_path, normalize_root};
use serde_json::json;
use std::path::Path;

#[test]
fn fixture_paths_preserve_json_escapes_and_normalize_only_known_paths() {
    for root in [r"C:\Users\runner\Temp\fixture", r"\\?\C:\Users\runner\Temp\fixture", r"\\?\UNC\server\share\fixture", "/tmp/fixture"] {
        let native = native_path(Path::new(root)).to_string_lossy().into_owned();
        let fixture = json!({"path":"$root/Nested/My sample.wav","message":"literal \\n","json":r#"{"path":"$root/Nested/My sample.wav","literal":"\\n"}"#});
        let expanded = map_strings(&fixture, &|text| text.replace("$root", &native));
        let wire = serde_json::to_string(&expanded).unwrap();
        let decoded: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded, expanded);
        assert_eq!(map_strings(&expanded, &|text| normalize_root(text, root, "$root")), fixture);
    }
    assert_eq!(normalize_root(r"C:\fixture\Nested\file.wav", r"C:\fixture", "$root"), "$root/Nested/file.wav");
    assert_eq!(normalize_root(r"\\?\C:\fixture\Nested\file.wav", r"C:\fixture", "$root"), "$root/Nested/file.wav");
    assert_eq!(normalize_root(r"\\?\UNC\server\share\fixture\file.wav", r"\\server\share\fixture", "$root"), "$root/file.wav");
    assert_eq!(
        normalize_root(r"literal \n, open 'C:\fixture\Nested\file.wav', keep \t", r"C:\fixture", "$root"),
        r"literal \n, open '$root/Nested/file.wav', keep \t"
    );
    assert_eq!(normalize_root(r"/tmp/fixture/valid\filename", "/tmp/fixture", "$root"), r"$root/valid\filename");
    assert_eq!(normalize_root(r"unrelated C:\other\file", r"C:\fixture", "$root"), r"unrelated C:\other\file");
}
