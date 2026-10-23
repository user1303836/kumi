//! (m7) The producer's REAPER setup, read on each look: what new items and tracks start with, the extensions present and
//! the render settings in use. Power users change these defaults (fades off, loop source off, tracks at −6 dB), so Kumi
//! reads them rather than assume REAPER's factory ones. The config variables were checked in the lab (KP1, REAPER 7.82):
//! `splitautoxfade` &8 turns the new-item fades off and &2 gives new MIDI items them too, `deffadelen` is their length,
//! `loopnewitems` &4 loops imported media and &2 new MIDI items, `deftrackvol` is a new track's volume.

use serde_json::{json, Map, Value};

use crate::reaper::{Project, Reaper};
use crate::rows::{clip, db};

/// Extensions a script or a named command may need, by an API function each registers (`APIExists`).
const EXTENSIONS: [(&str, &str); 4] = [
    ("SWS", "CF_GetSWSVersion"),
    ("ReaPack", "ReaPack_BrowsePackages"),
    ("ReaImGui", "ImGui_GetVersion"),
    ("js_ReaScriptAPI", "JS_ReaScriptAPI_Version"),
];

fn config_number(reaper: &Reaper, name: &str) -> Option<f64> {
    reaper.config_text(name).and_then(|text| text.trim().parse::<f64>().ok())
}

/// Whether the producer's REAPER loops the source of imported media (`loopnewitems` &4); None when it can't say.
pub fn loops_imported_media(reaper: &Reaper) -> Option<bool> {
    config_number(reaper, "loopnewitems").map(|flags| flags as i64 & 4 != 0)
}

/// The look's `setup`: only what REAPER answered.
pub fn setup(reaper: &Reaper, proj: Project) -> Value {
    let mut setup = Map::new();
    if let (Some(flags), Some(length)) = (config_number(reaper, "splitautoxfade"), config_number(reaper, "deffadelen")) {
        let flags = flags as i64;
        setup.insert(
            "newItemFades".into(),
            if flags & 8 == 0 && length > 0.0 {
                json!({"ms": (length * 10_000.0).round() / 10.0, "midiItemsToo": flags & 2 != 0})
            } else {
                json!("off")
            },
        );
    }
    if let Some(flags) = config_number(reaper, "loopnewitems") {
        let flags = flags as i64;
        setup.insert("loopSourceOfNewItems".into(), json!({"imported": flags & 4 != 0, "midi": flags & 2 != 0}));
    }
    if let Some(volume) = config_number(reaper, "deftrackvol") {
        setup.insert("newTrackVolumeDb".into(), db(volume).into());
    }
    let extensions: Map<String, Value> =
        EXTENSIONS.iter().map(|(name, function)| (name.to_string(), reaper.api_exists(function).into())).collect();
    setup.insert("extensions".into(), extensions.into());
    setup.insert("render".into(), render(reaper, proj));
    Value::Object(setup)
}

/// The render settings in use (what File > Render would do now), in REAPER's words.
fn render(reaper: &Reaper, proj: Project) -> Value {
    let number = |key: &str| reaper.project_info(proj, key).unwrap_or(0.0);
    let text = |key: &str| reaper.project_text(proj, key).unwrap_or_default();
    let settings = number("RENDER_SETTINGS") as i64;
    let source = match () {
        _ if settings & 8 != 0 => "region render matrix",
        _ if settings & 4096 != 0 => "razor edit areas",
        _ if settings & 32 != 0 => "selected media items",
        _ if settings & 64 != 0 => "selected media items via master",
        _ if settings & 128 != 0 => "selected tracks via master",
        _ if settings & 3 == 3 => "stems (selected tracks)",
        _ if settings & 3 == 1 => "master mix and stems",
        // (2 alone renders the master and lists no targets, 7.82 lab)
        _ => "master mix",
    };
    let bounds = match number("RENDER_BOUNDSFLAG") as i64 {
        0 => "custom time range",
        1 => "entire project",
        2 => "time selection",
        3 => "all project regions",
        4 => "selected media items",
        5 => "selected project regions",
        6 => "all project markers",
        7 => "selected project markers",
        _ => "other",
    };
    let mut row = json!({"source": source, "bounds": bounds, "channels": number("RENDER_CHANNELS") as i64});
    if settings & 8192 != 0 {
        row["preFaderStems"] = true.into();
    }
    match number("RENDER_SRATE") as i64 {
        rate if rate > 0 => row["sampleRate"] = rate.into(),
        _ => row["sampleRate"] = "project".into(),
    }
    if let Some((format, bits)) = format(&text("RENDER_FORMAT")) {
        row["format"] = format.into();
        if let Some(bits) = bits {
            row["bits"] = bits.into();
        }
    }
    for (key, field) in [("RENDER_PATTERN", "pattern"), ("RENDER_FILE", "folder")] {
        let value = text(key);
        if !value.is_empty() {
            row[field] = clip(&value).into();
        }
    }
    // The tail applies when its flag has the bit for these bounds.
    let tail_bit = match number("RENDER_BOUNDSFLAG") as i64 {
        0 => 1,
        1 => 2,
        2 => 4,
        3 | 6 => 8,
        4 => 16,
        5 | 7 => 32,
        _ => 0,
    };
    if number("RENDER_TAILFLAG") as i64 & tail_bit != 0 {
        row["tailMs"] = (number("RENDER_TAILMS") as i64).into();
    }
    let post = number("RENDER_NORMALIZE") as i64;
    if post & (4 << 16) == 0 {
        if post & 1 != 0 {
            let to = match post & 14 {
                0 => "LUFS-I",
                2 => "RMS",
                4 => "peak",
                6 => "true peak",
                8 => "LUFS-M max",
                10 => "LUFS-S max",
                _ => "other",
            };
            row["normalize"] = json!({"to": to, "db": db(number("RENDER_NORMALIZE_TARGET"))});
        }
        if post & 64 != 0 {
            row["limitDb"] = db(number("RENDER_BRICKWALL")).into();
        }
    }
    row
}

/// The render format from `RENDER_FORMAT` (base64 of the sink's settings, whose first four bytes are its id reversed:
/// `evaw` is WAV), and a WAV's bit depth (its fifth byte).
fn format(config: &str) -> Option<(String, Option<u8>)> {
    let bytes = base64_prefix(config, 6).filter(|bytes| bytes.len() >= 4)?;
    let id: String = bytes[..4].iter().rev().map(|&b| b as char).collect();
    let name = match id.as_str() {
        "wave" => "WAV",
        "aiff" => "AIFF",
        "flac" => "FLAC",
        "mp3l" => "MP3",
        "oggv" => "OGG Vorbis",
        "OggS" => "Opus",
        "wvpk" => "WavPack",
        "caff" => "CAF",
        "ddp " => "DDP",
        "FFMP" => "video (FFmpeg)",
        other if other.chars().all(|c| c.is_ascii_graphic() || c == ' ') => other,
        _ => return None,
    };
    let bits = bytes.get(4).copied().filter(|bits| id == "wave" && matches!(bits, 8 | 16 | 24 | 32 | 64));
    Some((name.to_string(), bits))
}

/// At most the first `count` bytes base64 decodes to; None when it isn't base64.
fn base64_prefix(text: &str, count: usize) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(count + 2);
    for chunk in text.trim().as_bytes().chunks(4) {
        if out.len() >= count {
            break;
        }
        let digits: Vec<u32> = chunk.iter().take_while(|&&c| c != b'=').map(|&c| value(c)).collect::<Option<_>>()?;
        let joined = digits.iter().enumerate().fold(0u32, |acc, (i, d)| acc | (d << (18 - 6 * i)));
        out.extend(joined.to_be_bytes()[1..].iter().take(digits.len().saturating_sub(1)));
    }
    out.truncate(count);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::fake;

    fn read() -> Value {
        fake::reset();
        let api = fake::api();
        let reaper = Reaper::new(&api);
        setup(&reaper, reaper.current_project().unwrap().ptr)
    }

    #[test]
    fn reaper_factory_defaults_read_as_such() {
        let setup = read();
        assert_eq!(setup["newItemFades"], json!({"ms": 10.0, "midiItemsToo": false}));
        assert_eq!(setup["loopSourceOfNewItems"], json!({"imported": true, "midi": true}));
        assert_eq!(setup["newTrackVolumeDb"], json!(0.0));
        assert_eq!(setup["extensions"], json!({"SWS": false, "ReaPack": false, "ReaImGui": false, "js_ReaScriptAPI": false}));
        assert_eq!(
            setup["render"],
            json!({"source": "master mix", "bounds": "entire project", "channels": 2, "sampleRate": "project", "format": "WAV", "bits": 24})
        );
    }

    #[test]
    fn a_power_users_changed_defaults_are_read() {
        fake::reset();
        fake::extra(|e| {
            for (name, value) in [("splitautoxfade", "8"), ("loopnewitems", "10"), ("deftrackvol", "0.501187")] {
                e.config.insert(name.into(), value.into());
            }
            e.extension_apis = vec!["CF_GetSWSVersion".into(), "ReaPack_BrowsePackages".into()];
        });
        fake::with(|s| {
            for (key, value) in
                [("RENDER_SETTINGS", 8.0 + 8192.0), ("RENDER_BOUNDSFLAG", 3.0), ("RENDER_SRATE", 48000.0), ("RENDER_TAILFLAG", 8.0)]
            {
                s.listening.numbers.insert(key.into(), value);
            }
            s.listening.texts.insert("RENDER_PATTERN".into(), "$region".into());
        });
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let setup = setup(&reaper, reaper.current_project().unwrap().ptr);
        assert_eq!(setup["newItemFades"], json!("off"));
        assert_eq!(setup["loopSourceOfNewItems"], json!({"imported": false, "midi": true}));
        assert_eq!(setup["newTrackVolumeDb"], json!(-6.0));
        assert_eq!(setup["extensions"]["SWS"], json!(true));
        assert_eq!(setup["extensions"]["ReaPack"], json!(true));
        assert_eq!(setup["extensions"]["ReaImGui"], json!(false));
        let render = &setup["render"];
        assert_eq!(render["source"], json!("region render matrix"));
        assert_eq!(render["bounds"], json!("all project regions"));
        assert_eq!((render["sampleRate"].clone(), render["tailMs"].clone()), (json!(48000), json!(1000)));
        assert_eq!((render["pattern"].clone(), render["preFaderStems"].clone()), (json!("$region"), json!(true)));
    }

    #[test]
    fn each_look_carries_the_setup() {
        fake::lab_project();
        let api = fake::api();
        let snapshot = crate::read::Reads::default().snapshot(&Reaper::new(&api)).unwrap();
        assert_eq!(snapshot["setup"]["loopSourceOfNewItems"], json!({"imported": true, "midi": true}));
        assert_eq!(snapshot["setup"]["render"]["source"], json!("master mix"));
    }

    #[test]
    fn render_settings_two_alone_is_the_master_mix_reaper_renders() {
        for (settings, source) in
            [(0.0, "master mix"), (1.0, "master mix and stems"), (2.0, "master mix"), (3.0, "stems (selected tracks)")]
        {
            fake::reset();
            fake::with(|s| s.listening.numbers.insert("RENDER_SETTINGS".into(), settings));
            let api = fake::api();
            let reaper = Reaper::new(&api);
            assert_eq!(setup(&reaper, reaper.current_project().unwrap().ptr)["render"]["source"], json!(source), "{settings}");
        }
    }

    #[test]
    fn render_formats_decode_from_their_settings() {
        assert_eq!(format("ZXZhdxgAAQ=="), Some(("WAV".into(), Some(24))));
        // Kumi's own float WAV for listening.
        assert_eq!(format(crate::listening::render::FLOAT_WAV), Some(("WAV".into(), Some(32))));
        assert_eq!(format("Y2FsZg=="), Some(("FLAC".into(), None)));
        assert_eq!(format(""), None);
        assert_eq!(format("not base64!"), None);
    }
}
