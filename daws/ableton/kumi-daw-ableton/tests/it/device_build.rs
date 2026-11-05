//! A device built for Hermes (CC-22): previewed without writing, built only as a new file, refused before anything is
//! written, and laid out on Live's device view whatever its controls.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::FutureExt;
use kumi_common::abort::Signal;
use kumi_daw_ableton::devices::build::{build, checked, preview};
use kumi_daw_ableton::devices::gen::{guarded_effect_patcher, GenSpec};
use kumi_daw_ableton::devices::patch::check::check;
use kumi_daw_ableton::devices::patch::standard::Level;
use kumi_daw_ableton::devices::patch::{NoFiles, Patcher};
use kumi_daw_ableton::devices::tool::{DeviceToolOptions, Made};
use serde_json::{json, Value};

fn options(library: &Path) -> DeviceToolOptions {
    DeviceToolOptions {
        user_library: library.to_string_lossy().into_owned(),
        browser_sees: Rc::new(|_, _| async { Ok(false) }.boxed_local()),
        wait_ms: Some(0),
        made: Rc::new(RefCell::new(Made::default())),
    }
}

fn echo() -> Value {
    json!({ "name": "Echo", "about": "A short echo.", "controls": [{ "name": "Time", "type": "number", "min": 1, "max": 500, "default": 120, "unit": "ms" }],
        "code": "Delay line(48000);\nline.write(in1);\nout1 = in1 + line.read(time * samplerate * 0.001) * 0.5;\nout2 = out1;\n" })
}

/// Every file under `folder`.
fn files(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        if entry.path().is_dir() {
            found.extend(files(&entry.path()));
        } else {
            found.push(entry.path());
        }
    }
    found
}

async fn built(library: &Path, spec: &Value) -> Result<kumi_agent::core::contracts::BuiltDevice, Vec<String>> {
    let file = preview(library, spec)?.file;
    build(&options(library), spec, &file, &Signal::new()).await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn a_preview_names_the_free_file_and_writes_nothing() {
    let library = tempfile::tempdir().unwrap();
    let previewed = preview(library.path(), &echo()).unwrap();
    assert_eq!((previewed.name.as_str(), previewed.file.as_str()), ("Echo", "User Library/Kumi/Audio Effects/Echo.amxd"));
    assert_eq!(previewed.lines, 4);
    assert_eq!(previewed.controls, [json!({ "type": "number", "name": "Time", "min": 1.0, "max": 500.0, "default": 120.0, "unit": "ms" })]);
    assert!(files(library.path()).is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn two_builds_asking_one_name_write_two_files_and_never_over_the_first() {
    let library = tempfile::tempdir().unwrap();
    let first = built(library.path(), &echo()).await.unwrap();
    assert_eq!((first.name.as_str(), first.item.as_str()), ("Echo", "user_library/Kumi/Audio Effects/Echo"));
    assert!(!first.listed);
    let bytes = std::fs::read(&first.path).unwrap();
    let second = built(library.path(), &echo()).await.unwrap();
    assert_eq!((second.name.as_str(), second.file.as_str()), ("Echo 2", "User Library/Kumi/Audio Effects/Echo 2.amxd"));
    assert_eq!(std::fs::read(&first.path).unwrap(), bytes, "the first file is as it was written");
    assert_eq!(files(library.path()).len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_build_whose_free_name_changed_since_its_preview_refuses_and_writes_nothing() {
    let library = tempfile::tempdir().unwrap();
    let file = preview(library.path(), &echo()).unwrap().file;
    let folder = library.path().join("Kumi/Audio Effects");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Echo.amxd"), b"the producer's own").unwrap();
    let refused = build(&options(library.path()), &echo(), &file, &Signal::new()).await.unwrap().unwrap_err();
    assert!(refused[0].contains("Echo 2.amxd now, not User Library/Kumi/Audio Effects/Echo.amxd"), "{refused:?}");
    assert_eq!(files(library.path()), [folder.join("Echo.amxd")]);
    assert_eq!(std::fs::read(folder.join("Echo.amxd")).unwrap(), b"the producer's own");
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_spec_is_refused_before_anything_is_written() {
    let library = tempfile::tempdir().unwrap();
    let with = |code: &str| {
        let mut spec = echo();
        spec["code"] = json!(code);
        spec
    };
    for (spec, why) in [
        (with("x = 0;\nwhile (x < time) { x += 1; }\nout1 = in1; out2 = in2;\n"), "while loop"),
        (with("Delay line(4194304);\nline.write(in1);\nout1 = line.read(time); out2 = out1;\n"), "holds 4194304 samples"),
        (with("Data table(4194304);\nout1 = in1 * time; out2 = in2;\n"), "holds 4194304 samples"),
        (with("Buffer b(\"shared\");\nout1 = in1 * time; out2 = in2;\n"), "Buffer"),
        (with("out1 = in1;\nout2 = in2;\n"), "nothing in it reads time"),
        (json!({ "name": "Echo", "about": "A short echo.", "code": "out1 = in1; out2 = in2;", "map": [] }), "map: not part"),
        (
            json!({ "name": "Echo", "about": "A short echo.", "controls": [{ "name": "Time", "type": "number", "min": 1, "max": 5, "default": 2, "group": "A" }],
            "code": "out1 = in1 * time; out2 = in2;" }),
            "controls[0].group",
        ),
        (json!({ "name": "", "about": "A short echo.", "code": "out1 = in1; out2 = in2;" }), "name"),
    ] {
        let found = checked(&spec).unwrap_err();
        assert!(found.iter().any(|said| said.contains(why)), "{why}: {found:?}");
        let refused = build(&options(library.path()), &spec, "User Library/Kumi/Audio Effects/Echo.amxd", &Signal::new()).await.unwrap();
        assert!(refused.is_err(), "{why}");
        assert!(preview(library.path(), &spec).is_err(), "{why}");
    }
    assert!(files(library.path()).is_empty(), "{:?}", files(library.path()));
}

#[test]
fn sixteen_controls_of_every_type_fit_the_device_view_with_output_last() {
    let types = ["number", "integer", "choice", "switch"];
    let controls: Vec<Value> = (0..16)
        .map(|index| {
            let name = format!("Control {}", index + 1);
            match types[index % 4] {
                "number" => json!({ "name": name, "type": "number", "min": 0, "max": 1, "default": 0.5 }),
                "integer" => json!({ "name": name, "type": "integer", "min": 1, "max": 8, "default": 2 }),
                "choice" => json!({ "name": name, "type": "choice", "options": ["Soft", "Hard", "Fold"], "default": "Hard" }),
                _ => json!({ "name": name, "type": "switch", "default": true }),
            }
        })
        .collect();
    let reads = (1..=16).map(|index| format!("control_{index}")).collect::<Vec<_>>().join(" + ");
    let spec = json!({ "name": "Sixteen", "about": "Every kind of control.", "controls": controls, "code": format!("out1 = in1 * ({reads});\nout2 = in2;\n") });
    let patcher = guarded_effect_patcher(&GenSpec::from(&checked(&spec).unwrap()));
    let report = check(&Patcher::read(&patcher, &NoFiles));
    let broken: Vec<String> = report.at_least(Level::Warn).map(ToString::to_string).collect();
    assert!(broken.is_empty(), "{}", broken.join("\n"));
    let boxes: Vec<&Value> = patcher["patcher"]["boxes"].as_array().unwrap().iter().map(|item| &item["box"]).collect();
    let faces: Vec<(&str, Vec<f64>)> = boxes
        .iter()
        .filter(|item| item.get("parameter_enable") == Some(&json!(1)))
        .map(|item| {
            let rect = item["presentation_rect"].as_array().unwrap().iter().map(|n| n.as_f64().unwrap()).collect();
            (item["varname"].as_str().unwrap(), rect)
        })
        .collect();
    assert_eq!(faces.len(), 18, "16 of the spec's, Mix and Output");
    assert!(faces.iter().all(|(_, rect)| rect[1] >= 0.0 && rect[1] + rect[3] <= 169.0), "{faces:?}");
    let (rightmost, _) = faces.iter().max_by(|a, b| (a.1[0] + a.1[2]).total_cmp(&(b.1[0] + b.1[2]))).unwrap();
    assert_eq!(*rightmost, "Output");
    // The guarded stage, its Output never boosting.
    let stage = boxes.iter().find(|item| item["id"] == "obj-output").unwrap();
    let code = stage["patcher"]["boxes"][0]["box"]["code"].as_str().unwrap();
    assert!(code.contains("Param kumi_output(0, min=-36, max=0);"), "{code}");
    let output = boxes.iter().find(|item| item.get("varname") == Some(&json!("Output"))).unwrap();
    assert_eq!(output["saved_attribute_attributes"]["valueof"]["parameter_mmax"], json!(0.0));
}
