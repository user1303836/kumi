//! The bundled models catalog, compacted: data/models-catalog.json stays indented in git (for its diffs, as
//! scripts/models-catalog.py writes it), and the binary carries it without the whitespace, a third smaller.
fn main() {
    let source = "data/models-catalog.json";
    println!("cargo:rerun-if-changed={source}");
    let text = std::fs::read_to_string(source).expect("the bundled catalog is read");
    let value: serde_json::Value = serde_json::from_str(&text).expect("the bundled catalog is JSON");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("models-catalog.min.json");
    std::fs::write(out, serde_json::to_string(&value).expect("the catalog serializes")).expect("the compact catalog is written");
}
