//! Measure Max for Live devices (and Max patchers) on this machine against Kumi's standard for Max patchers, and fold
//! what it finds into max-standard/standard.json. Each device is read where it lies, a frozen one with its own files.
//! Only counts leave the machine: how often each rule was broken in how many things it looked at, how the patchers are
//! laid out, which of Max's objects they use and how often, how their parameters are kept. No names, text or code.
//!
//! cargo run -p kumi-runtime --example max_patch_survey -- <report.json> [--into max-standard/standard.json] [--findings]
//!     <file or folder>...
//!
//! --findings prints each device's broken rules here, to read them; they never go into the report.
//!
//! With Max installed, the report also says how well Kumi's reference of Max's objects (learned from this machine's
//! Max) predicts the inlets and outlets the devices' objects were saved with.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kumi_runtime::devices::patch::catalog::catalog;
use kumi_runtime::devices::patch::check::{check_with, RULES};
use kumi_runtime::devices::patch::frozen::read_device;
use kumi_runtime::devices::patch::measure::{quantile, shape, Shape};
use kumi_runtime::devices::patch::reference;
use kumi_runtime::devices::patch::standard::standard;
use kumi_runtime::devices::patch::{Files, MaxBox, NoFiles, Patcher};
use serde_json::{json, Map, Value};

#[derive(Default)]
struct Survey {
    print_findings: bool,
    devices: usize,
    patchers: usize,
    unread: usize,
    found: BTreeMap<&'static str, usize>,
    looked_at: BTreeMap<&'static str, usize>,
    shape: Shape,
    objects: BTreeMap<String, usize>,
    colours: BTreeMap<String, BTreeMap<String, usize>>,
    faces: BTreeMap<&'static str, usize>,
    /// Max objects whose saved inlets and outlets the reference predicted, didn't, or doesn't know.
    ports: BTreeMap<&'static str, usize>,
    /// The classes it predicted wrongly, and how often.
    port_misses: BTreeMap<String, usize>,
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: max_patch_survey <report.json> [--into max-standard/standard.json] <file or folder>...");
        std::process::exit(2);
    }
    let report_path = PathBuf::from(args.remove(0));
    let into = args.iter().position(|arg| arg == "--into").map(|at| {
        args.remove(at);
        PathBuf::from(args.remove(at))
    });
    let print_findings = args.iter().any(|arg| arg == "--findings");
    args.retain(|arg| arg != "--findings");
    let mut files = Vec::new();
    for arg in &args {
        collect(Path::new(arg), &mut files);
    }
    let mut survey = Survey { print_findings, ..Survey::default() };
    for file in &files {
        survey.read(file);
    }
    let report = survey.report();
    std::fs::write(&report_path, format!("{}\n", serde_json::to_string_pretty(&report).unwrap())).expect("the report");
    eprintln!("{} devices or patchers measured ({} unread) → {}", survey.devices, survey.unread, report_path.display());
    if let Some(into) = into {
        let mut standard: Value =
            serde_json::from_str(&std::fs::read_to_string(&into).expect("the standard")).expect("the standard's JSON");
        fold(&mut standard, &report);
        std::fs::write(&into, standard_text(&standard)).expect("the standard");
        eprintln!("folded into {}", into.display());
    }
}

/// Every device and patcher under a path.
fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for entry in entries {
            collect(&entry, files);
        }
    } else if matches!(path.extension().and_then(|ext| ext.to_str()), Some("amxd" | "maxpat")) {
        files.push(path.to_path_buf());
    }
}

impl Survey {
    fn read(&mut self, file: &Path) {
        let Ok(bytes) = std::fs::read(file) else {
            self.unread += 1;
            return;
        };
        let read = if file.extension().and_then(|ext| ext.to_str()) == Some("amxd") {
            read_device(&bytes).map(|device| {
                let files: Box<dyn Files> = match device.frozen {
                    Some(frozen) => Box::new(frozen),
                    None => Box::new(NoFiles),
                };
                Patcher::read(&device.patcher, files.as_ref())
            })
        } else {
            serde_json::from_slice::<Value>(&bytes).ok().map(|value| Patcher::read(&value, &NoFiles))
        };
        let Some(patcher) = read else {
            self.unread += 1;
            return;
        };
        self.devices += 1;
        if patcher.fields.get("openinpresentation").and_then(Value::as_f64) == Some(1.0) {
            *self.faces.entry("devices opening in presentation").or_default() += 1;
        }
        let report = check_with(&patcher, standard(), reference::installed());
        if self.print_findings {
            eprintln!("{}: {} broken", file.display(), report.findings.len());
            for finding in &report.findings {
                eprintln!("  [{}] {finding}", finding.level.as_str());
            }
        }
        for (rule, count) in report.counts() {
            *self.found.entry(rule).or_default() += count;
        }
        for (rule, count) in &report.looked_at {
            *self.looked_at.entry(rule).or_default() += count;
        }
        let mut seen = std::collections::HashSet::new();
        self.walk(&patcher, &mut seen);
    }

    /// A patcher's shape, objects and colours, then those of each patcher inside it (a file used twice, once).
    fn walk(&mut self, patcher: &Patcher, seen: &mut std::collections::HashSet<String>) {
        self.patchers += 1;
        let measured = shape(patcher);
        self.shape.boxes += measured.boxes;
        self.shape.cords += measured.cords;
        self.shape.bent += measured.bent;
        self.shape.crossings += measured.crossings;
        self.shape.in_columns += measured.in_columns;
        self.shape.down_gaps.extend(measured.down_gaps);
        self.shape.side_gaps.extend(measured.side_gaps);
        for item in &patcher.boxes {
            let class = object_class(patcher, item);
            *self.objects.entry(class.clone()).or_default() += 1;
            if let Some(colour) = item.fields.get("color").and_then(Value::as_array) {
                let colour: Vec<String> = colour.iter().filter_map(Value::as_f64).map(|part| format!("{part:.2}")).collect();
                let role = match catalog().role(item.class()) {
                    Some(role) => format!("{role:?}").to_lowercase(),
                    None => "other".into(),
                };
                *self.colours.entry(role).or_default().entry(colour.join(" ")).or_default() += 1;
            }
            match item.maxclass() {
                "jsui" => *self.faces.entry("jsui").or_default() += 1,
                "v8ui" => *self.faces.entry("v8ui").or_default() += 1,
                "bpatcher" => *self.faces.entry("bpatchers").or_default() += 1,
                "v8.codebox" => *self.faces.entry("v8.codebox").or_default() += 1,
                maxclass if maxclass.starts_with("live.") => *self.faces.entry("live objects").or_default() += 1,
                _ => {}
            }
            if matches!(item.class(), "js" | "v8") {
                *self.faces.entry("js or v8").or_default() += 1;
            }
            if let (Some(reference), false, "newobj", None) = (reference::installed(), patcher.is_gen(), item.maxclass(), &item.file) {
                if !catalog().ports_from_contents(item.class()) {
                    let saved = (item.inlets(), item.outlets());
                    let outcome = match reference.ports(item.text()) {
                        Some(ports) if (ports.inlets, ports.outlets) == saved => "predicted",
                        Some(_) => {
                            *self.port_misses.entry(catalog().canonical(item.class()).to_string()).or_default() += 1;
                            "mispredicted"
                        }
                        None => "unknown",
                    };
                    *self.ports.entry(outcome).or_default() += 1;
                }
            }
            let inner = match (&item.patcher, &item.file) {
                (Some(inner), _) => inner,
                (None, Some((name, inner))) if seen.insert(name.clone()) => inner,
                _ => continue,
            };
            self.walk(inner, seen);
        }
    }

    fn report(&self) -> Value {
        let rules: Map<String, Value> = RULES
            .iter()
            .map(|rule| {
                (
                    rule.to_string(),
                    json!({ "found": self.found.get(rule).copied().unwrap_or(0), "of": self.looked_at.get(rule).copied().unwrap_or(0) }),
                )
            })
            .collect();
        let tenth = |numbers: &[f64], at: f64| quantile(numbers, at).map(|value| (value * 10.0).round() / 10.0);
        let gaps = |numbers: &[f64]| json!([tenth(numbers, 0.25), tenth(numbers, 0.5), tenth(numbers, 0.75)]);
        json!({
            "devices": self.devices,
            "patchers": self.patchers,
            "boxes": self.shape.boxes,
            "cords": self.shape.cords,
            "rules": rules,
            "layout": {
                "bent_cords": self.shape.bent,
                "crossings": self.shape.crossings,
                "boxes_in_columns": self.shape.in_columns,
                "down_gap_quartiles": gaps(&self.shape.down_gaps),
                "side_gap_quartiles": gaps(&self.shape.side_gaps),
            },
            "faces": self.faces,
            "reference_ports": { "outcomes": self.ports, "mispredicted_classes": self.port_misses },
            "colours": self.colours,
            "objects": self.objects,
        })
    }
}

/// How the survey counts a box: a Max object by its class, a gen operator as gen:…, an abstraction as one, never by
/// its own name.
fn object_class(patcher: &Patcher, item: &MaxBox) -> String {
    if item.maxclass() == "newobj" && item.file.is_some() && !matches!(item.class(), "poly~" | "mc.poly~" | "gen~" | "gen" | "mc.gen~") {
        return "(abstraction)".into();
    }
    let class = catalog().canonical(item.class()).to_string();
    if patcher.is_gen() {
        format!("gen:{class}")
    } else {
        class
    }
}

/// The standard with the survey's counts in place of the last ones.
fn fold(standard: &mut Value, report: &Value) {
    for (rule, measured) in report["rules"].as_object().into_iter().flatten() {
        if let Some(entry) = standard["rules"].get_mut(rule).and_then(Value::as_object_mut) {
            entry.insert("measured".into(), measured.clone());
        }
    }
    standard["measured"] = json!({
        "devices": report["devices"],
        "patchers": report["patchers"],
        "boxes": report["boxes"],
        "cords": report["cords"],
        "layout": report["layout"],
    });
}

/// The standard as text: one line for each rule, so a change to one reads as one line.
fn standard_text(standard: &Value) -> String {
    let mut lines = vec!["{".to_string()];
    let fields: Vec<(&String, &Value)> = standard.as_object().into_iter().flatten().collect();
    for (index, (key, value)) in fields.iter().enumerate() {
        let comma = if index + 1 < fields.len() { "," } else { "" };
        match value.as_object().filter(|_| *key == "rules") {
            Some(rules) => {
                lines.push(format!("  {}: {{", json!(key)));
                for (at, (rule, entry)) in rules.iter().enumerate() {
                    let rule_comma = if at + 1 < rules.len() { "," } else { "" };
                    lines.push(format!("    {}: {}{rule_comma}", json!(rule), spaced(entry)));
                }
                lines.push(format!("  }}{comma}"));
            }
            None => lines.push(format!("  {}: {}{comma}", json!(key), spaced(value))),
        }
    }
    lines.push("}".to_string());
    lines.join("\n") + "\n"
}

/// JSON on one line, with a space after each colon and comma, as the standard is written.
fn spaced(value: &Value) -> String {
    match value {
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Object(map) => {
            format!("{{ {} }}", map.iter().map(|(key, value)| format!("{}: {}", json!(key), spaced(value))).collect::<Vec<_>>().join(", "))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(spaced).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}
