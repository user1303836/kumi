//! m4-r-fxenv: the fake's FX in every chain (a track's main and input chains, a take's, containers' children, by REAPER's
//! addresses), FX chain files and chunk entries, presets, named configuration (modulation, ReaEQ's bands, a container's
//! macros), parameter envelopes, envelopes' points and automation items, and the global automation override. Its
//! functions come before the M3 fake's own of the same name.
//!
//! It keeps REAPER's quirks the changes have to meet (lab 7.82): a copy into a container is counted (it hangs REAPER);
//! `GetFXEnvelope(…, create)`, a state chunk, an envelope chunk, a macro taken off and most modulation values make no undo
//! point by themselves; a track in touch, latch or write mode records parameter and bypass writes unless the global
//! override is read or trim; a chain file's FX id is kept only when no FX in the project has it.

use super::*;
use std::path::Path;

pub(super) fn lookup(name: &CStr) -> Option<*mut c_void> {
    macro_rules! table {
        ($($name:literal => $f:expr),* $(,)?) => {
            match name.to_bytes() {
                $($name => Some($f as *mut c_void),)*
                _ => None,
            }
        };
    }
    table! {
        b"TrackFX_AddByName" => track_fx_add_by_name as unsafe extern "C" fn(*mut MediaTrack, *const c_char, bool, c_int) -> c_int,
        b"TakeFX_AddByName" => take_fx_add_by_name as unsafe extern "C" fn(*mut MediaItem_Take, *const c_char, c_int) -> c_int,
        b"TrackFX_Delete" => track_fx_delete as unsafe extern "C" fn(*mut MediaTrack, c_int) -> bool,
        b"TakeFX_Delete" => take_fx_delete as unsafe extern "C" fn(*mut MediaItem_Take, c_int) -> bool,
        b"TrackFX_CopyToTrack" => track_fx_copy_to_track as unsafe extern "C" fn(*mut MediaTrack, c_int, *mut MediaTrack, c_int, bool),
        b"TrackFX_CopyToTake" => track_fx_copy_to_take as unsafe extern "C" fn(*mut MediaTrack, c_int, *mut MediaItem_Take, c_int, bool),
        b"TakeFX_CopyToTrack" => take_fx_copy_to_track as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *mut MediaTrack, c_int, bool),
        b"TakeFX_CopyToTake" => take_fx_copy_to_take as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *mut MediaItem_Take, c_int, bool),
        b"TrackFX_SetEnabled" => track_fx_set_enabled as unsafe extern "C" fn(*mut MediaTrack, c_int, bool),
        b"TakeFX_SetEnabled" => take_fx_set_enabled as unsafe extern "C" fn(*mut MediaItem_Take, c_int, bool),
        b"TrackFX_SetOffline" => track_fx_set_offline as unsafe extern "C" fn(*mut MediaTrack, c_int, bool),
        b"TakeFX_SetOffline" => take_fx_set_offline as unsafe extern "C" fn(*mut MediaItem_Take, c_int, bool),
        b"TrackFX_GetNumParams" => track_fx_get_num_params as unsafe extern "C" fn(*mut MediaTrack, c_int) -> c_int,
        b"TakeFX_GetNumParams" => take_fx_get_num_params as unsafe extern "C" fn(*mut MediaItem_Take, c_int) -> c_int,
        b"TrackFX_GetParamName" => track_fx_get_param_name as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, *mut c_char, c_int) -> bool,
        b"TakeFX_GetParamName" => take_fx_get_param_name as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, *mut c_char, c_int) -> bool,
        b"TrackFX_GetParamNormalized" => track_fx_get_param_normalized as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int) -> f64,
        b"TakeFX_GetParamNormalized" => take_fx_get_param_normalized as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int) -> f64,
        b"TrackFX_SetParamNormalized" => track_fx_set_param_normalized as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, f64) -> bool,
        b"TakeFX_SetParamNormalized" => take_fx_set_param_normalized as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, f64) -> bool,
        b"TrackFX_GetFormattedParamValue" => track_fx_get_formatted_param_value as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, *mut c_char, c_int) -> bool,
        b"TakeFX_GetFormattedParamValue" => take_fx_get_formatted_param_value as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, *mut c_char, c_int) -> bool,
        b"TrackFX_FormatParamValueNormalized" => track_fx_format_param_value_normalized as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, f64, *mut c_char, c_int) -> bool,
        b"TakeFX_FormatParamValueNormalized" => take_fx_format_param_value_normalized as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, f64, *mut c_char, c_int) -> bool,
        b"TrackFX_GetParam" => track_fx_get_param as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, *mut f64, *mut f64) -> f64,
        b"TakeFX_GetParam" => take_fx_get_param as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, *mut f64, *mut f64) -> f64,
        b"TrackFX_SetParam" => track_fx_set_param as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, f64) -> bool,
        b"TakeFX_SetParam" => take_fx_set_param as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, f64) -> bool,
        b"TrackFX_GetNamedConfigParm" => track_fx_get_named_config_parm as unsafe extern "C" fn(*mut MediaTrack, c_int, *const c_char, *mut c_char, c_int) -> bool,
        b"TakeFX_GetNamedConfigParm" => take_fx_get_named_config_parm as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *const c_char, *mut c_char, c_int) -> bool,
        b"TrackFX_SetNamedConfigParm" => track_fx_set_named_config_parm as unsafe extern "C" fn(*mut MediaTrack, c_int, *const c_char, *const c_char) -> bool,
        b"TakeFX_SetNamedConfigParm" => take_fx_set_named_config_parm as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *const c_char, *const c_char) -> bool,
        b"TrackFX_GetParamFromIdent" => track_fx_get_param_from_ident as unsafe extern "C" fn(*mut MediaTrack, c_int, *const c_char) -> c_int,
        b"TakeFX_GetParamFromIdent" => take_fx_get_param_from_ident as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *const c_char) -> c_int,
        b"TrackFX_GetPresetIndex" => track_fx_get_preset_index as unsafe extern "C" fn(*mut MediaTrack, c_int, *mut c_int) -> c_int,
        b"TakeFX_GetPresetIndex" => take_fx_get_preset_index as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *mut c_int) -> c_int,
        b"TrackFX_SetPreset" => track_fx_set_preset as unsafe extern "C" fn(*mut MediaTrack, c_int, *const c_char) -> bool,
        b"TakeFX_SetPreset" => take_fx_set_preset as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *const c_char) -> bool,
        b"TrackFX_SetPresetByIndex" => track_fx_set_preset_by_index as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int) -> bool,
        b"TakeFX_SetPresetByIndex" => take_fx_set_preset_by_index as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int) -> bool,
        b"TrackFX_NavigatePresets" => track_fx_navigate_presets as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int) -> bool,
        b"TakeFX_NavigatePresets" => take_fx_navigate_presets as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int) -> bool,
        b"TrackFX_GetEQParam" => track_fx_get_eq_param as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, *mut c_int, *mut c_int, *mut c_int, *mut f64) -> bool,
        b"TrackFX_GetPinMappings" => track_fx_get_pin_mappings as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, c_int, *mut c_int) -> c_int,
        b"TakeFX_GetPinMappings" => take_fx_get_pin_mappings as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, c_int, *mut c_int) -> c_int,
        b"TrackFX_SetPinMappings" => track_fx_set_pin_mappings as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, c_int, c_int, c_int) -> bool,
        b"TakeFX_SetPinMappings" => take_fx_set_pin_mappings as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, c_int, c_int, c_int) -> bool,
        b"TakeFX_GetIOSize" => take_fx_get_io_size as unsafe extern "C" fn(*mut MediaItem_Take, c_int, *mut c_int, *mut c_int) -> c_int,
        b"GetFXEnvelope" => get_fx_envelope as unsafe extern "C" fn(*mut MediaTrack, c_int, c_int, bool) -> *mut TrackEnvelope,
        b"TakeFX_GetEnvelope" => take_fx_get_envelope as unsafe extern "C" fn(*mut MediaItem_Take, c_int, c_int, bool) -> *mut TrackEnvelope,
        b"Envelope_GetParentTrack" => envelope_get_parent_track as unsafe extern "C" fn(*mut TrackEnvelope, *mut c_int, *mut c_int) -> *mut MediaTrack,
        b"CountTrackEnvelopes" => count_track_envelopes as unsafe extern "C" fn(*mut MediaTrack) -> c_int,
        b"GetTrackEnvelope" => get_track_envelope as unsafe extern "C" fn(*mut MediaTrack, c_int) -> *mut TrackEnvelope,
        b"GetTrackEnvelopeByChunkName" => get_track_envelope_by_chunk_name as unsafe extern "C" fn(*mut MediaTrack, *const c_char) -> *mut TrackEnvelope,
        b"GetTrackEnvelopeByName" => get_track_envelope_by_name as unsafe extern "C" fn(*mut MediaTrack, *const c_char) -> *mut TrackEnvelope,
        b"GetTakeEnvelopeByName" => get_take_envelope_by_name as unsafe extern "C" fn(*mut MediaItem_Take, *const c_char) -> *mut TrackEnvelope,
        b"CountEnvelopePoints" => count_envelope_points as unsafe extern "C" fn(*mut TrackEnvelope) -> c_int,
        b"CountEnvelopePointsEx" => count_envelope_points_ex as unsafe extern "C" fn(*mut TrackEnvelope, c_int) -> c_int,
        b"GetEnvelopePointEx" => get_envelope_point_ex as unsafe extern "C" fn(*mut TrackEnvelope, c_int, c_int, *mut f64, *mut f64, *mut c_int, *mut f64, *mut bool) -> bool,
        b"InsertEnvelopePointEx" => insert_envelope_point_ex as unsafe extern "C" fn(*mut TrackEnvelope, c_int, f64, f64, c_int, f64, bool, *mut bool) -> bool,
        b"SetEnvelopePointEx" => set_envelope_point_ex as unsafe extern "C" fn(*mut TrackEnvelope, c_int, c_int, *mut f64, *mut f64, *mut c_int, *mut f64, *mut bool, *mut bool) -> bool,
        b"DeleteEnvelopePointEx" => delete_envelope_point_ex as unsafe extern "C" fn(*mut TrackEnvelope, c_int, c_int) -> bool,
        b"DeleteEnvelopePointRangeEx" => delete_envelope_point_range_ex as unsafe extern "C" fn(*mut TrackEnvelope, c_int, f64, f64) -> bool,
        b"Envelope_SortPointsEx" => envelope_sort_points_ex as unsafe extern "C" fn(*mut TrackEnvelope, c_int) -> bool,
        b"GetEnvelopeScalingMode" => get_envelope_scaling_mode as unsafe extern "C" fn(*mut TrackEnvelope) -> c_int,
        b"ScaleToEnvelopeMode" => scale_to_envelope_mode as unsafe extern "C" fn(c_int, f64) -> f64,
        b"ScaleFromEnvelopeMode" => scale_from_envelope_mode as unsafe extern "C" fn(c_int, f64) -> f64,
        b"Envelope_FormatValue" => envelope_format_value as unsafe extern "C" fn(*mut TrackEnvelope, f64, *mut c_char, c_int),
        b"GetEnvelopeStateChunk" => get_envelope_state_chunk as unsafe extern "C" fn(*mut TrackEnvelope, *mut c_char, c_int, bool) -> bool,
        b"SetEnvelopeStateChunk" => set_envelope_state_chunk as unsafe extern "C" fn(*mut TrackEnvelope, *const c_char, bool) -> bool,
        b"GetSetEnvelopeInfo_String" => get_set_envelope_info_string as unsafe extern "C" fn(*mut TrackEnvelope, *const c_char, *mut c_char, bool) -> bool,
        b"GetEnvelopeInfo_Value" => get_envelope_info_value as unsafe extern "C" fn(*mut TrackEnvelope, *const c_char) -> f64,
        b"CountAutomationItems" => count_automation_items as unsafe extern "C" fn(*mut TrackEnvelope) -> c_int,
        b"GetSetAutomationItemInfo" => get_set_automation_item_info as unsafe extern "C" fn(*mut TrackEnvelope, c_int, *const c_char, f64, bool) -> f64,
        b"GetSetAutomationItemInfo_String" => get_set_automation_item_info_string as unsafe extern "C" fn(*mut TrackEnvelope, c_int, *const c_char, *mut c_char, bool) -> bool,
        b"InsertAutomationItem" => insert_automation_item as unsafe extern "C" fn(*mut TrackEnvelope, c_int, f64, f64) -> c_int,
        b"GetGlobalAutomationOverride" => get_global_automation_override as unsafe extern "C" fn() -> c_int,
        b"SetGlobalAutomationOverride" => set_global_automation_override as unsafe extern "C" fn(c_int),
    }
}

// ----- chains, by REAPER's addresses -----

fn chain_mut(s: &mut State, owner: u64, input: bool) -> Option<&mut Vec<FakeFx>> {
    let (kind, a, b, c) = unpack(owner);
    match kind {
        TRACK => {
            let track = if a == MASTER { &mut s.master } else { s.tracks.get_mut(a as usize)? };
            Some(if input { &mut track.input_fx } else { &mut track.fx })
        }
        TAKE => s.tracks.get_mut(a as usize)?.items.get_mut(b as usize)?.takes.get_mut(c as usize).map(|t| &mut t.fx),
        _ => None,
    }
}

/// The chain holding the FX at a path, and the FX's place in it.
fn holder_mut<'a>(s: &'a mut State, owner: u64, input: bool, path: &[usize]) -> Option<(&'a mut Vec<FakeFx>, usize)> {
    let (last, parents) = path.split_last()?;
    let mut chain = chain_mut(s, owner, input)?;
    for &place in parents {
        chain = chain.get_mut(place)?.children.as_mut()?;
    }
    Some((chain, *last))
}

fn fx_mut(s: &mut State, owner: u64, index: i32) -> Option<&mut FakeFx> {
    let path = fx_path(s, owner, index);
    let (chain, place) = holder_mut(s, owner, index & INPUT_FX != 0, &path)?;
    chain.get_mut(place)
}

/// Every FX id in the project, at any depth.
fn all_ids(s: &State) -> Vec<String> {
    fn walk(chain: &[FakeFx], out: &mut Vec<String>) {
        for fx in chain {
            out.push(fx.guid.clone());
            if let Some(children) = &fx.children {
                walk(children, out);
            }
        }
    }
    let mut out = Vec::new();
    for track in s.tracks.iter().chain(std::iter::once(&s.master)) {
        walk(&track.fx, &mut out);
        walk(&track.input_fx, &mut out);
        for item in &track.items {
            for take in &item.takes {
                walk(&take.fx, &mut out);
            }
        }
    }
    out
}

fn new_guid(s: &mut State) -> String {
    s.fx_made += 1;
    format!("{{FX-NEW-{}}}", s.fx_made)
}

/// A copy of an FX with new ids for it and everything inside it.
fn renewed(s: &mut State, fx: &FakeFx) -> FakeFx {
    let mut copy = fx.clone();
    copy.guid = new_guid(s);
    if let Some(children) = copy.children.take() {
        copy.children = Some(children.iter().map(|child| renewed(s, child)).collect());
    }
    copy
}

/// The envelopes of an owner (a track's or a take's).
fn envelopes_mut(s: &mut State, owner: u64) -> Option<&mut Vec<FakeEnvelope>> {
    let (kind, a, b, c) = unpack(owner);
    match kind {
        TRACK => Some(if a == MASTER { &mut s.master.envelopes } else { &mut s.tracks.get_mut(a as usize)?.envelopes }),
        TAKE => s.tracks.get_mut(a as usize)?.items.get_mut(b as usize)?.takes.get_mut(c as usize).map(|t| &mut t.envelopes),
        _ => None,
    }
}

fn envelopes_of(s: &State, owner: u64) -> Option<&Vec<FakeEnvelope>> {
    let (kind, _, _, _) = unpack(owner);
    match kind {
        TRACK => track_of(s, owner).map(|t| &t.envelopes),
        TAKE => take_of(s, owner).map(|t| &t.envelopes),
        _ => None,
    }
}

/// Takes an FX's parameter envelopes, and its children's, off its owner: they go with it.
fn drop_envelopes(s: &mut State, owner: u64, fx: &FakeFx) {
    let mut ids = Vec::new();
    fn ids_of(fx: &FakeFx, out: &mut Vec<String>) {
        out.push(fx.guid.clone());
        for child in fx.children.iter().flatten() {
            ids_of(child, out);
        }
    }
    ids_of(fx, &mut ids);
    if let Some(envelopes) = envelopes_mut(s, owner) {
        envelopes.retain(|e| e.fx_param.as_ref().is_none_or(|(guid, _)| !ids.contains(guid)));
    }
}

/// A container's macros: (stable id, the child's id, the child's parameter), in their order.
fn macros(fx: &FakeFx) -> Vec<(i64, String, i32)> {
    fx.config
        .get("cmaps")
        .map(|text| {
            text.split(';')
                .filter_map(|m| {
                    let mut parts = m.split('|');
                    Some((parts.next()?.parse().ok()?, parts.next()?.to_string(), parts.next()?.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn set_macros(fx: &mut FakeFx, list: &[(i64, String, i32)]) {
    let text = list.iter().map(|(h, g, p)| format!("{h}|{g}|{p}")).collect::<Vec<_>>().join(";");
    fx.config.insert("cmaps".into(), text);
}

/// A container's macros that still map one of its own children (REAPER drops a macro whose FX left).
fn prune_macros(chain: &mut [FakeFx]) {
    for fx in chain.iter_mut() {
        if let Some(children) = fx.children.as_mut() {
            prune_macros(children);
        }
        if let Some(children) = fx.children.as_ref() {
            let kept: Vec<_> = macros(fx).into_iter().filter(|(_, guid, _)| children.iter().any(|c| c.guid == *guid)).collect();
            if fx.config.contains_key("cmaps") {
                set_macros(fx, &kept);
            }
        }
    }
}

fn prune_all(s: &mut State) {
    for track in s.tracks.iter_mut().chain(std::iter::once(&mut s.master)) {
        prune_macros(&mut track.fx);
        prune_macros(&mut track.input_fx);
        for item in track.items.iter_mut() {
            for take in item.takes.iter_mut() {
                prune_macros(&mut take.fx);
            }
        }
    }
}

// ----- chunk entries -----

fn entry_text(s: &mut State, owner: u64, fx: &FakeFx) -> String {
    let mut text = format!("BYPASS {} {} 0\n", if fx.enabled { 0 } else { 1 }, if fx.offline { 1 } else { 0 });
    // Equal states share a number, so an unchanged FX reads the same each time (print fences hash chunk text). A
    // plug-in's is inside its block, where REAPER keeps the plug-in's own state; a container's covers the container
    // alone (what is inside it has entries of its own, as in REAPER's text).
    let mut kept = fx.clone();
    if kept.children.is_some() {
        kept.children = Some(vec![]);
    }
    let n = match s.fx_store.iter().position(|stored| *stored == kept) {
        Some(n) => n,
        None => {
            s.fx_store.push(kept);
            s.fx_store.len() - 1
        }
    };
    match &fx.children {
        Some(children) => {
            text += "<CONTAINER Container \"\"\n";
            for child in children {
                text += &entry_text(s, owner, child);
            }
            text += ">\n";
            text += &format!("FAKEFX {n}\n");
        }
        None => {
            text += &format!("<VST \"{}\" fake.vst 0 \"\" 0\n", fx.name);
            for p in &fx.params {
                text += &format!("PARAM {} {}\n", p.value, p.name);
            }
            if let Some(chunk) = &fx.chunk {
                text += &format!("CHUNK {chunk}\n");
            }
            text += &format!("FAKEFX {n}\n");
            text += ">\n";
        }
    }
    if !fx.renamed.is_empty() {
        text += &format!("RENAMED \"{}\"\n", fx.renamed);
    }
    if !fx.preset.is_empty() {
        text += &format!("PRESETNAME \"{}\"\n", fx.preset);
    }
    text += &format!("FLOATPOS 0 0 0 0\nFXID {}\n", fx.guid);
    // Modulation, by parameter, as REAPER writes it after the id.
    let mut params: Vec<i32> = fx.config.keys().filter_map(|k| k.strip_prefix("param.")?.split('.').next()?.parse().ok()).collect();
    params.sort_unstable();
    params.dedup();
    for p in params {
        text += &format!("<PROGRAMENV {p}\n");
        let mut keys: Vec<&String> = fx.config.keys().filter(|k| k.starts_with(&format!("param.{p}."))).collect();
        keys.sort();
        for key in keys {
            text += &format!("KEY {} {}\n", key, fx.config[key]);
        }
        text += ">\n";
    }
    let mut others: Vec<&String> = fx.config.keys().filter(|k| !k.starts_with("param.")).collect();
    others.sort();
    for key in others {
        text += &format!("CONFIG {} {}\n", key, fx.config[key]);
    }
    // Its parameter envelopes are inside its entry.
    for envelope in envelopes_of(s, owner).into_iter().flatten().filter(|e| e.fx_param.as_ref().is_some_and(|(g, _)| *g == fx.guid)) {
        let param = envelope.fx_param.as_ref().map_or(0, |(_, p)| *p);
        text += &format!("<PARMENV {param} {} {} 0\n", envelope.range.0, envelope.range.1);
        text += &format!(
            "EGUID {}\nACT {} -1\nVIS {} 1 1\nARM {}\n",
            envelope.guid, envelope.active as i32, envelope.visible as i32, envelope.armed as i32
        );
        for point in points_of(envelope) {
            text += &format!("PT {} {} {}\n", point.time, point.value, point.shape);
        }
        text += ">\n";
    }
    text += "WAK 0 0\n";
    if fx.parallel != 0 {
        text += &format!("PARALLEL {}\n", fx.parallel);
    }
    text
}

/// An FX chain file's (or entry's) FX: each from the FX its `FAKEFX` line names, with what the text says over it, and
/// the parameter envelopes inside (made again, with new ids, as REAPER makes them).
struct Parsed {
    fx: FakeFx,
    envelopes: Vec<FakeEnvelope>,
}

fn parse_entries(s: &mut State, lines: &[&str], at: &mut usize, end_at_close: bool) -> Vec<Parsed> {
    let mut out: Vec<Parsed> = Vec::new();
    while *at < lines.len() {
        let line = lines[*at].trim_end();
        if line == ">" && end_at_close {
            *at += 1;
            return out;
        }
        if let Some(rest) = line.strip_prefix("BYPASS ") {
            let mut flags = rest.split_whitespace();
            let enabled = flags.next() == Some("0");
            let offline = flags.next() == Some("1");
            *at += 1;
            let mut fx = FakeFx::new("", "");
            let mut params: Vec<(f64, String)> = Vec::new();
            let mut children = None;
            let mut chunk = None;
            let mut envelopes = Vec::new();
            let mut config = HashMap::new();
            let (mut guid, mut parallel, mut renamed, mut preset, mut stored) = (None, 0, String::new(), String::new(), None);
            while *at < lines.len() {
                let line = lines[*at].trim_end();
                if line.starts_with("BYPASS ") || (line == ">" && end_at_close) {
                    break;
                }
                *at += 1;
                if line.starts_with("<CONTAINER") {
                    children = Some(parse_entries(s, lines, at, true));
                } else if let Some(rest) = line.strip_prefix("<VST \"") {
                    fx.name = rest.split('"').next().unwrap_or_default().to_string();
                    while *at < lines.len() && lines[*at].trim_end() != ">" {
                        let inner = lines[*at].trim_end();
                        if let Some(rest) = inner.strip_prefix("PARAM ") {
                            if let Some((value, name)) = rest.split_once(' ') {
                                params.push((value.parse().unwrap_or(0.0), name.to_string()));
                            }
                        } else if let Some(rest) = inner.strip_prefix("CHUNK ") {
                            chunk = Some(rest.to_string());
                        } else if let Some(n) = inner.strip_prefix("FAKEFX ") {
                            stored = n.trim().parse::<usize>().ok();
                        }
                        *at += 1;
                    }
                    *at += 1;
                } else if let Some(n) = line.strip_prefix("FAKEFX ") {
                    stored = n.trim().parse::<usize>().ok();
                } else if let Some(name) = line.strip_prefix("RENAMED \"") {
                    renamed = name.trim_end_matches('"').to_string();
                } else if let Some(name) = line.strip_prefix("PRESETNAME \"") {
                    preset = name.trim_end_matches('"').to_string();
                } else if let Some(id) = line.strip_prefix("FXID ") {
                    guid = Some(id.trim().to_string());
                } else if line.starts_with("<PROGRAMENV") {
                    while *at < lines.len() && lines[*at].trim_end() != ">" {
                        if let Some(rest) = lines[*at].trim_end().strip_prefix("KEY ") {
                            if let Some((key, value)) = rest.split_once(' ') {
                                config.insert(key.to_string(), value.to_string());
                            }
                        }
                        *at += 1;
                    }
                    *at += 1;
                } else if let Some(rest) = line.strip_prefix("CONFIG ") {
                    if let Some((key, value)) = rest.split_once(' ') {
                        config.insert(key.to_string(), value.to_string());
                    }
                } else if let Some(rest) = line.strip_prefix("<PARMENV ") {
                    let words: Vec<&str> = rest.split_whitespace().collect();
                    let param = words.first().and_then(|w| w.split(':').next()?.parse::<i32>().ok()).unwrap_or(0);
                    let mut envelope = FakeEnvelope::for_param("", "", param, 0);
                    envelope.range = (
                        words.get(1).and_then(|w| w.parse().ok()).unwrap_or(0.0),
                        words.get(2).and_then(|w| w.parse().ok()).unwrap_or(1.0),
                    );
                    let mut points = Vec::new();
                    while *at < lines.len() && lines[*at].trim_end() != ">" {
                        let inner = lines[*at].trim_end();
                        let fields: Vec<&str> = inner.split_whitespace().collect();
                        match fields.first().copied() {
                            Some("ACT") => envelope.active = fields.get(1) == Some(&"1"),
                            Some("VIS") => envelope.visible = fields.get(1) == Some(&"1"),
                            Some("ARM") => envelope.armed = fields.get(1) == Some(&"1"),
                            Some("PT") => points.push(FakePoint {
                                time: fields.get(1).and_then(|f| f.parse().ok()).unwrap_or(0.0),
                                value: fields.get(2).and_then(|f| f.parse().ok()).unwrap_or(0.0),
                                shape: fields.get(3).and_then(|f| f.parse().ok()).unwrap_or(0),
                                tension: 0.0,
                                selected: false,
                            }),
                            _ => {}
                        }
                        *at += 1;
                    }
                    *at += 1;
                    envelope.pts = Some(points);
                    envelopes.push(envelope);
                } else if let Some(n) = line.strip_prefix("PARALLEL ") {
                    parallel = n.trim().parse().unwrap_or(0);
                }
            }
            if let Some(stored) = stored.and_then(|n| s.fx_store.get(n).cloned()) {
                fx = FakeFx { children: None, ..stored };
            }
            fx.enabled = enabled;
            fx.offline = offline;
            fx.parallel = parallel;
            fx.renamed = renamed;
            fx.preset = preset;
            fx.config = config;
            if chunk.is_some() {
                fx.chunk = chunk;
            }
            for (n, (value, name)) in params.into_iter().enumerate() {
                match fx.params.get_mut(n) {
                    Some(p) => {
                        p.value = value;
                        p.shown = p.text_for(value);
                    }
                    None => fx.params.push(FakeParam::new(&name, value, "")),
                }
            }
            let (children, child_envelopes): (Option<Vec<FakeFx>>, Vec<FakeEnvelope>) = match children {
                Some(list) => {
                    let mut envelopes = Vec::new();
                    let fxs = list
                        .into_iter()
                        .map(|p| {
                            envelopes.extend(p.envelopes);
                            p.fx
                        })
                        .collect();
                    (Some(fxs), envelopes)
                }
                None => (None, Vec::new()),
            };
            if children.is_some() {
                fx.children = children;
            }
            // REAPER keeps a chain file's id only when no FX in the project has it.
            fx.guid = match guid {
                Some(id) if !all_ids(s).contains(&id) => id,
                _ => new_guid(s),
            };
            for envelope in envelopes.iter_mut() {
                envelope.fx_param = Some((fx.guid.clone(), envelope.fx_param.as_ref().map_or(0, |(_, p)| *p)));
            }
            envelopes.extend(child_envelopes);
            out.push(Parsed { fx, envelopes });
            continue;
        }
        *at += 1;
    }
    out
}

fn parse_chain(s: &mut State, text: &str) -> Vec<Parsed> {
    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0;
    parse_entries(s, &lines, &mut at, false)
}

/// A chain's entries as REAPER writes a chain block of a chunk.
pub(super) fn chain_block(s: &mut State, owner: u64, name: &str, chain: &[FakeFx]) -> String {
    if chain.is_empty() {
        return String::new();
    }
    let mut text = format!("<{name}\nSHOW 0\nLASTSEL 0\nDOCKED 0\n");
    for fx in chain {
        text += &entry_text(s, owner, fx);
    }
    text + ">\n"
}

// ----- adds, deletes, copies -----

/// REAPER keeps a parameter link's source by its place in the chain and follows it as FX come and go (7.82 lab): a link
/// to an FX that is deleted is cleared, one to an FX after a place that changed moves with it.
fn follow_links(chain: &mut [FakeFx], place: usize, inserted: Option<usize>) {
    for fx in chain.iter_mut() {
        let linked: Vec<String> = fx.config.keys().filter(|k| k.ends_with(".plink.effect")).cloned().collect();
        for key in linked {
            let Some(effect) = fx.config.get(&key).and_then(|v| v.parse::<i64>().ok()).filter(|e| *e >= 0) else { continue };
            let effect = effect as usize;
            let moved = match inserted {
                Some(count) if effect >= place => Some(effect + count),
                Some(_) => None,
                None if effect == place => {
                    fx.config.insert(key.replace(".plink.effect", ".plink.active"), "0".into());
                    fx.config.insert(key.clone(), "-1".into());
                    None
                }
                None if effect > place => Some(effect - 1),
                None => None,
            };
            if let Some(moved) = moved {
                fx.config.insert(key, moved.to_string());
            }
        }
    }
}

/// Puts FX into a chain at an address (REAPER's `-1000 - address`, or the end): the place in its chain the first went to.
fn insert(s: &mut State, owner: u64, input: bool, address: Option<i32>, made: Vec<Parsed>) -> c_int {
    let path = match address {
        Some(a) if a & CONTAINER_FX != 0 => fx_path(s, owner, a),
        Some(a) => vec![(a & !INPUT_FX) as usize],
        None => vec![chain_mut(s, owner, input).map_or(0, |c| c.len())],
    };
    let input = input || address.is_some_and(|a| a & INPUT_FX != 0);
    let mut envelopes = Vec::new();
    let place = {
        let Some((chain, place)) = holder_mut(s, owner, input, &path) else { return -1 };
        let place = place.min(chain.len());
        if path.len() == 1 {
            follow_links(chain, place, Some(made.len()));
        }
        for (n, parsed) in made.into_iter().enumerate() {
            envelopes.extend(parsed.envelopes);
            chain.insert(place + n, parsed.fx);
        }
        place
    };
    // (REAPER makes them again with new ids: lab)
    s.made += 1;
    let serial = s.made;
    if let Some(list) = envelopes_mut(s, owner) {
        for (n, mut envelope) in envelopes.into_iter().enumerate() {
            envelope.guid = format!("{{ENV-MADE-{serial}-{n}}}");
            envelope.name = format!("Param {} / restored", envelope.fx_param.as_ref().map_or(0, |(_, p)| *p));
            list.push(envelope);
        }
    }
    s.undo_dirty = true;
    place as c_int
}

/// What a name makes: an FX chain file's FX, a container, or a plug-in REAPER knows.
fn made_by(s: &mut State, name: &str) -> Option<Vec<Parsed>> {
    if name.to_ascii_lowercase().ends_with(".rfxchain") {
        // A chain file is found by a name in REAPER's FXChains folder only (lab: an absolute path adds nothing).
        if Path::new(name).is_absolute() {
            return None;
        }
        let text = std::fs::read_to_string(Path::new(&s.resource).join("FXChains").join(name)).ok()?;
        let made = parse_chain(s, &text);
        return (!made.is_empty()).then_some(made);
    }
    if name == "Container" {
        let guid = new_guid(s);
        return Some(vec![Parsed { fx: FakeFx::container(&guid, vec![]), envelopes: vec![] }]);
    }
    let mut made = vec![super::new_fx(s, name)?];
    // A name that stands for several plug-ins (as "FXADD:" does) adds them all.
    if name.to_ascii_lowercase().contains("twice") {
        made.extend(super::new_fx(s, name));
    }
    Some(made.into_iter().map(|fx| Parsed { fx, envelopes: vec![] }).collect())
}

fn add_by_name(owner: u64, name: &str, input: bool, instantiate: c_int) -> c_int {
    counted(|s| {
        if instantiate == 0 {
            return -1;
        }
        let Some(made) = made_by(s, name) else { return -1 };
        let address = (instantiate <= -1000).then_some(-1000 - instantiate);
        insert(s, owner, input, address, made)
    })
}

unsafe extern "C" fn track_fx_add_by_name(track: *mut MediaTrack, name: *const c_char, rec: bool, instantiate: c_int) -> c_int {
    add_by_name(track as u64, &name_of(name), rec, instantiate)
}

unsafe extern "C" fn take_fx_add_by_name(take: *mut MediaItem_Take, name: *const c_char, instantiate: c_int) -> c_int {
    add_by_name(take as u64, &name_of(name), false, instantiate)
}

fn delete(owner: u64, index: c_int) -> bool {
    counted(|s| {
        if s.fail_fx_delete {
            return false;
        }
        let path = fx_path(s, owner, index);
        let Some((chain, place)) = holder_mut(s, owner, index & INPUT_FX != 0, &path) else { return false };
        if place >= chain.len() {
            return false;
        }
        let removed = chain.remove(place);
        if path.len() == 1 {
            follow_links(chain, place, None);
        }
        drop_envelopes(s, owner, &removed);
        prune_all(s);
        s.undo_dirty = true;
        true
    })
}

unsafe extern "C" fn track_fx_delete(track: *mut MediaTrack, fx: c_int) -> bool {
    delete(track as u64, fx)
}

unsafe extern "C" fn take_fx_delete(take: *mut MediaItem_Take, fx: c_int) -> bool {
    delete(take as u64, fx)
}

/// REAPER's copy and move between chains: the FX lands at the destination address (the place it ends up at).
fn copy(src: u64, from: c_int, dest: u64, to: c_int, is_move: bool) {
    counted(|s| {
        let into_container = to & CONTAINER_FX != 0;
        let from_path = fx_path(s, src, from);
        let to_path = if into_container { fx_path(s, dest, to) } else { vec![(to & !INPUT_FX) as usize] };
        let same_container = from_path.len() > 1
            && from_path.len() == to_path.len()
            && from_path[..from_path.len() - 1] == to_path[..to_path.len() - 1]
            && src == dest;
        if into_container && !same_container {
            // Real REAPER hangs here (lab, 7.82): the changes must never ask it.
            s.copies_into_containers += 1;
            return;
        }
        let Some(fx) = fx_of(s, src, from).cloned() else { return };
        let fx = if is_move {
            let Some((chain, place)) = holder_mut(s, src, from & INPUT_FX != 0, &from_path) else { return };
            chain.remove(place)
        } else {
            renewed(s, &fx)
        };
        let Some((chain, place)) = holder_mut(s, dest, to & INPUT_FX != 0, &to_path) else { return };
        let place = place.min(chain.len());
        chain.insert(place, fx);
        if is_move {
            // Its parameter envelopes go with it to another owner.
            if src != dest {
                let guid = chain[place].guid.clone();
                let moving: Vec<FakeEnvelope> = envelopes_mut(s, src)
                    .map(|list| {
                        let (go, stay): (Vec<_>, Vec<_>) =
                            list.drain(..).partition(|e| e.fx_param.as_ref().is_some_and(|(g, _)| *g == guid));
                        *list = stay;
                        go
                    })
                    .unwrap_or_default();
                if let Some(list) = envelopes_mut(s, dest) {
                    list.extend(moving);
                }
            }
            prune_all(s);
        }
        s.undo_dirty = true;
    })
}

unsafe extern "C" fn track_fx_copy_to_track(src: *mut MediaTrack, from: c_int, dest: *mut MediaTrack, to: c_int, is_move: bool) {
    copy(src as u64, from, dest as u64, to, is_move)
}
unsafe extern "C" fn track_fx_copy_to_take(src: *mut MediaTrack, from: c_int, dest: *mut MediaItem_Take, to: c_int, is_move: bool) {
    copy(src as u64, from, dest as u64, to, is_move)
}
unsafe extern "C" fn take_fx_copy_to_track(src: *mut MediaItem_Take, from: c_int, dest: *mut MediaTrack, to: c_int, is_move: bool) {
    copy(src as u64, from, dest as u64, to, is_move)
}
unsafe extern "C" fn take_fx_copy_to_take(src: *mut MediaItem_Take, from: c_int, dest: *mut MediaItem_Take, to: c_int, is_move: bool) {
    copy(src as u64, from, dest as u64, to, is_move)
}

// ----- an FX's state -----

/// Whether REAPER records a write to this owner's FX into an envelope: its track is in touch, latch or write mode (or the
/// override is), and the override isn't read or trim.
fn records(s: &State, owner: u64) -> bool {
    let (kind, a, _, _) = unpack(owner);
    if kind != TRACK {
        return false;
    }
    let mode = track_of(s, owner).and_then(|t| t.extra.get("I_AUTOMODE")).copied().unwrap_or(0.0) as i32;
    let writes = |m: i32| (2..=5).contains(&m);
    let _ = a;
    writes(s.automation_override) || ((s.automation_override == -1 || s.automation_override == 6) && writes(mode))
}

/// What REAPER records of a write in touch mode: a point on the parameter's envelope (made if it has none).
fn record(s: &mut State, owner: u64, guid: &str, param: i32, value: f64) {
    let Some(list) = envelopes_mut(s, owner) else { return };
    match list.iter_mut().find(|e| e.fx_param.as_ref().is_some_and(|(g, p)| g == guid && *p == param)) {
        Some(envelope) => points_mut(envelope).push(FakePoint::at(0.0, value)),
        None => {
            let mut envelope = FakeEnvelope::for_param("Recorded", guid, param, 0);
            envelope.pts = Some(vec![FakePoint::at(0.0, value)]);
            list.push(envelope);
        }
    }
}

fn set_enabled(owner: u64, index: c_int, enabled: bool) {
    counted(|s| {
        let recording = records(s, owner);
        let Some(fx) = fx_mut(s, owner, index) else { return };
        fx.enabled = enabled;
        let guid = fx.guid.clone();
        if recording {
            record(s, owner, &guid, -1, if enabled { 0.0 } else { 1.0 });
        }
        s.undo_dirty = true;
    })
}

unsafe extern "C" fn track_fx_set_enabled(track: *mut MediaTrack, fx: c_int, enabled: bool) {
    set_enabled(track as u64, fx, enabled)
}
unsafe extern "C" fn take_fx_set_enabled(take: *mut MediaItem_Take, fx: c_int, enabled: bool) {
    set_enabled(take as u64, fx, enabled)
}

fn set_offline(owner: u64, index: c_int, offline: bool) {
    counted(|s| {
        if let Some(fx) = fx_mut(s, owner, index) {
            fx.offline = offline;
            s.undo_dirty = true;
        }
    })
}

unsafe extern "C" fn track_fx_set_offline(track: *mut MediaTrack, fx: c_int, offline: bool) {
    set_offline(track as u64, fx, offline)
}
unsafe extern "C" fn take_fx_set_offline(take: *mut MediaItem_Take, fx: c_int, offline: bool) {
    set_offline(take as u64, fx, offline)
}

/// A parameter of an FX as REAPER numbers them: a container's macros come first, then its own.
enum Param<'a> {
    Own(&'a FakeParam),
    /// A container's macro: the child and its parameter.
    Macro(i32, i32),
}

fn param_of(s: &State, owner: u64, index: c_int, param: c_int) -> Option<Param<'_>> {
    let fx = fx_of(s, owner, index)?;
    let maps = macros(fx);
    if (param as usize) < maps.len() {
        let (_, guid, p) = &maps[param as usize];
        let child = fx.children.as_ref()?.iter().position(|c| c.guid == *guid)?;
        let path = [fx_path(s, owner, index), vec![child]].concat();
        return Some(Param::Macro(fx_address(s, owner, &path, index & INPUT_FX != 0), *p));
    }
    fx.params.get(param as usize - maps.len()).map(Param::Own)
}

fn num_params(owner: u64, index: c_int) -> c_int {
    counted(|s| fx_of(s, owner, index).map_or(0, |fx| (fx.params.len() + macros(fx).len()) as c_int))
}
unsafe extern "C" fn track_fx_get_num_params(t: *mut MediaTrack, fx: c_int) -> c_int {
    num_params(t as u64, fx)
}
unsafe extern "C" fn take_fx_get_num_params(t: *mut MediaItem_Take, fx: c_int) -> c_int {
    num_params(t as u64, fx)
}

fn param_name(owner: u64, index: c_int, param: c_int, buf: *mut c_char, size: c_int) -> bool {
    let name = counted(|s| match param_of(s, owner, index, param)? {
        Param::Own(p) => Some(p.name.clone()),
        Param::Macro(child, p) => {
            let fx = fx_of(s, owner, child)?;
            Some(format!("{}: {}", fx.name, fx.params.get(p as usize)?.name))
        }
    });
    name.map(|name| write(buf, size, &name)).is_some()
}
unsafe extern "C" fn track_fx_get_param_name(t: *mut MediaTrack, fx: c_int, p: c_int, buf: *mut c_char, size: c_int) -> bool {
    param_name(t as u64, fx, p, buf, size)
}
unsafe extern "C" fn take_fx_get_param_name(t: *mut MediaItem_Take, fx: c_int, p: c_int, buf: *mut c_char, size: c_int) -> bool {
    param_name(t as u64, fx, p, buf, size)
}

fn param_normalized(owner: u64, index: c_int, param: c_int) -> f64 {
    counted(|s| {
        let calls = s.calls;
        match param_of(s, owner, index, param) {
            Some(Param::Own(p)) if p.wobbles => (calls % 997) as f64 / 1000.0,
            Some(Param::Own(p)) => p.value,
            Some(Param::Macro(child, p)) => fx_of(s, owner, child).and_then(|fx| fx.params.get(p as usize)).map_or(0.0, |p| p.value),
            None => 0.0,
        }
    })
}
unsafe extern "C" fn track_fx_get_param_normalized(t: *mut MediaTrack, fx: c_int, p: c_int) -> f64 {
    param_normalized(t as u64, fx, p)
}
unsafe extern "C" fn take_fx_get_param_normalized(t: *mut MediaItem_Take, fx: c_int, p: c_int) -> f64 {
    param_normalized(t as u64, fx, p)
}

fn set_param(owner: u64, index: c_int, param: c_int, value: f64) -> bool {
    counted(|s| {
        let (index, param) = match param_of(s, owner, index, param) {
            Some(Param::Macro(child, p)) => (child, p),
            Some(Param::Own(_)) => (index, param - fx_of(s, owner, index).map_or(0, |fx| macros(fx).len() as c_int)),
            None => return false,
        };
        let recording = records(s, owner);
        let Some(fx) = fx_mut(s, owner, index) else { return false };
        let guid = fx.guid.clone();
        let Some(p) = fx.params.get_mut(param as usize) else { return false };
        let before = p.value;
        p.value = value.clamp(0.0, 1.0);
        p.shown = p.text_for(p.value);
        let value = p.value;
        if recording {
            record(s, owner, &guid, param, value);
        }
        if s.undo_depth > 0 {
            s.block_params.entry((guid, param)).or_insert((before, value)).1 = value;
        } else {
            s.undo_dirty = true;
        }
        true
    })
}
unsafe extern "C" fn track_fx_set_param_normalized(track: *mut MediaTrack, fx: c_int, param: c_int, value: f64) -> bool {
    set_param(track as u64, fx, param, value)
}
unsafe extern "C" fn take_fx_set_param_normalized(take: *mut MediaItem_Take, fx: c_int, param: c_int, value: f64) -> bool {
    set_param(take as u64, fx, param, value)
}

fn own_param(s: &State, owner: u64, index: c_int, param: c_int) -> Option<FakeParam> {
    match param_of(s, owner, index, param)? {
        Param::Own(p) => Some(p.clone()),
        Param::Macro(child, p) => fx_of(s, owner, child)?.params.get(p as usize).cloned(),
    }
}

fn formatted(owner: u64, index: c_int, param: c_int, buf: *mut c_char, size: c_int) -> bool {
    counted(|s| {
        own_param(s, owner, index, param)
            .map(|p| match &p.display {
                Display::Linear { unit, .. } if p.bare => write(buf, size, p.shown.trim_end_matches(unit.as_str()).trim_end()),
                _ => write(buf, size, &p.shown),
            })
            .is_some()
    })
}
unsafe extern "C" fn track_fx_get_formatted_param_value(t: *mut MediaTrack, fx: c_int, p: c_int, buf: *mut c_char, size: c_int) -> bool {
    formatted(t as u64, fx, p, buf, size)
}
unsafe extern "C" fn take_fx_get_formatted_param_value(t: *mut MediaItem_Take, fx: c_int, p: c_int, buf: *mut c_char, size: c_int) -> bool {
    formatted(t as u64, fx, p, buf, size)
}

fn format_normalized(owner: u64, index: c_int, param: c_int, value: f64, buf: *mut c_char, size: c_int) -> bool {
    counted(|s| own_param(s, owner, index, param).map(|p| write(buf, size, &p.text_for(value))).is_some())
}
unsafe extern "C" fn track_fx_format_param_value_normalized(
    t: *mut MediaTrack,
    fx: c_int,
    p: c_int,
    v: f64,
    buf: *mut c_char,
    size: c_int,
) -> bool {
    format_normalized(t as u64, fx, p, v, buf, size)
}
unsafe extern "C" fn take_fx_format_param_value_normalized(
    t: *mut MediaItem_Take,
    fx: c_int,
    p: c_int,
    v: f64,
    buf: *mut c_char,
    size: c_int,
) -> bool {
    format_normalized(t as u64, fx, p, v, buf, size)
}

/// A parameter in its own units (`min` to `max` over its normalized range): a JSFX slider's value.
fn get_raw(owner: u64, index: c_int, param: c_int, min: *mut f64, max: *mut f64) -> f64 {
    counted(|s| match own_param(s, owner, index, param) {
        Some(p) => {
            unsafe {
                if !min.is_null() {
                    *min = p.min;
                }
                if !max.is_null() {
                    *max = p.max;
                }
            }
            p.min + p.value * (p.max - p.min)
        }
        None => 0.0,
    })
}
unsafe extern "C" fn track_fx_get_param(t: *mut MediaTrack, fx: c_int, p: c_int, min: *mut f64, max: *mut f64) -> f64 {
    get_raw(t as u64, fx, p, min, max)
}
unsafe extern "C" fn take_fx_get_param(t: *mut MediaItem_Take, fx: c_int, p: c_int, min: *mut f64, max: *mut f64) -> f64 {
    get_raw(t as u64, fx, p, min, max)
}

fn set_raw(owner: u64, index: c_int, param: c_int, value: f64) -> bool {
    let range = counted(|s| own_param(s, owner, index, param).map(|p| (p.min, p.max)));
    match range {
        Some((min, max)) if max != min => set_param(owner, index, param, (value - min) / (max - min)),
        _ => false,
    }
}
unsafe extern "C" fn track_fx_set_param(t: *mut MediaTrack, fx: c_int, p: c_int, v: f64) -> bool {
    set_raw(t as u64, fx, p, v)
}
unsafe extern "C" fn take_fx_set_param(t: *mut MediaItem_Take, fx: c_int, p: c_int, v: f64) -> bool {
    set_raw(t as u64, fx, p, v)
}

/// What REAPER holds once a parameter's modulation block is made.
const MOD_DEFAULTS: [(&str, &str); 28] = [
    ("mod.active", "0"),
    ("mod.baseline", "0"),
    ("mod.visible", "0"),
    ("lfo.active", "0"),
    ("lfo.dir", "1"),
    ("lfo.phase", "0"),
    ("lfo.speed", "1"),
    ("lfo.strength", "1"),
    ("lfo.temposync", "0"),
    ("lfo.free", "0"),
    ("lfo.shape", "0"),
    ("acs.active", "0"),
    ("acs.dir", "1"),
    ("acs.strength", "1"),
    ("acs.attack", "300"),
    ("acs.release", "300"),
    ("acs.dblo", "-24"),
    ("acs.dbhi", "0"),
    ("acs.chan", "-1"),
    ("acs.stereo", "0"),
    ("acs.x2", "0.5"),
    ("acs.y2", "0.5"),
    ("plink.active", "0"),
    ("plink.scale", "1"),
    ("plink.offset", "0"),
    ("plink.effect", "-1"),
    ("plink.param", "-1"),
    ("plink.midi_bus", "0"),
];

fn config_get(owner: u64, index: c_int, key: &str) -> Option<String> {
    counted(|s| {
        let fx = fx_of(s, owner, index)?;
        let path = fx_path(s, owner, index);
        let input = index & INPUT_FX != 0;
        match key {
            "container_count" => return fx.children.as_ref().map(|c| c.len().to_string()),
            "parallel" => return Some(fx.parallel.to_string()),
            "renamed_name" => return Some(fx.renamed.clone()),
            "fx_type" => {
                return Some(if fx.children.is_some() {
                    "Container".into()
                } else {
                    fx.name.split(':').next().unwrap_or("").trim().to_string()
                })
            }
            // The fake's state chunk is its parameters' values (see `chunk_of`), for a plug-in that gives one.
            "vst_chunk" => return fx.chunk.as_ref().map(|_| chunk_of(&fx.params.iter().map(|p| p.value).collect::<Vec<_>>())),
            "parent_container" => {
                return (path.len() > 1).then(|| fx_address(s, owner, &path[..path.len() - 1], input).to_string());
            }
            _ => {}
        }
        if let Some(n) = key.strip_prefix("container_item.").and_then(|n| n.parse::<usize>().ok()) {
            let children = fx.children.as_ref()?;
            return (n < children.len()).then(|| fx_address(s, owner, &[path.as_slice(), &[n]].concat(), input).to_string());
        }
        if let Some(rest) = key.strip_prefix("container_map.get.") {
            let (child, param) = rest.split_once('.')?;
            let (child, param): (usize, i32) = (child.parse().ok()?, param.parse().ok()?);
            let guid = fx.children.as_ref()?.get(child)?.guid.clone();
            return macros(fx).iter().position(|(_, g, p)| *g == guid && *p == param).map(|n| n.to_string());
        }
        // `param.N.…`: a macro's mapping, or the parameter's modulation (none until its block is made).
        if let Some(rest) = key.strip_prefix("param.") {
            let (param, field) = rest.split_once('.')?;
            let param: usize = param.parse().ok()?;
            if let Some(what) = field.strip_prefix("container_map.") {
                let (hint, guid, p) = macros(fx).get(param)?.clone();
                let child = fx.children.as_ref()?.iter().position(|c| c.guid == guid)?;
                return match what {
                    "hint_id" => Some(hint.to_string()),
                    "fx_index" => Some(child.to_string()),
                    "fx_parm" => Some(p.to_string()),
                    _ => None,
                };
            }
            let made = fx.config.keys().any(|k| k.starts_with(&format!("param.{param}.")) && !k.contains(".learn."));
            if let Some(value) = fx.config.get(key) {
                return Some(value.clone());
            }
            if made && !field.starts_with("learn.") {
                return MOD_DEFAULTS.iter().find(|(f, _)| *f == field).map(|(_, v)| v.to_string());
            }
            return None;
        }
        fx.config.get(key).cloned()
    })
}

fn named_config(owner: u64, index: c_int, key: *const c_char, buf: *mut c_char, size: c_int) -> bool {
    let key = name_of(key);
    // `container_map.add.C.P` maps as it is read (REAPER's way): the new macro's index.
    if let Some(rest) = key.strip_prefix("container_map.add.") {
        let answer = counted(|s| {
            let (child, param) = rest.split_once('.')?;
            let (child, param): (usize, i32) = (child.parse().ok()?, param.parse().ok()?);
            let fx = fx_mut(s, owner, index)?;
            let guid = fx.children.as_ref()?.get(child)?.guid.clone();
            let mut list = macros(fx);
            if let Some(n) = list.iter().position(|(_, g, p)| *g == guid && *p == param) {
                return Some(n);
            }
            let hint = fx.config.get("cmaps.next").and_then(|n| n.parse::<i64>().ok()).unwrap_or(1);
            fx.config.insert("cmaps.next".into(), (hint + 1).to_string());
            list.push((hint, guid, param));
            set_macros(fx, &list);
            s.undo_dirty = true;
            Some(list.len() - 1)
        });
        return answer.map(|n| write(buf, size, &n.to_string())).is_some();
    }
    // `param.X.container_map.delete` unmaps as it is read; REAPER keeps no undo point of it.
    if let Some((param, "container_map.delete")) = key.strip_prefix("param.").and_then(|rest| rest.split_once('.')) {
        let answer = counted(|s| {
            let param: usize = param.parse().ok()?;
            let fx = fx_mut(s, owner, index)?;
            let mut list = macros(fx);
            if param >= list.len() {
                return None;
            }
            list.remove(param);
            set_macros(fx, &list);
            s.state_changed = true;
            Some(())
        });
        return answer.map(|_| write(buf, size, "0")).is_some();
    }
    match config_get(owner, index, &key) {
        Some(value) => {
            write(buf, size, &value);
            true
        }
        None => false,
    }
}
unsafe extern "C" fn track_fx_get_named_config_parm(
    t: *mut MediaTrack,
    fx: c_int,
    key: *const c_char,
    buf: *mut c_char,
    size: c_int,
) -> bool {
    named_config(t as u64, fx, key, buf, size)
}
unsafe extern "C" fn take_fx_get_named_config_parm(
    t: *mut MediaItem_Take,
    fx: c_int,
    key: *const c_char,
    buf: *mut c_char,
    size: c_int,
) -> bool {
    named_config(t as u64, fx, key, buf, size)
}

/// The fake's state chunk for these parameter values: base64's letters, as a real chunk has (each value's bits in hex,
/// joined by "+").
pub fn chunk_of(values: &[f64]) -> String {
    values.iter().map(|v| format!("{:016x}", v.to_bits())).collect::<Vec<_>>().join("+")
}

/// The modulation keys REAPER clamps when it reads them back (measured): what they read for a value.
fn clamped(field: &str, value: &str) -> String {
    let number = value.parse::<f64>().ok();
    let clamp = |lo: f64, hi: f64| number.map_or(value.to_string(), |n| format!("{}", n.clamp(lo, hi)));
    match field {
        "lfo.phase" => clamp(0.0, 1.0),
        "acs.attack" | "acs.release" => clamp(0.0, 1000.0),
        "acs.dblo" => clamp(-60.0, -0.1),
        "acs.stereo" => clamp(0.0, 1.0),
        _ => value.to_string(),
    }
}

fn set_config(owner: u64, index: c_int, key: *const c_char, value: *const c_char) -> bool {
    let (key, value) = (name_of(key), name_of(value));
    counted(|s| {
        let Some(fx) = fx_mut(s, owner, index) else { return false };
        match key.as_str() {
            "parallel" => {
                fx.parallel = value.parse().unwrap_or(0);
                s.undo_dirty = true;
                true
            }
            "renamed_name" => {
                fx.renamed = value;
                s.undo_dirty = true;
                true
            }
            "vst_chunk" => {
                if fx.chunk.is_none() {
                    return false;
                }
                // The fake's state chunk is its parameters' values: setting it sets them (and clears the preset's name, as
                // REAPER's does). No undo point.
                for (p, v) in fx.params.iter_mut().zip(value.split('+')) {
                    if let Some(v) = u64::from_str_radix(v, 16).ok().map(f64::from_bits) {
                        p.value = v;
                        p.shown = p.text_for(v);
                    }
                }
                fx.chunk = Some(value);
                fx.preset.clear();
                s.state_changed = true;
                true
            }
            // (m6-library) A sampler's file is named, then taken with DONE (RS5k's way): read back only after.
            file if file.len() > 4 && file.starts_with("FILE") && file[4..].bytes().all(|b| b.is_ascii_digit()) => {
                fx.config.insert(format!("pending.{file}"), value);
                true
            }
            "DONE" => {
                if super::extra(|e| e.sampler_refuses_files) {
                    return true;
                }
                let pending: Vec<(String, String)> =
                    fx.config.iter().filter(|(k, _)| k.starts_with("pending.FILE")).map(|(k, v)| (k.clone(), v.clone())).collect();
                for (key, file) in pending {
                    fx.config.remove(&key);
                    fx.config.insert(key["pending.".len()..].to_string(), file);
                }
                s.state_changed = true;
                true
            }
            band if band.starts_with("BANDTYPE") || band.starts_with("BANDENABLED") => {
                // ReaEQ answers false and takes it.
                if fx.config.contains_key(band) {
                    fx.config.insert(band.to_string(), value);
                }
                false
            }
            other => {
                if let Some((param, field)) = other.strip_prefix("param.").and_then(|r| r.split_once('.')) {
                    let first = !fx.config.keys().any(|k| k.starts_with(&format!("param.{param}.")) && !k.contains(".learn."));
                    // REAPER's state holds a parameter's modulation only while a source of it is on, and its MIDI learn.
                    let sourced = |fx: &FakeFx| {
                        ["lfo.active", "acs.active", "plink.active"]
                            .iter()
                            .any(|k| fx.config.get(&format!("param.{param}.{k}")).is_some_and(|v| v == "1"))
                    };
                    let had = sourced(fx);
                    fx.config.insert(other.to_string(), clamped(field, &value));
                    let held = had || sourced(fx) || field.starts_with("learn.");
                    // An LFO or a link turned on makes the block and turns the modulation on with it; REAPER marks only those.
                    if (field == "lfo.active" || field == "plink.active") && value == "1" {
                        if first {
                            fx.config.insert(format!("param.{param}.mod.active"), "1".into());
                        }
                        s.undo_dirty = true;
                    }
                    s.state_changed |= held;
                    return true;
                }
                fx.config.insert(other.to_string(), value);
                true
            }
        }
    })
}
unsafe extern "C" fn track_fx_set_named_config_parm(t: *mut MediaTrack, fx: c_int, key: *const c_char, value: *const c_char) -> bool {
    set_config(t as u64, fx, key, value)
}
unsafe extern "C" fn take_fx_set_named_config_parm(t: *mut MediaItem_Take, fx: c_int, key: *const c_char, value: *const c_char) -> bool {
    set_config(t as u64, fx, key, value)
}

fn param_from_ident(owner: u64, index: c_int, ident: *const c_char) -> c_int {
    let ident = name_of(ident);
    counted(|s| {
        let Some(fx) = fx_of(s, owner, index) else { return -1 };
        let offset = macros(fx).len();
        let named = |name: &str| fx.params.iter().position(|p| p.name.eq_ignore_ascii_case(name)).map_or(-1, |n| (n + offset) as c_int);
        match ident.as_str() {
            ":wet" => named("Wet"),
            ":delta" => named("Delta"),
            ":bypass" => named("Bypass"),
            other => other.split(':').next().and_then(|n| n.parse().ok()).unwrap_or(-1),
        }
    })
}
unsafe extern "C" fn track_fx_get_param_from_ident(t: *mut MediaTrack, fx: c_int, ident: *const c_char) -> c_int {
    param_from_ident(t as u64, fx, ident)
}
unsafe extern "C" fn take_fx_get_param_from_ident(t: *mut MediaItem_Take, fx: c_int, ident: *const c_char) -> c_int {
    param_from_ident(t as u64, fx, ident)
}

// ----- presets -----

fn preset_index(owner: u64, index: c_int, count: *mut c_int) -> c_int {
    counted(|s| {
        let Some(fx) = fx_of(s, owner, index) else { return -1 };
        if !count.is_null() {
            unsafe { *count = fx.presets.len() as c_int };
        }
        fx.presets.iter().position(|p| p.name == fx.preset).map_or(-1, |n| n as c_int)
    })
}
unsafe extern "C" fn track_fx_get_preset_index(t: *mut MediaTrack, fx: c_int, count: *mut c_int) -> c_int {
    preset_index(t as u64, fx, count)
}
unsafe extern "C" fn take_fx_get_preset_index(t: *mut MediaItem_Take, fx: c_int, count: *mut c_int) -> c_int {
    preset_index(t as u64, fx, count)
}

/// Loads a preset by its place: its name shows and its values are set (a chunk plug-in's chunk follows).
fn load_preset(s: &mut State, owner: u64, index: c_int, at: usize) -> bool {
    let Some(fx) = fx_mut(s, owner, index) else { return false };
    let Some(preset) = fx.presets.get(at).cloned() else { return false };
    for (p, v) in fx.params.iter_mut().zip(&preset.values) {
        p.value = *v;
        p.shown = p.text_for(*v);
    }
    fx.preset = preset.name;
    s.undo_dirty = true;
    true
}

fn set_preset(owner: u64, index: c_int, name: *const c_char) -> bool {
    let name = name_of(name);
    counted(|s| {
        let at = fx_of(s, owner, index).and_then(|fx| fx.presets.iter().position(|p| p.name == name));
        at.is_some_and(|at| load_preset(s, owner, index, at))
    })
}
unsafe extern "C" fn track_fx_set_preset(t: *mut MediaTrack, fx: c_int, name: *const c_char) -> bool {
    set_preset(t as u64, fx, name)
}
unsafe extern "C" fn take_fx_set_preset(t: *mut MediaItem_Take, fx: c_int, name: *const c_char) -> bool {
    set_preset(t as u64, fx, name)
}

fn set_preset_by_index(owner: u64, index: c_int, at: c_int) -> bool {
    counted(|s| {
        if at == -2 {
            // The plug-in's factory state: every parameter at 0.5 in the fake, and no name.
            let Some(fx) = fx_mut(s, owner, index) else { return false };
            for p in fx.params.iter_mut().filter(|p| !p.wobbles) {
                p.value = 0.5;
                p.shown = p.text_for(0.5);
            }
            fx.preset.clear();
            s.undo_dirty = true;
            return true;
        }
        at >= 0 && load_preset(s, owner, index, at as usize)
    })
}
unsafe extern "C" fn track_fx_set_preset_by_index(t: *mut MediaTrack, fx: c_int, at: c_int) -> bool {
    set_preset_by_index(t as u64, fx, at)
}
unsafe extern "C" fn take_fx_set_preset_by_index(t: *mut MediaItem_Take, fx: c_int, at: c_int) -> bool {
    set_preset_by_index(t as u64, fx, at)
}

fn navigate(owner: u64, index: c_int, step: c_int) -> bool {
    counted(|s| {
        let Some(fx) = fx_of(s, owner, index) else { return false };
        let count = fx.presets.len() as i64;
        if count == 0 {
            return false;
        }
        let now = fx.presets.iter().position(|p| p.name == fx.preset).map_or(-1, |n| n as i64);
        let next = (now + step as i64).rem_euclid(count);
        load_preset(s, owner, index, next as usize)
    })
}
unsafe extern "C" fn track_fx_navigate_presets(t: *mut MediaTrack, fx: c_int, step: c_int) -> bool {
    navigate(t as u64, fx, step)
}
unsafe extern "C" fn take_fx_navigate_presets(t: *mut MediaItem_Take, fx: c_int, step: c_int) -> bool {
    navigate(t as u64, fx, step)
}

unsafe extern "C" fn track_fx_get_eq_param(
    track: *mut MediaTrack,
    fx: c_int,
    param: c_int,
    band_type: *mut c_int,
    band: *mut c_int,
    kind: *mut c_int,
    normalized: *mut f64,
) -> bool {
    counted(|s| {
        let Some(Param::Own(p)) = param_of(s, track as u64, fx, param) else { return false };
        let Some((t, b, k)) = p.eq else { return false };
        unsafe {
            *band_type = t;
            *band = b;
            *kind = k;
            *normalized = p.value;
        }
        true
    })
}

// ----- pins of any FX -----

fn pin(owner: u64, index: c_int, output: c_int, pin: c_int, high: *mut c_int) -> c_int {
    let mask = counted(|s| fx_of(s, owner, index).map(|f| f.pins.get(&(output != 0, pin)).copied().unwrap_or(1 << pin)).unwrap_or(0));
    if !high.is_null() {
        unsafe { *high = (mask >> 32) as u32 as c_int };
    }
    mask as u32 as c_int
}
unsafe extern "C" fn track_fx_get_pin_mappings(t: *mut MediaTrack, fx: c_int, output: c_int, p: c_int, high: *mut c_int) -> c_int {
    pin(t as u64, fx, output, p, high)
}
unsafe extern "C" fn take_fx_get_pin_mappings(t: *mut MediaItem_Take, fx: c_int, output: c_int, p: c_int, high: *mut c_int) -> c_int {
    pin(t as u64, fx, output, p, high)
}
fn set_pin(owner: u64, index: c_int, output: c_int, pin: c_int, low: c_int, hi: c_int) -> bool {
    counted(|s| {
        let Some(f) = fx_mut(s, owner, index) else { return false };
        f.pins.insert((output != 0, pin), (low as u32 as u64) | ((hi as u32 as u64) << 32));
        // REAPER doesn't notice a pin mapping by itself (lab 7.82): its state holds it, so a nudge makes the point.
        s.state_changed = true;
        true
    })
}
unsafe extern "C" fn track_fx_set_pin_mappings(t: *mut MediaTrack, fx: c_int, output: c_int, p: c_int, low: c_int, hi: c_int) -> bool {
    set_pin(t as u64, fx, output, p, low, hi)
}
unsafe extern "C" fn take_fx_set_pin_mappings(t: *mut MediaItem_Take, fx: c_int, output: c_int, p: c_int, low: c_int, hi: c_int) -> bool {
    set_pin(t as u64, fx, output, p, low, hi)
}
unsafe extern "C" fn take_fx_get_io_size(take: *mut MediaItem_Take, fx: c_int, ins: *mut c_int, outs: *mut c_int) -> c_int {
    counted(|s| match fx_of(s, take as u64, fx) {
        Some(f) => {
            unsafe {
                *ins = f.io.0;
                *outs = f.io.1;
            }
            0
        }
        None => -1,
    })
}

// ----- envelopes -----

/// An envelope's points: its own, or (a test's envelope made with a count) that many made up on first use.
pub fn points_of(e: &FakeEnvelope) -> Vec<FakePoint> {
    e.pts.clone().unwrap_or_else(|| (0..e.points).map(|i| FakePoint::at(i as f64, 0.5)).collect())
}

fn points_mut(e: &mut FakeEnvelope) -> &mut Vec<FakePoint> {
    if e.pts.is_none() {
        e.pts = Some(points_of(e));
    }
    e.pts.as_mut().expect("points")
}

/// Whether REAPER lists an envelope among its owner's: one REAPER hands out before it is shown (hidden, off, no points)
/// isn't (lab 7.82).
fn listed(e: &FakeEnvelope) -> bool {
    e.active || e.visible || !points_of(e).is_empty()
}

fn envelope_mut(s: &mut State, handle: u64) -> Option<&mut FakeEnvelope> {
    let (kind, a, b, c) = unpack(handle);
    match kind {
        ENVELOPE => {
            let track = if a == MASTER { &mut s.master } else { s.tracks.get_mut(a as usize)? };
            track.envelopes.get_mut(b as usize)
        }
        TAKE_ENVELOPE => s
            .tracks
            .get_mut(a as usize)?
            .items
            .get_mut(b as usize)?
            .takes
            .get_mut((c >> 10) as usize)?
            .envelopes
            .get_mut((c & 1023) as usize),
        _ => None,
    }
}

fn track_envelope_handle(track: u64, position: usize) -> *mut TrackEnvelope {
    pack(ENVELOPE, unpack(track).1, position as u64, 0) as *mut TrackEnvelope
}

unsafe extern "C" fn count_track_envelopes(track: *mut MediaTrack) -> c_int {
    counted(|s| track_of(s, track as u64).map_or(0, |t| t.envelopes.iter().filter(|e| listed(e)).count() as c_int))
}

unsafe extern "C" fn get_track_envelope(track: *mut MediaTrack, index: c_int) -> *mut TrackEnvelope {
    counted(|s| {
        let Some(t) = track_of(s, track as u64) else { return ptr::null_mut() };
        let position = t.envelopes.iter().enumerate().filter(|(_, e)| listed(e)).nth(index.max(0) as usize).map(|(n, _)| n);
        position.filter(|_| index >= 0).map_or(ptr::null_mut(), |n| track_envelope_handle(track as u64, n))
    })
}

/// A track's built-in envelopes by chunk name, and the names REAPER shows for them.
const BUILT_IN: [(&str, &str); 8] = [
    ("<VOLENV2", "Volume"),
    ("<PANENV2", "Pan"),
    ("<WIDTHENV2", "Width"),
    ("<VOLENV", "Volume (Pre-FX)"),
    ("<PANENV", "Pan (Pre-FX)"),
    ("<WIDTHENV", "Width (Pre-FX)"),
    ("<VOLENV3", "Trim Volume"),
    ("<MUTEENV", "Mute"),
];

unsafe extern "C" fn get_track_envelope_by_chunk_name(track: *mut MediaTrack, name: *const c_char) -> *mut TrackEnvelope {
    let name = name_of(name);
    counted(|s| {
        let Some(t) = track_mut(s, track as u64) else { return ptr::null_mut() };
        if name.starts_with('{') {
            // (only one in the track's list: REAPER finds no hidden, empty envelope by its id, lab 7.82)
            let found = t.envelopes.iter().position(|e| e.guid.eq_ignore_ascii_case(&name) && listed(e));
            return found.map_or(ptr::null_mut(), |n| track_envelope_handle(track as u64, n));
        }
        if let Some(n) = t.envelopes.iter().position(|e| e.chunk.as_deref() == Some(name.as_str())) {
            return track_envelope_handle(track as u64, n);
        }
        // REAPER hands out a track's built-in envelope before it is shown: hidden, off and without points.
        let Some((_, shown)) = BUILT_IN.iter().find(|(chunk, _)| *chunk == name) else { return ptr::null_mut() };
        let mut envelope = FakeEnvelope::own(shown, &name, 0);
        envelope.guid = format!("{{ENV-{}-{}}}", t.guid.trim_matches(|c| c == '{' || c == '}'), name.trim_start_matches('<'));
        envelope.active = false;
        envelope.visible = false;
        envelope.pts = Some(vec![]);
        t.envelopes.push(envelope);
        track_envelope_handle(track as u64, t.envelopes.len() - 1)
    })
}

unsafe extern "C" fn get_track_envelope_by_name(track: *mut MediaTrack, name: *const c_char) -> *mut TrackEnvelope {
    let name = name_of(name);
    counted(|s| {
        let found = track_of(s, track as u64).and_then(|t| t.envelopes.iter().position(|e| listed(e) && e.name == name));
        found.map_or(ptr::null_mut(), |n| track_envelope_handle(track as u64, n))
    })
}

unsafe extern "C" fn get_take_envelope_by_name(take: *mut MediaItem_Take, name: *const c_char) -> *mut TrackEnvelope {
    let name = name_of(name);
    counted(|s| {
        let Some(t) = take_of(s, take as u64) else { return ptr::null_mut() };
        let (_, a, b, c) = unpack(take as u64);
        t.envelopes
            .iter()
            .position(|e| e.name == name)
            .map_or(ptr::null_mut(), |n| pack(TAKE_ENVELOPE, a, b, c << 10 | n as u64) as *mut TrackEnvelope)
    })
}

fn fx_envelope(owner: u64, index: c_int, param: c_int, create: bool) -> *mut TrackEnvelope {
    counted(|s| {
        let Some(fx) = fx_of(s, owner, index) else { return ptr::null_mut() };
        let guid = fx.guid.clone();
        let name = format!("{} / {}", fx.params.get(param as usize).map_or("Param", |p| p.name.as_str()), fx.name);
        let value = fx.params.get(param as usize).map_or(0.0, |p| p.value);
        let (kind, a, b, c) = unpack(owner);
        let Some(list) = envelopes_mut(s, owner) else { return ptr::null_mut() };
        let found = list.iter().position(|e| e.fx_param.as_ref() == Some(&(guid.clone(), param)));
        let position = match (found, create) {
            (Some(n), _) => n,
            (None, true) => {
                // REAPER makes it with a point at the parameter's value, and keeps no undo point of it by itself.
                let mut envelope = FakeEnvelope::for_param(&name, &guid, param, 0);
                envelope.pts = Some(vec![FakePoint::at(0.0, value)]);
                envelope.armed = true;
                list.push(envelope);
                list.len() - 1
            }
            (None, false) => return ptr::null_mut(),
        };
        match kind {
            TRACK => pack(ENVELOPE, a, position as u64, 0) as *mut TrackEnvelope,
            _ => pack(TAKE_ENVELOPE, a, b, c << 10 | position as u64) as *mut TrackEnvelope,
        }
    })
}

unsafe extern "C" fn get_fx_envelope(track: *mut MediaTrack, fx: c_int, param: c_int, create: bool) -> *mut TrackEnvelope {
    fx_envelope(track as u64, fx, param, create)
}

unsafe extern "C" fn take_fx_get_envelope(take: *mut MediaItem_Take, fx: c_int, param: c_int, create: bool) -> *mut TrackEnvelope {
    fx_envelope(take as u64, fx, param, create)
}

/// The FX with this id in a chain, as the path that leads to it.
fn path_of(chain: &[FakeFx], guid: &str) -> Option<Vec<usize>> {
    chain.iter().enumerate().find_map(|(n, fx)| {
        if fx.guid == guid {
            return Some(vec![n]);
        }
        let mut path = path_of(fx.children.as_deref()?, guid)?;
        path.insert(0, n);
        Some(path)
    })
}

unsafe extern "C" fn envelope_get_parent_track(envelope: *mut TrackEnvelope, fx_out: *mut c_int, param_out: *mut c_int) -> *mut MediaTrack {
    counted(|s| {
        let (kind, a, _, _) = unpack(envelope as u64);
        let Some(e) = envelope_of(s, envelope as u64).filter(|_| kind == ENVELOPE) else { return ptr::null_mut() };
        let track = pack(TRACK, a, 0, 0);
        let fx_param = e.fx_param.as_ref().and_then(|(guid, param)| {
            let t = track_of(s, track)?;
            match path_of(&t.fx, guid) {
                Some(path) => Some((fx_address(s, track, &path, false), *param)),
                None => path_of(&t.input_fx, guid).map(|path| (fx_address(s, track, &path, true), *param)),
            }
        });
        unsafe {
            *fx_out = fx_param.map_or(-1, |(fx, _)| fx);
            *param_out = fx_param.map_or(-1, |(_, param)| param);
        }
        track as *mut MediaTrack
    })
}

/// A lane's points (the envelope's own for -1, else an automation item's pool, at the item's place).
fn lane_points(s: &State, handle: u64, item: c_int) -> Option<Vec<FakePoint>> {
    let e = envelope_of(s, handle)?;
    if item < 0 {
        return Some(points_of(e));
    }
    let it = e.items.get((item & 0xFFFF) as usize)?;
    Some(s.pools.get(&it.pool).cloned().unwrap_or_default().into_iter().map(|p| FakePoint { time: p.time + it.position, ..p }).collect())
}

/// Writes a lane's points back (an item's into its pool, from the item's start).
fn set_lane(s: &mut State, handle: u64, item: c_int, points: Vec<FakePoint>) -> bool {
    if item < 0 {
        let Some(e) = envelope_mut(s, handle) else { return false };
        e.pts = Some(points);
        return true;
    }
    let Some(it) = envelope_of(s, handle).and_then(|e| e.items.get((item & 0xFFFF) as usize)).cloned() else { return false };
    s.pools.insert(it.pool, points.into_iter().map(|p| FakePoint { time: p.time - it.position, ..p }).collect());
    true
}

fn sorted(mut points: Vec<FakePoint>) -> Vec<FakePoint> {
    points.sort_by(|a, b| a.time.total_cmp(&b.time));
    points
}

unsafe extern "C" fn count_envelope_points(envelope: *mut TrackEnvelope) -> c_int {
    counted(|s| envelope_of(s, envelope as u64).map_or(0, |e| points_of(e).len() as c_int))
}

unsafe extern "C" fn count_envelope_points_ex(envelope: *mut TrackEnvelope, item: c_int) -> c_int {
    counted(|s| lane_point(s, envelope as u64, item, None).map_or(0, |(count, _)| count as c_int))
}

/// A lane's point count, and one of its points, without copying the lane (a read pages through big lanes).
fn lane_point(s: &State, handle: u64, item: c_int, index: Option<usize>) -> Option<(usize, Option<FakePoint>)> {
    let e = envelope_of(s, handle)?;
    if item < 0 {
        return Some(match &e.pts {
            Some(points) => (points.len(), index.and_then(|i| points.get(i).cloned())),
            None => {
                let count = e.points.max(0) as usize;
                (count, index.filter(|i| *i < count).map(|i| FakePoint::at(i as f64, 0.5)))
            }
        });
    }
    let it = e.items.get((item & 0xFFFF) as usize)?;
    let pool = s.pools.get(&it.pool).map_or(&[][..], Vec::as_slice);
    Some((pool.len(), index.and_then(|i| pool.get(i)).map(|p| FakePoint { time: p.time + it.position, ..*p })))
}

unsafe extern "C" fn get_envelope_point_ex(
    envelope: *mut TrackEnvelope,
    item: c_int,
    index: c_int,
    time: *mut f64,
    value: *mut f64,
    shape: *mut c_int,
    tension: *mut f64,
    selected: *mut bool,
) -> bool {
    counted(|s| {
        let Some(point) = lane_point(s, envelope as u64, item, usize::try_from(index).ok()).and_then(|(_, point)| point) else {
            return false;
        };
        unsafe {
            *time = point.time;
            *value = point.value;
            *shape = point.shape;
            *tension = point.tension;
            *selected = point.selected;
        }
        true
    })
}

unsafe extern "C" fn insert_envelope_point_ex(
    envelope: *mut TrackEnvelope,
    item: c_int,
    time: f64,
    value: f64,
    shape: c_int,
    tension: f64,
    selected: bool,
    no_sort: *mut bool,
) -> bool {
    let no_sort = !no_sort.is_null() && unsafe { *no_sort };
    counted(|s| {
        let Some(mut points) = lane_points(s, envelope as u64, item) else { return false };
        points.push(FakePoint { time, value, shape, tension, selected });
        let points = if no_sort { points } else { sorted(points) };
        s.undo_dirty = true;
        set_lane(s, envelope as u64, item, points)
    })
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn set_envelope_point_ex(
    envelope: *mut TrackEnvelope,
    item: c_int,
    index: c_int,
    time: *mut f64,
    value: *mut f64,
    shape: *mut c_int,
    tension: *mut f64,
    selected: *mut bool,
    no_sort: *mut bool,
) -> bool {
    let read = |p: *mut f64| (!p.is_null()).then(|| unsafe { *p });
    let (time, value, tension) = (read(time), read(value), read(tension));
    let shape = (!shape.is_null()).then(|| unsafe { *shape });
    let selected = (!selected.is_null()).then(|| unsafe { *selected });
    let no_sort = !no_sort.is_null() && unsafe { *no_sort };
    counted(|s| {
        let Some(mut points) = lane_points(s, envelope as u64, item) else { return false };
        let Some(point) = points.get_mut(index.max(0) as usize).filter(|_| index >= 0) else { return false };
        if let Some(t) = time {
            point.time = t;
        }
        if let Some(v) = value {
            point.value = v;
        }
        if let Some(sh) = shape {
            point.shape = sh;
        }
        if let Some(t) = tension {
            point.tension = t;
        }
        if let Some(sel) = selected {
            point.selected = sel;
        }
        let points = if no_sort { points } else { sorted(points) };
        s.undo_dirty = true;
        set_lane(s, envelope as u64, item, points)
    })
}

unsafe extern "C" fn delete_envelope_point_ex(envelope: *mut TrackEnvelope, item: c_int, index: c_int) -> bool {
    counted(|s| {
        let Some(mut points) = lane_points(s, envelope as u64, item) else { return false };
        if index < 0 || index as usize >= points.len() {
            return false;
        }
        points.remove(index as usize);
        s.undo_dirty = true;
        set_lane(s, envelope as u64, item, points)
    })
}

unsafe extern "C" fn delete_envelope_point_range_ex(envelope: *mut TrackEnvelope, item: c_int, start: f64, end: f64) -> bool {
    counted(|s| {
        let Some(points) = lane_points(s, envelope as u64, item) else { return false };
        let kept: Vec<FakePoint> = points.into_iter().filter(|p| p.time < start || p.time >= end).collect();
        s.undo_dirty = true;
        set_lane(s, envelope as u64, item, kept)
    })
}

unsafe extern "C" fn envelope_sort_points_ex(envelope: *mut TrackEnvelope, item: c_int) -> bool {
    counted(|s| {
        let Some(points) = lane_points(s, envelope as u64, item) else { return false };
        // Sorting is an envelope edit REAPER keeps a point of (lab).
        s.undo_dirty = true;
        set_lane(s, envelope as u64, item, sorted(points))
    })
}

fn is_volume(kind: &str) -> bool {
    matches!(kind, "VOLENV" | "VOLENV2" | "VOLENV3" | "AUXVOLENV")
}

unsafe extern "C" fn get_envelope_scaling_mode(envelope: *mut TrackEnvelope) -> c_int {
    counted(|s| envelope_of(s, envelope as u64).map_or(0, |e| is_volume(&e.kind) as c_int))
}

/// The fake's fader scaling: any curve that goes both ways does (REAPER's maps 1.0 to 716.2).
const FADER: f64 = 716.217_850_312_61;

unsafe extern "C" fn scale_to_envelope_mode(mode: c_int, value: f64) -> f64 {
    if mode == 1 {
        value.max(0.0).powf(0.25) * FADER
    } else {
        value
    }
}

unsafe extern "C" fn scale_from_envelope_mode(mode: c_int, value: f64) -> f64 {
    if mode == 1 {
        (value.max(0.0) / FADER).powi(4)
    } else {
        value
    }
}

fn format_value(e: &FakeEnvelope, value: f64) -> String {
    match e.kind.as_str() {
        kind if is_volume(kind) => {
            let gain = unsafe { scale_from_envelope_mode(1, value) };
            if gain <= 0.0 {
                "-inf dB".into()
            } else {
                format!("{:.2}dB", 20.0 * gain.log10())
            }
        }
        "PANENV" | "PANENV2" | "AUXPANENV" => match value {
            v if v.abs() < 1e-9 => "center".into(),
            v if v > 0.0 => format!("{}%L", (v * 100.0).round()),
            v => format!("{}%R", (-v * 100.0).round()),
        },
        "WIDTHENV" | "WIDTHENV2" => format!("{:.1}%", value * 100.0),
        "MUTEENV" | "AUXMUTEENV" => if value >= 0.5 { "UNMUTE" } else { "MUTE" }.into(),
        "PITCHENV" => format!("{:+.4} semitones", value),
        _ => format!("{value:.3}"),
    }
}

unsafe extern "C" fn envelope_format_value(envelope: *mut TrackEnvelope, value: f64, buf: *mut c_char, size: c_int) {
    counted(|s| {
        if let Some(e) = envelope_of(s, envelope as u64) {
            write(buf, size, &format_value(e, value));
        }
    })
}

fn chunk_text(s: &State, e: &FakeEnvelope) -> String {
    let head = if e.kind == "PARMENV" {
        format!("<PARMENV {} {} {} 0", e.fx_param.as_ref().map_or(0, |(_, p)| *p), e.range.0, e.range.1)
    } else {
        format!("<{}", e.kind)
    };
    let mut text = format!(
        "{head}\nEGUID {}\nACT {} -1\nVIS {} 1 1\nLANEHEIGHT 0 0\nARM {}\nDEFSHAPE 0 -1 -1\n",
        e.guid, e.active as i32, e.visible as i32, e.armed as i32
    );
    for it in &e.items {
        text += &format!(
            "POOLEDENVINST {} {} {} {} {} {} {} {} {} 0 0 0 {} 0 0 0\n",
            it.pool,
            it.position,
            it.length,
            it.offset,
            it.rate,
            it.selected as i32,
            it.baseline,
            it.amplitude,
            it.looped as i32,
            it.mute as i32
        );
    }
    let _ = s;
    for p in points_of(e) {
        text += &format!("PT {} {} {} {} {}\n", p.time, p.value, p.shape, p.selected as i32, p.tension);
    }
    text + ">"
}

unsafe extern "C" fn get_envelope_state_chunk(envelope: *mut TrackEnvelope, buf: *mut c_char, size: c_int, _isundo: bool) -> bool {
    counted(|s| match envelope_of(s, envelope as u64) {
        Some(e) => {
            let text = chunk_text(s, e);
            write(buf, size, &text);
            true
        }
        None => false,
    })
}

/// An envelope's chunk written (REAPER keeps no undo point of it by itself). A pool left with no instance anywhere is
/// emptied, as REAPER empties it.
unsafe extern "C" fn set_envelope_state_chunk(envelope: *mut TrackEnvelope, chunk: *const c_char, _isundo: bool) -> bool {
    let chunk = name_of(chunk);
    counted(|s| {
        let Some(e) = envelope_mut(s, envelope as u64) else { return false };
        let (mut points, mut items) = (Vec::new(), Vec::new());
        for line in chunk.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            let number = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            match f.first().copied() {
                Some("ACT") => e.active = f.get(1) == Some(&"1"),
                Some("VIS") => e.visible = f.get(1) == Some(&"1"),
                Some("ARM") => e.armed = f.get(1) == Some(&"1"),
                Some("PT") => points.push(FakePoint {
                    time: number(1),
                    value: number(2),
                    shape: number(3) as i32,
                    selected: number(4) != 0.0,
                    tension: number(5),
                }),
                Some("POOLEDENVINST") => items.push(FakeAutoItem {
                    pool: number(1) as i64,
                    position: number(2),
                    length: number(3),
                    offset: number(4),
                    rate: number(5),
                    selected: number(6) != 0.0,
                    baseline: number(7),
                    amplitude: number(8),
                    looped: number(9) != 0.0,
                    mute: number(13) != 0.0,
                }),
                _ => {}
            }
        }
        e.pts = Some(points);
        e.items = items;
        // An emptied take envelope leaves its take's list (lab 7.82: CountTakeEnvelopes drops).
        let emptied = !listed(e);
        if let (TAKE_ENVELOPE, a, b, c) = unpack(envelope as u64) {
            if emptied {
                if let Some(take) =
                    s.tracks.get_mut(a as usize).and_then(|t| t.items.get_mut(b as usize)).and_then(|i| i.takes.get_mut((c >> 10) as usize))
                {
                    take.envelopes.remove((c & 1023) as usize);
                }
            }
        }
        let used: Vec<i64> = s
            .tracks
            .iter()
            .chain(std::iter::once(&s.master))
            .flat_map(|t| t.envelopes.iter())
            .flat_map(|e| e.items.iter().map(|i| i.pool))
            .collect();
        for (pool, points) in s.pools.iter_mut() {
            if !used.contains(pool) {
                points.clear();
            }
        }
        true
    })
}

unsafe extern "C" fn get_set_envelope_info_string(envelope: *mut TrackEnvelope, parm: *const c_char, buf: *mut c_char, set: bool) -> bool {
    let parm = name_of(parm);
    if set {
        let on = name_of(buf).trim() == "1";
        return counted(|s| {
            let volume = envelope_of(s, envelope as u64).is_some_and(|e| is_volume(&e.kind));
            let Some(e) = envelope_mut(s, envelope as u64) else { return false };
            match parm.as_str() {
                "ACTIVE" => {
                    // An envelope shown for the first time gets a point (REAPER's: the fader's value, 0 for others).
                    if on && !e.active && points_of(e).is_empty() {
                        e.pts = Some(vec![FakePoint::at(0.0, if volume { FADER } else { 0.0 })]);
                        e.armed = true;
                    }
                    e.active = on;
                }
                "VISIBLE" => e.visible = on,
                "ARM" => e.armed = on,
                _ => return false,
            }
            s.undo_dirty = true;
            true
        });
    }
    counted(|s| {
        let Some(e) = envelope_of(s, envelope as u64) else { return false };
        let flag = |on: bool| if on { "1" } else { "0" };
        match parm.as_str() {
            "ACTIVE" => write(buf, 4096, flag(e.active)),
            "ARM" => write(buf, 4096, flag(e.armed)),
            "VISIBLE" => write(buf, 4096, flag(e.visible)),
            "GUID" => write(buf, 4096, &e.guid),
            _ => return false,
        }
        true
    })
}

unsafe extern "C" fn get_envelope_info_value(envelope: *mut TrackEnvelope, parm: *const c_char) -> f64 {
    let parm = name_of(parm);
    counted(|s| {
        let (kind, a, b, c) = unpack(envelope as u64);
        match (kind, parm.as_str()) {
            (TAKE_ENVELOPE, "P_TAKE") if envelope_of(s, envelope as u64).is_some() => {
                take_ptr(a as usize, b as usize, (c >> 10) as usize) as u64 as f64
            }
            (ENVELOPE, "P_TRACK") => pack(TRACK, a, 0, 0) as f64,
            _ => 0.0,
        }
    })
}

// ----- automation items -----

unsafe extern "C" fn count_automation_items(envelope: *mut TrackEnvelope) -> c_int {
    counted(|s| envelope_of(s, envelope as u64).map_or(0, |e| e.items.len() as c_int))
}

unsafe extern "C" fn get_set_automation_item_info(
    envelope: *mut TrackEnvelope,
    item: c_int,
    key: *const c_char,
    value: f64,
    set: bool,
) -> f64 {
    let key = name_of(key);
    counted(|s| {
        let Some(e) = envelope_mut(s, envelope as u64) else { return 0.0 };
        let Some(it) = e.items.get_mut(item.max(0) as usize).filter(|_| item >= 0) else { return 0.0 };
        if set {
            match key.as_str() {
                "D_POSITION" => it.position = value,
                "D_LENGTH" => it.length = value,
                "D_STARTOFFS" => it.offset = value,
                "D_PLAYRATE" => it.rate = value,
                "D_BASELINE" => it.baseline = value,
                "D_AMPLITUDE" => it.amplitude = value,
                "D_LOOPSRC" => it.looped = value != 0.0,
                "D_MUTE" => it.mute = value != 0.0,
                "D_UISEL" => it.selected = value != 0.0,
                _ => return 0.0,
            }
            s.undo_dirty = true;
            return value;
        }
        match key.as_str() {
            "D_POOL_ID" => it.pool as f64,
            "D_POSITION" => it.position,
            "D_LENGTH" => it.length,
            "D_STARTOFFS" => it.offset,
            "D_PLAYRATE" => it.rate,
            "D_BASELINE" => it.baseline,
            "D_AMPLITUDE" => it.amplitude,
            "D_LOOPSRC" => it.looped as i32 as f64,
            "D_MUTE" => it.mute as i32 as f64,
            "D_UISEL" => it.selected as i32 as f64,
            _ => 0.0,
        }
    })
}

unsafe extern "C" fn get_set_automation_item_info_string(
    envelope: *mut TrackEnvelope,
    item: c_int,
    key: *const c_char,
    buf: *mut c_char,
    set: bool,
) -> bool {
    let key = name_of(key);
    if key != "P_POOL_NAME" {
        return false;
    }
    let value = set.then(|| name_of(buf));
    counted(|s| {
        let Some(pool) = envelope_of(s, envelope as u64).and_then(|e| e.items.get(item.max(0) as usize)).map(|it| it.pool) else {
            return false;
        };
        match value {
            Some(name) => {
                s.pool_names.insert(pool, name);
            }
            None => write(buf, 4096, s.pool_names.get(&pool).map_or("", String::as_str)),
        }
        true
    })
}

/// A new automation item: of a new pool (it takes the envelope's points in its range) or another instance of a pool. Its
/// index (the items are kept in order of where they start).
unsafe extern "C" fn insert_automation_item(envelope: *mut TrackEnvelope, pool: c_int, position: f64, length: f64) -> c_int {
    counted(|s| {
        if envelope_of(s, envelope as u64).is_none() {
            return -1;
        }
        let pool = if pool < 0 {
            s.pool_made += 1;
            let id = s.pool_made;
            let e = envelope_mut(s, envelope as u64).expect("the envelope");
            let points = points_of(e);
            let (inside, outside): (Vec<FakePoint>, Vec<FakePoint>) =
                points.into_iter().partition(|p| p.time >= position && p.time < position + length);
            e.pts = Some(outside);
            s.pools.insert(id, inside.into_iter().map(|p| FakePoint { time: p.time - position, ..p }).collect());
            id
        } else {
            s.pools.entry(pool as i64).or_default();
            pool as i64
        };
        let e = envelope_mut(s, envelope as u64).expect("the envelope");
        let item = FakeAutoItem {
            pool,
            position,
            length,
            offset: 0.0,
            rate: 1.0,
            baseline: 0.5,
            amplitude: 1.0,
            looped: true,
            mute: false,
            selected: true,
        };
        let at = e.items.iter().position(|it| it.position > position).unwrap_or(e.items.len());
        e.items.insert(at, item);
        s.undo_dirty = true;
        at as c_int
    })
}

unsafe extern "C" fn get_global_automation_override() -> c_int {
    with(|s| s.automation_override)
}

unsafe extern "C" fn set_global_automation_override(mode: c_int) {
    // REAPER's own setting: no undo point, no change to the state count (lab).
    with(|s| s.automation_override = mode);
}

// ----- take envelopes made by REAPER's actions, and the item selection they use -----

/// The take envelope actions: (command, the envelope's name, its chunk name).
const TAKE_ACTIONS: [(c_int, &str, &str); 4] =
    [(40693, "Volume", "VOLENV"), (40694, "Pan", "PANENV"), (40695, "Mute", "MUTEENV"), (41612, "Pitch", "PITCHENV")];

pub(super) unsafe extern "C" fn main_on_command_ex(command: c_int, flag: c_int, proj: *mut ReaProject) {
    unsafe { super::main_on_command_ex(command, flag, proj) };
    let Some((_, name, kind)) = TAKE_ACTIONS.iter().find(|(c, _, _)| *c == command) else { return };
    with(|s| {
        for track in s.tracks.iter_mut() {
            for item in track.items.iter_mut().filter(|i| i.selected) {
                let active = item.active;
                let Some(take) = item.takes.get_mut(active) else { continue };
                match take.envelopes.iter_mut().find(|e| e.name == *name) {
                    // On a take that has it, the action shows or hides it.
                    Some(envelope) => envelope.visible = !envelope.visible,
                    None => {
                        s.made += 1;
                        let mut envelope = FakeEnvelope::own(name, &format!("<{kind}"), 0);
                        envelope.chunk = None;
                        envelope.guid = format!("{{ENV-TAKE-{}}}", s.made);
                        envelope.pts = Some(vec![FakePoint::at(0.0, if *kind == "VOLENV" { FADER } else { 0.0 })]);
                        take.envelopes.push(envelope);
                    }
                }
                s.undo_dirty = true;
            }
        }
    });
}
