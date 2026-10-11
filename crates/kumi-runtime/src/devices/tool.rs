//! Build a checked Max for Live device in Live's User Library, then wait for its Browser.

use super::{
    amxd::{encode_amxd, DeviceType},
    gen::{audio_effect_patcher, instrument_patcher, GenSpec, MIX, OUTPUT},
    harness::{check_midi_device_isolated, IsolatedOptions},
    midi::midi_device_patcher,
    patch::reference,
    patched::patched_device,
    spec::{check_spec, Control, DeviceSpec},
};
use crate::{
    core::{
        contracts::{JsonObject, KernelTool, ToolResult},
        disk::{low_disk, MB},
        errors::RuntimeError,
    },
    system::windows_device_name,
};
use async_trait::async_trait;
use futures::future::LocalBoxFuture;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{json::stringify, number::to_string},
    time::now_ms,
};
use serde_json::{json, Value};
use std::{path::Path, rc::Rc, sync::LazyLock, time::Duration};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub const MAKE_DEVICE_TOOL: &str = "make_device";
static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("tool-data.json")).expect("device tool data"));
pub type BrowserSees = Rc<dyn Fn(String, Signal) -> LocalBoxFuture<'static, Result<bool, RuntimeError>>>;
#[derive(Clone)]
pub struct DeviceToolOptions {
    pub user_library: String,
    pub browser_sees: BrowserSees,
    pub wait_ms: Option<u64>,
}
struct DeviceTool {
    options: DeviceToolOptions,
}
pub fn device_tool(options: DeviceToolOptions) -> Rc<dyn KernelTool> {
    Rc::new(DeviceTool { options })
}

/// The device's name, numbered past one a file in the folder has. On Windows a name it takes for a device ("Aux",
/// "Con.Dist") is taken as a file's would be, and its number goes right after that part ("Aux 2", "Con 2.Dist").
fn free_name(folder: &Path, name: &str, windows: bool) -> String {
    let (stem, rest) = name.split_at(name.find('.').unwrap_or(name.len()));
    let numbered = |number: &str| {
        if windows && windows_device_name(stem) {
            format!("{stem} {number}{rest}")
        } else {
            format!("{name} {number}")
        }
    };
    for index in 1..1000 {
        let candidate = if index == 1 { name.into() } else { numbered(&index.to_string()) };
        if !(windows && windows_device_name(&candidate)) && !folder.join(format!("{candidate}.amxd")).exists() {
            return candidate;
        }
    }
    numbered(&Uuid::new_v4().to_string()[..8])
}
fn describe_control(control: &Control) -> String {
    match control {
        Control::Choice(c) => format!("{} ({}; {})", c.name, c.options.join(" / "), c.default),
        Control::Switch(c) => format!("{} (on/off; {})", c.name, if c.default { "on" } else { "off" }),
        Control::Number(c) | Control::Integer(c) => format!(
            "{} ({}–{}{}; {})",
            c.name,
            to_string(c.min),
            to_string(c.max),
            if c.unit.as_str().is_empty() { String::new() } else { format!(" {}", c.unit) },
            to_string(c.default)
        ),
    }
}
fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
#[async_trait(?Send)]
impl KernelTool for DeviceTool {
    fn name(&self) -> &str {
        MAKE_DEVICE_TOOL
    }
    fn description(&self) -> &str {
        DATA["description"].as_str().unwrap()
    }
    fn input_schema(&self) -> JsonObject {
        DATA["schema"].as_object().unwrap().clone()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        if input.get("guide").and_then(Value::as_bool) == Some(true) {
            let kind = input.get("type").and_then(Value::as_str).unwrap_or("");
            return Ok(ToolResult::text(if matches!(kind, "midi_effect" | "audio_effect" | "instrument") {
                DATA[kind].as_str().unwrap().into()
            } else {
                ["midi_effect", "audio_effect", "instrument"].map(|k| DATA[k].as_str().unwrap()).join("\n\n")
            }));
        }
        let spec = match check_spec(&input) {
            Ok(spec) => spec,
            Err(problems) => {
                return Ok(ToolResult::error(stringify(&json!({"problems": problems, "next":"Fix these and call make_device again."}))))
            }
        };
        let tested = if let DeviceSpec::Patch(_) = &spec {
            match reference::installed() {
                Some(_) => "Kumi's checks passed: every object is one this machine's Max has, every cord meets a port, and no message's order is left to where its boxes sit; Kumi laid it out top to bottom".to_string(),
                None => "Kumi's checks passed (Max wasn't found here, so its objects weren't checked against it); Kumi laid it out top to bottom".to_string(),
            }
        } else if let DeviceSpec::MidiEffect(midi) = &spec {
            let verified = check_midi_device_isolated(midi, IsolatedOptions::default()).await;
            if !verified.problems.is_empty() {
                return Ok(ToolResult::error(stringify(
                    &json!({"problems":verified.problems,"passed":format!("{} of {} of its tests", verified.passed, verified.of),"next":"Fix the code (or a test that's wrong) and call make_device again."}),
                )));
            }
            format!(
                "{} of {} of its tests passed, and Kumi's checks ({})",
                verified.passed,
                verified.of,
                if midi.runs_free { "no errors; it runs free" } else { "no errors, no hanging notes" }
            )
        } else {
            "Kumi's checks passed (its outputs, its inputs, its controls); Max compiles the code when Live loads it".into()
        };
        signal.check()?;
        let folder = Path::new(&self.options.user_library).join("Kumi");
        if let Some(full) = low_disk(&self.options.user_library, 100.0 * MB, "Live's User Library is on").await {
            return Ok(ToolResult::error(format!("{full} No device was made.")));
        }
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o755);
        builder.create(&folder).await.map_err(io_error)?;
        let name = free_name(&folder, spec.name(), crate::system::platform() == "win32");
        let file = folder.join(format!("{name}.amxd"));
        let temporary = folder.join(format!(".{}.amxd", Uuid::new_v4()));
        let patcher = match &spec {
            DeviceSpec::MidiEffect(s) => {
                let mut s = s.clone();
                s.name = name.clone();
                midi_device_patcher(&s)
            }
            DeviceSpec::AudioEffect(s) => {
                let mut s = GenSpec::from(s);
                s.name = name.clone();
                audio_effect_patcher(&s)
            }
            DeviceSpec::Instrument(s) => {
                let mut s = GenSpec::from(s);
                s.name = name.clone();
                instrument_patcher(&s)
            }
            DeviceSpec::Patch(s) => {
                let mut s = s.clone();
                s.name = name.clone();
                match patched_device(&s, reference::installed()) {
                    Ok(patcher) => patcher,
                    Err(problems) => {
                        return Ok(ToolResult::error(stringify(
                            &json!({"problems": problems, "next":"Fix these and call make_device again."}),
                        )))
                    }
                }
            }
        };
        let written: Result<(), RuntimeError> = async {
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            options.mode(0o644);
            let mut out = options.open(&temporary).await.map_err(io_error)?;
            out.write_all(&encode_amxd(spec.kind(), &patcher)).await.map_err(io_error)?;
            out.flush().await.map_err(io_error)?;
            drop(out);
            tokio::fs::rename(&temporary, &file).await.map_err(io_error)
        }
        .await;
        let removed = tokio::fs::remove_file(&temporary).await;
        if let Err(error) = removed {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(io_error(error));
            }
        }
        written?;
        let item_id = format!("user_library/Kumi/{name}");
        let deadline = now_ms().saturating_add(self.options.wait_ms.unwrap_or(20_000) as i64);
        let mut seen = false;
        while !seen && now_ms() < deadline {
            seen = (self.options.browser_sees)(item_id.clone(), signal.clone()).await.unwrap_or(false);
            if !seen {
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            signal.check()?;
        }
        // A silent render means the code didn't compile, or a patch passes no sound.
        let silent =
            if matches!(spec, DeviceSpec::Patch(_)) { "the patch passes no sound: fix it" } else { "the code didn't compile: fix it" };
        let (kind,next,extra)=match spec.kind() {
            DeviceType::MidiEffect => ("MIDI effect","load_device with this itemId on the MIDI track; Live puts a MIDI effect before the instrument.".to_string(),vec![]),
            DeviceType::AudioEffect => ("audio effect",format!("load_device with this itemId on the track, then hear it with audition while audio plays through that track (a silent render means {silent} and make it again)."),vec![&*MIX,&*OUTPUT]),
            DeviceType::Instrument => ("instrument",format!("load_device with this itemId on a MIDI track, write a short clip, and hear it with audition (a silent render means {silent} and make it again)."),vec![&*OUTPUT]),
        };
        let controls: Vec<_> = spec.controls().iter().chain(extra).map(describe_control).collect();
        let mut result = json!({"made":name,"type":kind,"itemId":item_id,"file":file.to_string_lossy(),"controls":controls});
        let fields = result.as_object_mut().unwrap();
        if let DeviceSpec::Instrument(s) = &spec {
            fields.insert("voices".into(), json!(s.voices));
        }
        fields.insert("checks".into(), json!(tested));
        if !seen {
            fields.insert("note".into(), json!("Live's Browser hasn't listed it yet; load it in a moment."));
        }
        fields.insert("next".into(), json!(next));
        Ok(ToolResult::text(stringify(&result)))
    }
}

#[cfg(test)]
mod tests {
    use super::free_name;

    #[test]
    fn a_name_windows_takes_for_a_device_is_numbered_there() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("Bass.amxd"), b"").unwrap();
        assert_eq!(free_name(folder.path(), "Bass", false), "Bass 2", "a file's name is taken everywhere");
        assert_eq!((free_name(folder.path(), "Aux", false), free_name(folder.path(), "Aux", true)), ("Aux".into(), "Aux 2".into()));
        assert_eq!(free_name(folder.path(), "Con.Dist", true), "Con 2.Dist", "the part before the dot is what Windows reads");
        assert_eq!(free_name(folder.path(), "LPT1 ", true), "LPT1  2");
        assert_eq!(free_name(folder.path(), "Console", true), "Console");
    }
}
