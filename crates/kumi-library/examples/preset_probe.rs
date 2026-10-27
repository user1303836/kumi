//! Offline encoder for checking a preset codec against an installed plug-in. The
//! output can be supplied to an isolated VST3 host; this program never loads a plug-in.
//!
//! `cargo run -p kumi-library --example preset_probe -- FAMILY INPUT OUTPUT EDITS.json [TEMPLATE.vstpreset]`
//! FAMILY is valhalla, minimal-audio, pulsar-modular, ozone-12, serum-2,
//! zynaptiq, softube, or superior-drummer.
//! Edits are [{"path":...,"value":...}]. Serum input is a native .SerumPreset;
//! its output is a VST3 preset, and `$add_distortion` appends the initialized FX to a rack.
//! A native vendor XML input can use the same product's VST3 template to produce a loadable state.
//! `workflow:<plugin-id> INPUT OUTPUT EDITS.json OPTIONS.json` exercises the public workflow;
//! INPUT may be `init` to create a patch. Options contain `version`, `classId`, and `name`.

use kumi_library::plugins::formats::{
    minimal_audio::MinimalAudioPreset, ozone12::OzoneState, pulsar_modular::PulsarModularPreset, serum2::SerumPreset,
    valhalla::ValhallaPreset, vstpreset::VstPreset,
};
use kumi_library::plugins::formats::{softube, superior_drummer, zynaptiq};
use kumi_library::plugins::preset::{self, Edit, WriteOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(4..=5).contains(&args.len()) {
        return Err("usage: preset_probe FAMILY INPUT OUTPUT EDITS.json [TEMPLATE.vstpreset]".into());
    }
    let edits: Vec<Edit> = serde_json::from_slice(&std::fs::read(&args[3])?)?;
    if let Some(plugin) = args[0].strip_prefix("workflow:") {
        let options: WriteOptions = serde_json::from_slice(&std::fs::read(args.get(4).ok_or("workflow requires OPTIONS.json")?)?)?;
        let output = if args[1] == "init" {
            preset::create(plugin, &options, &edits)?
        } else {
            preset::edit(plugin, &std::fs::read(&args[1])?, &options, &edits)?
        };
        std::fs::write(&args[2], output.write())?;
        return Ok(());
    }
    let input = std::fs::read(&args[1])?;
    if args[0] == "serum-2" {
        let mut format = SerumPreset::read(&input)?;
        for edit in &edits {
            if edit.path == "$add_distortion" {
                format.add_distortion(usize::try_from(edit.value.as_u64().ok_or("$add_distortion needs rack 0, 1 or 2")?)?)?;
            } else {
                format.set(&edit.path, &edit.value)?;
            }
        }
        std::fs::write(&args[2], format.to_vstpreset()?.write())?;
        return Ok(());
    }
    let mut preset = input.starts_with(b"VST3").then(|| VstPreset::read(&input)).transpose()?;
    let template = args.get(4).map(std::fs::read).transpose()?.map(|bytes| VstPreset::read(&bytes)).transpose()?;
    if template.is_some() && preset.is_some() {
        return Err("a template is only used for native XML input".into());
    }
    let template_component = template.as_ref().map(|preset| preset.component().ok_or("template has no component state")).transpose()?;
    let state = match &preset {
        Some(preset) => preset.component().ok_or("VST3 preset has no component state")?,
        None => &input,
    };
    let encoded = match args[0].as_str() {
        "zynaptiq" => {
            let mut format = zynaptiq::State::read(state)?;
            for edit in &edits {
                format.set(&edit.path, &edit.value)?;
            }
            format.write()?
        }
        "superior-drummer" => {
            let mut format = superior_drummer::State::read(state)?;
            if format.preset.is_none() {
                format.preset = Some(superior_drummer::Preset::create("3.4.1")?);
            }
            for edit in &edits {
                format.set(&edit.path, &edit.value)?;
            }
            format.write()?
        }
        "softube" => {
            let mut format = softube::Preset::read(state)?;
            for edit in &edits {
                format.set(&edit.path, &edit.value)?;
            }
            format.write()?
        }
        "ozone-12" => {
            let mut format = OzoneState::read(state)?;
            for edit in &edits {
                if edit.path == "$chain" {
                    let modules: Vec<&str> = edit
                        .value
                        .as_array()
                        .ok_or("$chain must be an array")?
                        .iter()
                        .map(|item| item.as_str().ok_or("chain entries must be strings"))
                        .collect::<Result<_, _>>()?;
                    format.set_chain(&modules)?;
                } else {
                    let (element, id) = edit.path.split_once('/').ok_or("Ozone path must be ElementID/ParamID")?;
                    format.set_value(element, id, edit.value.clone())?;
                }
            }
            if template_component.is_some() {
                return Err("Ozone's native preset conversion uses OzonePreset::to_state".into());
            }
            format.write()?
        }
        "valhalla" => {
            let mut format = ValhallaPreset::read(state)?;
            for edit in &edits {
                format.set(&edit.path, &edit.value)?;
            }
            match template_component {
                Some(template) => format.to_state(template)?,
                None => format.write()?,
            }
        }
        "minimal-audio" => {
            let mut format = MinimalAudioPreset::read(state)?;
            for edit in &edits {
                format.set(&edit.path, &edit.value)?;
            }
            match template_component {
                Some(template) => format.to_state(template)?,
                None => format.write()?,
            }
        }
        "pulsar-modular" => {
            let mut format = PulsarModularPreset::read(state)?;
            for edit in &edits {
                format.set(&edit.path, &edit.value)?;
            }
            match template_component {
                Some(template) => format.to_state(template)?,
                None => format.write()?,
            }
        }
        _ => return Err("unknown preset family".into()),
    };
    if preset.is_none() {
        preset = template;
    }
    let output = match &mut preset {
        Some(preset) => {
            preset.chunks.iter_mut().find(|(id, _)| id == "Comp").unwrap().1 = encoded;
            preset.write()
        }
        None => encoded,
    };
    std::fs::write(&args[2], output)?;
    Ok(())
}
