//! Keep readable format data and standard presets in the repository. The binary
//! embeds compact JSON and compressed initialized states, once per source file.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use flate2::{write::ZlibEncoder, Compression};
use serde_json::Value;

const MAX_TEMPLATE_BYTES: usize = 2 * 1024 * 1024;

fn embedded_file(path: &Path) -> Vec<u8> {
    println!("cargo:rerun-if-changed={}", path.display());
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest directory")).join("../../plugin-formats");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let catalog_path = root.join("catalog.json");
    let mut products: Vec<String> = serde_json::from_slice(&embedded_file(&catalog_path)).expect("plugin catalog IDs");
    products.sort();
    let mut registered = BTreeSet::new();
    let mut resources = BTreeMap::new();
    let mut catalog = String::new();
    let mut entries = Vec::new();
    let mut templates =
        format!("pub const MAX_TEMPLATE_BYTES: usize = {MAX_TEMPLATE_BYTES};\npub const COMPRESSED_TEMPLATES: &[(&str, &[u8])] = &[\n");
    for folder in products {
        assert!(
            Path::new(&folder).components().count() == 1
                && Path::new(&folder).components().all(|component| matches!(component, Component::Normal(_))),
            "catalog ID must be one product folder"
        );
        assert!(registered.insert(folder.clone()), "duplicate product in catalog");
        let product = root.join(&folder);
        let info: Value = serde_json::from_slice(&embedded_file(&product.join("plugin.json"))).expect("plugin JSON");
        let id = info["plugin"].as_str().expect("plugin ID");
        assert_eq!(product.file_name().and_then(|name| name.to_str()), Some(id), "folder name must equal plugin ID");
        let format = info["formats"][0].as_str().expect("format ID");
        let source = info["format_source"].as_str().map(str::to_owned).unwrap_or_else(|| format!("{id}/{format}"));
        assert!(Path::new(&source).components().all(|component| matches!(component, Component::Normal(_))), "local format source");
        let mut names = Vec::new();
        for relative in [format!("{id}/plugin.json"), format!("{source}/structure.json"), format!("{source}/verified.json")] {
            let next = resources.len();
            let index = *resources.entry(relative.clone()).or_insert(next);
            if index == next {
                let value: Value = serde_json::from_slice(&embedded_file(&root.join(&relative))).expect("format JSON");
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
            assert!(
                std::fs::metadata(&fixture).expect("template metadata").len() <= MAX_TEMPLATE_BYTES as u64,
                "{} exceeds the initialized-state byte limit",
                fixture.display()
            );
            let bytes = embedded_file(&fixture);
            assert!(bytes.len() <= MAX_TEMPLATE_BYTES, "{} grew beyond the initialized-state byte limit", fixture.display());
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
            encoder.write_all(&bytes).expect("compress initialized state");
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
