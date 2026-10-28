//! Keep readable format data and standard presets in the repository. The binary
//! embeds compact JSON and compressed initialized states, once per source file.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use flate2::{write::ZlibEncoder, Compression};
use serde_json::Value;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest directory")).join("../../plugin-formats");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    println!("cargo:rerun-if-changed={}", root.display());
    let mut products: Vec<_> = std::fs::read_dir(&root)
        .expect("plugin-formats directory")
        .map(|entry| entry.expect("format directory entry").path())
        .filter(|path| path.join("plugin.json").is_file())
        .collect();
    products.sort();
    let mut resources = BTreeMap::new();
    let mut catalog = String::new();
    let mut entries = Vec::new();
    let mut templates = String::from("pub const COMPRESSED_TEMPLATES: &[(&str, &[u8])] = &[\n");
    for product in products {
        let info: Value = serde_json::from_slice(&std::fs::read(product.join("plugin.json")).expect("plugin.json")).expect("plugin JSON");
        let id = info["plugin"].as_str().expect("plugin ID");
        let format = info["formats"][0].as_str().expect("format ID");
        let source = info["format_source"].as_str().map(str::to_owned).unwrap_or_else(|| format!("{id}/{format}"));
        assert!(Path::new(&source).components().all(|component| matches!(component, Component::Normal(_))), "local format source");
        let mut names = Vec::new();
        for relative in [format!("{id}/plugin.json"), format!("{source}/structure.json"), format!("{source}/verified.json")] {
            let next = resources.len();
            let index = *resources.entry(relative.clone()).or_insert(next);
            if index == next {
                let value: Value =
                    serde_json::from_slice(&std::fs::read(root.join(&relative)).expect("format JSON file")).expect("format JSON");
                let filename = format!("plugin-format-{index}.min.json");
                std::fs::write(out.join(&filename), serde_json::to_vec(&value).expect("compact format JSON"))
                    .expect("write compact format");
                catalog.push_str(&format!("const FORMAT_{index}: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/{filename}\"));\n"));
            }
            names.push(format!("FORMAT_{index}"));
        }
        entries.push(format!("    ({id:?}, {}, {}, {}),", names[0], names[1], names[2]));
        // Ozone constructs its selected suite/module state from its own typed
        // defaults; its old example fixtures are not creation templates.
        if id == "ozone-12" {
            continue;
        }
        let fixture = ["init.vstpreset", "empty.vstpreset"]
            .into_iter()
            .map(|name| root.join(&source).join("fixtures").join(name))
            .find(|path| path.is_file());
        if let Some(fixture) = fixture {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
            encoder.write_all(&std::fs::read(&fixture).expect("initialized VST3 fixture")).expect("compress initialized state");
            let filename = format!("plugin-init-{id}.zlib");
            std::fs::write(out.join(&filename), encoder.finish().expect("finish initialized state")).expect("write compressed state");
            templates.push_str(&format!("    ({id:?}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{filename}\"))),\n"));
        }
    }
    catalog.push_str("pub const BUILTIN_FILES: &[(&str, &str, &str, &str)] = &[\n");
    catalog.push_str(&entries.join("\n"));
    catalog.push_str("\n];\n");
    templates.push_str("];\n");
    std::fs::write(out.join("plugin_formats.rs"), catalog).expect("write format registry");
    std::fs::write(out.join("plugin_templates.rs"), templates).expect("write template registry");
}
