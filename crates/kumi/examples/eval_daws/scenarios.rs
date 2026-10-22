//! The scenarios: what the producer says, the fixture built in the lab DAW (Lua through Kumi's host), what each reads of
//! the state, and its checks (the research's "Expect"). Not specs: each checks only what the scenario is about.
use daw_bridge::Daw;
use serde_json::Value;

pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

impl Check {
    pub fn new(name: &str, passed: bool, detail: impl Into<String>) -> Check {
        Check { name: name.to_owned(), passed, detail: detail.into() }
    }
}

/// What a scenario's checks look at.
pub struct Seen<'a> {
    pub before: &'a Value,
    pub after: &'a Value,
    pub said: &'a str,
    pub tools: &'a [String],
    /// How many changes Kumi made (HISTORY rows).
    pub changes: usize,
}

pub struct Scenario {
    pub id: &'static str,
    pub daw: Daw,
    /// What the producer says, turn by turn.
    pub says: &'static [&'static str],
    /// Lua run first; then the host keeps the project's snapshot, as when Kumi last looked (catch-up's baseline).
    pub before: Option<&'static str>,
    /// Lua the fixture is built with (after `before`), in the DAW: REAPER's `reaper`, Renoise's `song`.
    pub fixture: &'static str,
    /// Lua that adds the scenario's own fields to the state (`out`), read before and after.
    pub read: &'static str,
    /// Lua run after the scoring (preferences the fixture changed put back).
    pub teardown: Option<&'static str>,
    /// Whether the producer asks for notes to be written (else none may be added).
    pub generates: bool,
    /// Words Kumi's answer should use, saying what it found or did (any one of them).
    pub names: &'static [&'static str],
    pub expect: fn(&Seen) -> Vec<Check>,
}

// ---------------------------------------------------------------------------------------------------------------------
// The preludes (each DAW's first Lua: the project or song put back to a plain start, and the fixtures' helpers) and the
// state readers (the fields every scenario's checks may use).

pub const REAPER_PRELUDE: &str = r#"
for i = reaper.CountTracks(0) - 1, 0, -1 do reaper.DeleteTrack(reaper.GetTrack(0, i)) end
local _, markers, regions = reaper.CountProjectMarkers(0)
for i = markers + regions - 1, 0, -1 do reaper.DeleteProjectMarkerByIndex(0, i) end
reaper.SetCurrentBPM(0, 120, false)
local function track(name, depth)
  local at = reaper.CountTracks(0)
  reaper.InsertTrackAtIndex(at, true)
  local t = reaper.GetTrack(0, at)
  reaper.GetSetMediaTrackInfo_String(t, "P_NAME", name, true)
  if depth then reaper.SetMediaTrackInfo_Value(t, "I_FOLDERDEPTH", depth) end
  return t
end
local function audio(t, file, pos, len)
  local it = reaper.AddMediaItemToTrack(t)
  local tk = reaper.AddTakeToMediaItem(it)
  reaper.SetMediaItemTake_Source(tk, reaper.PCM_Source_CreateFromFile(file))
  reaper.GetSetMediaItemTakeInfo_String(tk, "P_NAME", file:match("([^/]+)$"), true)
  reaper.SetMediaItemInfo_Value(it, "D_POSITION", pos)
  reaper.SetMediaItemInfo_Value(it, "D_LENGTH", len)
  reaper.SetMediaItemInfo_Value(it, "D_FADEINLEN", 0)
  reaper.SetMediaItemInfo_Value(it, "D_FADEOUTLEN", 0)
  return it
end
local function midi(t, pos, len, notes)
  local it = reaper.CreateNewMIDIItemInProj(t, pos, pos + len, false)
  local tk = reaper.GetActiveTake(it)
  for _, n in ipairs(notes or {}) do
    local s = reaper.MIDI_GetPPQPosFromProjTime(tk, pos + n[1])
    local e = reaper.MIDI_GetPPQPosFromProjTime(tk, pos + n[1] + n[2])
    reaper.MIDI_InsertNote(tk, false, false, s, e, 0, n[3], 100, true)
  end
  reaper.MIDI_Sort(tk)
  return it
end
"#;

pub const REAPER_STATE: &str = r#"
local out = {tracks = {}, notes = 0, regions = {}}
local function name_of(t) local _, n = reaper.GetSetMediaTrackInfo_String(t, "P_NAME", "", false) return n end
local function r6(v) return math.floor(v * 1e6 + 0.5) / 1e6 end
for i = 0, reaper.CountTracks(0) - 1 do
  local t = reaper.GetTrack(0, i)
  local parent = reaper.GetParentTrack(t)
  local sends = {}
  for s = 0, reaper.GetTrackNumSends(t, 0) - 1 do
    local dest = reaper.GetTrackSendInfo_Value(t, 0, s, "P_DESTTRACK")
    sends[#sends + 1] = {to = name_of(dest), mute = reaper.GetTrackSendInfo_Value(t, 0, s, "B_MUTE") ~= 0,
      volume = r6(reaper.GetTrackSendInfo_Value(t, 0, s, "D_VOL")), dstchan = reaper.GetTrackSendInfo_Value(t, 0, s, "I_DSTCHAN")}
  end
  local fx = {}
  for f = 0, reaper.TrackFX_GetCount(t) - 1 do
    local _, fname = reaper.TrackFX_GetFXName(t, f, "")
    local row = {name = fname, enabled = reaper.TrackFX_GetEnabled(t, f), offline = reaper.TrackFX_GetOffline(t, f)}
    if not row.offline then
      for p = 0, reaper.TrackFX_GetNumParams(t, f) - 1 do
        local _, pname = reaper.TrackFX_GetParamName(t, f, p, "")
        -- (ReaComp's detector input is SignIn: 0 the main input, anything else the sidechain; rendered in the 7.82 lab)
        if pname == "SignIn" then local _, v = reaper.TrackFX_GetFormattedParamValue(t, f, p, "") row.detector = v end
      end
    end
    fx[#fx + 1] = row
  end
  local items = {}
  for k = 0, reaper.CountTrackMediaItems(t) - 1 do
    local it = reaper.GetTrackMediaItem(t, k)
    local take = reaper.GetActiveTake(it)
    local row = {pos = r6(reaper.GetMediaItemInfo_Value(it, "D_POSITION")), len = r6(reaper.GetMediaItemInfo_Value(it, "D_LENGTH")),
      fadein = r6(reaper.GetMediaItemInfo_Value(it, "D_FADEINLEN")), fadeout = r6(reaper.GetMediaItemInfo_Value(it, "D_FADEOUTLEN")),
      loop = reaper.GetMediaItemInfo_Value(it, "B_LOOPSRC") ~= 0}
    if take then
      row.rate = r6(reaper.GetMediaItemTakeInfo_Value(take, "D_PLAYRATE"))
      row.pitch = r6(reaper.GetMediaItemTakeInfo_Value(take, "D_PITCH"))
      row.offset = r6(reaper.GetMediaItemTakeInfo_Value(take, "D_STARTOFFS"))
      if reaper.TakeIsMIDI(take) then local _, n = reaper.MIDI_CountEvts(take) out.notes = out.notes + n row.notes = n end
    end
    items[#items + 1] = row
  end
  out.tracks[#out.tracks + 1] = {name = name_of(t), parent = parent and name_of(parent) or nil,
    mainSend = reaper.GetMediaTrackInfo_Value(t, "B_MAINSEND") ~= 0, mute = reaper.GetMediaTrackInfo_Value(t, "B_MUTE") ~= 0,
    volume = r6(reaper.GetMediaTrackInfo_Value(t, "D_VOL")), nchan = reaper.GetMediaTrackInfo_Value(t, "I_NCHAN"),
    hidden = reaper.GetMediaTrackInfo_Value(t, "B_SHOWINTCP") == 0 and reaper.GetMediaTrackInfo_Value(t, "B_SHOWINMIXER") == 0,
    sends = sends, fx = fx, items = items}
end
local m = 0
while true do
  local ok, isrgn, pos, rgnend, name = reaper.EnumProjectMarkers(m)
  if ok == 0 then break end
  if isrgn then out.regions[#out.regions + 1] = {name = name, start = r6(pos), stop = r6(rgnend)} end
  m = m + 1
end
out.tempo = reaper.Master_GetTempo()
local _, targets = reaper.GetSetProjectInfo_String(0, "RENDER_TARGETS", "", false)
out.targets = 0
for _ in targets:gmatch("[^;]+") do out.targets = out.targets + 1 end
local prefs = {}
for _, name in ipairs({"deffadelen", "splitautoxfade", "loopnewitems", "deftrackvol", "itemtimelock", "tempoenvtimelock", "autoxfade"}) do
  local ok, value = reaper.get_config_var_string(name)
  prefs[name] = ok and value or ""
end
for _, key in ipairs({"RENDER_SETTINGS", "RENDER_BOUNDSFLAG", "RENDER_SRATE", "RENDER_CHANNELS", "RENDER_TAILFLAG"}) do
  prefs[key] = reaper.GetSetProjectInfo(0, key, 0, false)
end
for _, key in ipairs({"RENDER_FILE", "RENDER_PATTERN", "RENDER_FORMAT"}) do
  local _, value = reaper.GetSetProjectInfo_String(0, key, "", false)
  prefs[key] = value
end
out.prefs = prefs
"#;

pub const RENOISE_PRELUDE: &str = r#"
local R = renoise
local seq = song.sequencer
while #seq.pattern_sequence > 1 do seq:delete_sequence_at(#seq.pattern_sequence) end
local first = seq:pattern(1)
song:pattern(first):clear()
song:pattern(first).number_of_lines = 64
for i = #song.tracks, 1, -1 do
  if song:track(i).type == R.Track.TRACK_TYPE_SEND then song:delete_track_at(i) end
end
for i = song.sequencer_track_count, 1, -1 do
  if song:track(i).type == R.Track.TRACK_TYPE_GROUP then song:delete_track_at(i) end
end
while song.sequencer_track_count > 4 do song:delete_track_at(song.sequencer_track_count) end
while song.sequencer_track_count < 4 do song:insert_track_at(song.sequencer_track_count + 1) end
for t = 1, 4 do
  local tr = song:track(t)
  tr.name = string.format("Track %02d", t)
  while #tr.devices > 1 do tr:delete_device_at(#tr.devices) end
  tr.visible_note_columns = 1
  tr.prefx_volume.value = 1
  tr.postfx_volume.value = 1
end
while #song.instruments > 1 do song:delete_instrument_at(#song.instruments) end
song:instrument(1):clear()
song.transport.bpm = 120
song.transport.lpb = 4
local function sample_instrument(i, name, file)
  while #song.instruments < i do song:insert_instrument_at(#song.instruments + 1) end
  local ins = song:instrument(i)
  ins:clear()
  ins.name = name
  local s = ins:insert_sample_at(1)
  if not s.sample_buffer:load_from(file) then error("couldn't load " .. file) end
  s.name = name
  return ins
end
local function note(p, t, line, text, inst, col)
  local nc = song:pattern(p):track(t):line(line + 1):note_column(col or 1)
  nc.note_string = text
  nc.instrument_value = inst
end
"#;

pub const RENOISE_STATE: &str = r##"
local R = renoise
-- (the notes are counted by RENOISE_NOTES, in pages)
local out = {notes = 0, tracks = {}, instruments = {}, sequence = {}}
local types = {[R.Track.TRACK_TYPE_SEQUENCER] = "sequencer", [R.Track.TRACK_TYPE_GROUP] = "group",
  [R.Track.TRACK_TYPE_SEND] = "send", [R.Track.TRACK_TYPE_MASTER] = "master"}
for i, t in ipairs(song.tracks) do
  local devices = {}
  for _, dev in ipairs(t.devices) do
    local row = {name = dev.display_name, path = dev.device_path}
    if dev.device_path:find("#") or dev.device_path:find("Compressor") then
      local params = {}
      for _, prm in ipairs(dev.parameters) do params[prm.name] = prm.value_string end
      row.params = params
    end
    devices[#devices + 1] = row
  end
  local group = t.group_parent
  out.tracks[#out.tracks + 1] = {index = i, name = t.name, type = types[t.type], group = group and group.name or nil,
    pre = t.prefx_volume.value, post = t.postfx_volume.value, devices = devices}
end
for _, ins in ipairs(song.instruments) do
  local samples = {}
  for _, s in ipairs(ins.samples) do
    local b = s.sample_buffer
    samples[#samples + 1] = {name = s.name, frames = b.has_sample_data and b.number_of_frames or 0, rate = b.has_sample_data and b.sample_rate or 0}
  end
  out.instruments[#out.instruments + 1] = {name = ins.name, samples = samples}
end
for s, p in ipairs(song.sequencer.pattern_sequence) do out.sequence[s] = p end
local tr = song.transport
out.prefs = {editStep = tr.edit_step, octave = tr.octave, follow = tr.follow_player, editMode = tr.edit_mode,
  quantize = tr.record_quantize_enabled, quantizeLines = tr.record_quantize_lines, wrap = tr.wrapped_pattern_edit,
  singleTrack = tr.single_track_edit_mode, metronome = tr.metronome_enabled, lpb = tr.lpb, bpm = tr.bpm}
-- A pattern track's notes, as "line note" texts (through aliases, as Renoise plays them).
local function notes_of(p, t, c)
  local r = {}
  for l, line in ipairs(song:pattern(p):track(t).lines) do
    local nc = line:note_column(c or 1)
    if nc.note_value < 120 then r[#r + 1] = (l - 1) .. " " .. nc.note_string end
  end
  return r
end
"##;

/// The notes a Renoise song plays, counted from pattern `FROM` (set before it) for half a second at most, Renoise stopping a
/// callback at 800 ms (#400 review D21): `next` is the pattern to go on from, none once every pattern is counted. Each
/// sequencer track's lines are read one by one, an empty pattern track skipped, and an alias through its source, as
/// Renoise plays it (up to the shorter pattern's length).
pub const RENOISE_NOTES: &str = r##"
local R = renoise
local out = {notes = 0}
local deadline = os.clock() + 0.5
local p = FROM
while p <= #song.patterns do
  local pattern = song:pattern(p)
  for t = 1, #song.tracks do
    if song:track(t).type == R.Track.TRACK_TYPE_SEQUENCER then
      local pt, lines = pattern:track(t), pattern.number_of_lines
      if pt.is_alias then
        local source = song:pattern(pt.alias_pattern_index)
        pt, lines = source:track(t), math.min(lines, source.number_of_lines)
      end
      if not pt.is_empty then
        for l = 1, lines do
          local line = pt:line(l)
          if not line.is_empty then
            for c = 1, 12 do if line:note_column(c).note_value < 120 then out.notes = out.notes + 1 end end
          end
        end
      end
    end
  end
  p = p + 1
  if p <= #song.patterns and os.clock() > deadline then
    out.next = p
    break
  end
end
return out
"##;

// ---------------------------------------------------------------------------------------------------------------------
// Helpers for the checks

fn list(value: &Value) -> Vec<&Value> {
    value.as_array().into_iter().flatten().collect()
}

pub fn track<'a>(state: &'a Value, name: &str) -> Option<&'a Value> {
    list(&state["tracks"]).into_iter().find(|track| track["name"] == name)
}

fn track_like<'a>(state: &'a Value, needle: &str) -> Option<&'a Value> {
    list(&state["tracks"]).into_iter().find(|track| track["name"].as_str().is_some_and(|name| name.to_lowercase().contains(needle)))
}

/// Whether a REAPER track's sound reaches `to` (a track's name, or "master"): up its folders while its parent send is on,
/// and through each unmuted send.
fn reaches(state: &Value, from: &str, to: &str, depth: usize) -> bool {
    let Some(track) = track(state, from) else { return false };
    if depth > 24 {
        return false;
    }
    if from == to {
        return true;
    }
    let up = track["mainSend"] == true
        && match track["parent"].as_str() {
            Some(parent) => reaches(state, parent, to, depth + 1),
            None => to == "master",
        };
    up || list(&track["sends"]).iter().any(|send| {
        send["mute"] != true
            && send["volume"].as_f64().unwrap_or(0.0) > 0.0
            && reaches(state, send["to"].as_str().unwrap_or_default(), to, depth + 1)
    })
}

/// How many ways a REAPER track's sound reaches the master.
pub fn master_paths(state: &Value, name: &str) -> usize {
    fn count(state: &Value, name: &str, depth: usize) -> usize {
        let Some(track) = track(state, name) else { return 0 };
        if depth > 24 {
            return 0;
        }
        let up = if track["mainSend"] == true {
            match track["parent"].as_str() {
                Some(parent) => count(state, parent, depth + 1),
                None => 1,
            }
        } else {
            0
        };
        up + list(&track["sends"])
            .iter()
            .filter(|send| send["mute"] != true && send["volume"].as_f64().unwrap_or(0.0) > 0.0)
            .map(|send| count(state, send["to"].as_str().unwrap_or_default(), depth + 1))
            .sum::<usize>()
    }
    count(state, name, 0)
}

fn mentions(said: &str, words: &[&str]) -> bool {
    // (typographic apostrophes and dashes read as plain ones)
    let said = said.to_lowercase().replace(['’', '‘'], "'").replace(['–', '—'], "-");
    words.iter().any(|word| said.contains(&word.to_lowercase()))
}

fn number_in(text: &str) -> Option<f64> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let digits: String = text[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    digits.parse().ok()
}

fn sorted(value: &Value) -> Vec<String> {
    let mut items: Vec<String> = list(value).iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    items.sort();
    items
}

// ---------------------------------------------------------------------------------------------------------------------
// The scenarios

pub fn all() -> Vec<Scenario> {
    vec![
        // REAPER: the routing doctor.
        Scenario {
            id: "P30",
            daw: Daw::Reaper,
            says: &["The kick is way too loud and I can't find why"],
            before: None,
            // Kick sits in the Drums folder (to the master through it) and also sends to Drum Bus: it reaches the master twice.
            fixture: r#"
local drums, kick, snare, bus, bass = track("Drums", 1), track("Kick"), track("Snare", -1), track("Drum Bus"), track("Bass")
reaper.CreateTrackSend(kick, bus)
for _, t in ipairs({kick, snare, bass}) do reaper.CreateNewMIDIItemInProj(t, 0, 8, false) end
"#,
            read: "",
            teardown: None,
            generates: false,
            names: &["Drum Bus"],
            // (PR #400 review D19, D23: two routes to the master are the producer's own; the expected answer names both and asks
            // one question, nothing changed. One route removed also passes, with the kick's fader untouched and how to undo said.)
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let paths = master_paths(after, "Kick");
                let asked = paths == 2
                    && seen.changes == 0
                    && seen.said.contains("Drum Bus")
                    && seen.said.contains("Drums")
                    && seen.said.matches('?').count() == 1;
                let fixed = paths == 1 && mentions(seen.said, &["undo"]);
                let mut checks = vec![
                    Check::new(
                        "the kick reached the master twice before",
                        master_paths(before, "Kick") == 2,
                        format!("{} paths", master_paths(before, "Kick")),
                    ),
                    Check::new(
                        "both routes named with one question, nothing changed; or one removed and how to undo said",
                        asked || fixed,
                        format!("{} paths, {} changes, {} questions", paths, seen.changes, seen.said.matches('?').count()),
                    ),
                ];
                for name in ["Snare", "Bass", "Drum Bus", "Drums"] {
                    let same = track(before, name).map(|t| (&t["volume"], &t["mute"], &t["mainSend"]))
                        == track(after, name).map(|t| (&t["volume"], &t["mute"], &t["mainSend"]));
                    checks.push(Check::new(&format!("{name} left as it was"), same, ""));
                }
                let kick = |state: &Value| track(state, "Kick").map(|t| t["volume"].clone());
                checks.push(Check::new(
                    "the kick's fader is left alone",
                    kick(before) == kick(after),
                    format!("{:?} → {:?}", kick(before), kick(after)),
                ));
                checks
            },
        },
        // REAPER: SFX variations for a game, numbered and exported.
        Scenario {
            id: "P19",
            daw: Daw::Reaper,
            says: &["Make 12 variations of this gravel footstep from these three layers and export them for the game, numbered"],
            before: None,
            fixture: r#"
for i = 1, 3 do
  local it = audio(track("Gravel " .. i), MEDIA .. "/gravel" .. i .. ".wav", 0, 0.4)
  reaper.SetMediaItemSelected(it, true)
end
"#,
            read: "",
            teardown: None,
            generates: false,
            names: &["01", "Gravel 1", "Gravel 2", "Gravel 3"],
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let source = |state: &Value, i: usize| {
                    track(state, &format!("Gravel {i}"))
                        .and_then(|t| list(&t["items"]).into_iter().find(|item| item["pos"].as_f64() == Some(0.0)).cloned())
                        .map(|item| (item["len"].clone(), item["rate"].clone(), item["pitch"].clone(), item["offset"].clone()))
                };
                let kept = (1..=3).all(|i| source(before, i).is_some() && source(before, i) == source(after, i));
                let regions = list(&after["regions"]);
                let names: Vec<&str> = regions.iter().filter_map(|r| r["name"].as_str()).collect();
                let numbered = names.len() >= 12
                    && names.iter().all(|name| !name.contains(' ') && name.chars().rev().take(2).all(|c| c.is_ascii_digit()))
                    && names.iter().collect::<std::collections::BTreeSet<_>>().len() == names.len();
                let variations: std::collections::BTreeSet<String> = list(&after["tracks"])
                    .iter()
                    .flat_map(|t| list(&t["items"]))
                    .filter(|item| item["pos"].as_f64().unwrap_or(0.0) > 0.0)
                    .map(|item| format!("{}|{}|{}|{}", item["rate"], item["pitch"], item["offset"], item["fadein"]))
                    .collect();
                vec![
                    Check::new("the three source layers are kept as they were", kept, ""),
                    Check::new(
                        "at least 12 regions, named without spaces and numbered with two digits or more",
                        numbered,
                        names.join(", "),
                    ),
                    Check::new(
                        "the variations differ (pitch, rate, offset or fades)",
                        variations.len() >= 6,
                        format!("{} distinct", variations.len()),
                    ),
                    Check::new("the render lists 12 files", after["targets"].as_u64() == Some(12), format!("{}", after["targets"])),
                ]
            },
        },
        // REAPER: a new instrument in a big orchestral template.
        Scenario {
            id: "P23",
            daw: Daw::Reaper,
            says: &["Add a celesta and make sure it lands in the right stem"],
            before: None,
            fixture: r#"
local sections = {
  {"Strings", {"Violins I", "Violins II", "Violas", "Celli", "Basses"}},
  {"Woodwinds", {"Piccolo", "Flutes", "Oboes", "Clarinets", "Bassoons"}},
  {"Brass", {"Horns", "Trumpets", "Trombones", "Tuba"}},
  {"Percussion", {"Timpani", "Snare Drum", "Bass Drum", "Cymbals", "Glockenspiel", "Xylophone"}},
  {"Keys & Harp", {"Harp", "Piano", "Organ"}},
  {"Choir", {"Sopranos", "Altos", "Tenors", "Basses (Choir)"}},
}
local folders = {}
for _, s in ipairs(sections) do
  local folder = track(s[1], 1)
  reaper.SetMediaTrackInfo_Value(folder, "B_MAINSEND", 0)
  local last
  for _, name in ipairs(s[2]) do
    for _, part in ipairs({"", " Longs", " Shorts", " FX"}) do
      local t = track(name .. part)
      if part ~= "" then
        reaper.SetMediaTrackInfo_Value(t, "B_SHOWINTCP", 0)
        reaper.SetMediaTrackInfo_Value(t, "B_SHOWINMIXER", 0)
      end
      local fx = reaper.TrackFX_AddByName(t, "ReaSynth", false, -1)
      reaper.TrackFX_SetOffline(t, fx, true)
      last = t
    end
  end
  reaper.SetMediaTrackInfo_Value(last, "I_FOLDERDEPTH", -1)
  folders[#folders + 1] = {folder, s[1]}
end
for _, f in ipairs(folders) do reaper.CreateTrackSend(f[1], track("STEM " .. f[2])) end
track("FX Hall Reverb")
"#,
            read: "",
            teardown: None,
            generates: false,
            names: &["Keys & Harp"],
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let celesta = track_like(after, "celesta");
                let parent = celesta.and_then(|t| t["parent"].as_str()).unwrap_or_default().to_owned();
                let stem = celesta.is_some_and(|t| reaches(after, t["name"].as_str().unwrap_or_default(), "STEM Keys & Harp", 0));
                let hidden_shown: Vec<String> = list(&before["tracks"])
                    .iter()
                    .filter(|t| t["hidden"] == true)
                    .filter(|t| track(after, t["name"].as_str().unwrap_or_default()).is_some_and(|now| now["hidden"] != true))
                    .map(|t| t["name"].as_str().unwrap_or_default().to_owned())
                    .collect();
                let onlined: Vec<String> = list(&before["tracks"])
                    .iter()
                    .filter(|t| list(&t["fx"]).iter().any(|fx| fx["offline"] == true))
                    .filter(|t| {
                        track(after, t["name"].as_str().unwrap_or_default())
                            .is_some_and(|now| list(&now["fx"]).iter().any(|fx| fx["offline"] == false))
                    })
                    .map(|t| t["name"].as_str().unwrap_or_default().to_owned())
                    .collect();
                vec![
                    Check::new("a celesta track is added", celesta.is_some(), ""),
                    Check::new("it sits in the Keys & Harp folder", parent == "Keys & Harp", parent),
                    Check::new("it reaches STEM Keys & Harp", stem, ""),
                    Check::new("no hidden track shown", hidden_shown.is_empty(), hidden_shown.join(", ")),
                    Check::new("no offline instrument brought online", onlined.is_empty(), onlined.join(", ")),
                ]
            },
        },
        // REAPER: a changed setup (time timebase, loop source off, fades off) honoured through a stretch and a tempo change.
        Scenario {
            id: "P24",
            daw: Daw::Reaper,
            says: &["Stretch the synth part to 16 bars and push the tempo to 128"],
            before: None,
            fixture: r#"
reaper.set_config_var_string("itemtimelock", "0", 0)
reaper.set_config_var_string("loopnewitems", "8", 0)
reaper.set_config_var_string("splitautoxfade", "8", 0)
local notes = {}
for bar = 0, 7 do for b = 0, 3 do notes[#notes + 1] = {bar * 2 + b * 0.5, 0.4, 60 + (b % 3) * 4} end end
local it = midi(track("Synth"), 0, 16, notes)
reaper.SetMediaItemInfo_Value(it, "B_LOOPSRC", 0)
audio(track("Drums"), MEDIA .. "/break120.wav", 0, 8)
"#,
            read: "",
            teardown: Some(
                r#"reaper.set_config_var_string("itemtimelock", "1", 0) reaper.set_config_var_string("loopnewitems", "14", 0) reaper.set_config_var_string("splitautoxfade", "0", 0) return "ok""#,
            ),
            generates: false,
            names: &["4 bars", "Drums", "break120"],
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let tempo = after["tempo"].as_f64().unwrap_or(0.0);
                let synth = track(after, "Synth").and_then(|t| list(&t["items"]).first().cloned().cloned()).unwrap_or(Value::Null);
                let beats = synth["len"].as_f64().unwrap_or(0.0) * tempo / 60.0;
                let drums = |state: &Value| {
                    track(state, "Drums")
                        .and_then(|t| list(&t["items"]).first().map(|i| (i["pos"].clone(), i["len"].clone(), i["rate"].clone())))
                };
                let fades = |state: &Value| -> Vec<(f64, f64)> {
                    list(&state["tracks"])
                        .iter()
                        .flat_map(|t| list(&t["items"]))
                        .map(|i| (i["fadein"].as_f64().unwrap_or(0.0), i["fadeout"].as_f64().unwrap_or(0.0)))
                        .collect()
                };
                vec![
                    Check::new("the tempo is 128", (tempo - 128.0).abs() < 0.01, format!("{tempo}")),
                    Check::new("the synth item is 16 bars long at the new tempo", (beats - 64.0).abs() < 0.5, format!("{beats:.2} beats")),
                    Check::new("the synth item doesn't loop its source", synth["loop"] == false, format!("{}", synth["loop"])),
                    Check::new(
                        "the drums aren't stretched or moved",
                        drums(before).is_some() && drums(before) == drums(after),
                        format!("{:?}", drums(after)),
                    ),
                    Check::new("no fades added", fades(after).iter().all(|(i, o)| *i == 0.0 && *o == 0.0), format!("{:?}", fades(after))),
                ]
            },
        },
        // REAPER: three compressors compared, level-matched, the best kept.
        Scenario {
            id: "P26",
            daw: Daw::Reaper,
            says: &["Compare these three compressors on the vocal, level-matched, and keep the best"],
            before: None,
            fixture: r#"
local vox = track("Vocal")
audio(vox, MEDIA .. "/vocal.wav", 0, 6)
-- Each set to work on the line: its threshold near -20 dB (found through its own formatted values), all three bypassed.
for _, name in ipairs({"ReaComp", "JS: sstillwell/1175", "JS: sstillwell/expressbus"}) do
  local fx = reaper.TrackFX_AddByName(vox, name, false, -1)
  if fx < 0 then error("no FX " .. name) end
  for p = 0, reaper.TrackFX_GetNumParams(vox, fx) - 1 do
    local _, pname = reaper.TrackFX_GetParamName(vox, fx, p, "")
    if pname:lower():find("thresh") then
      local best, gap = 0, math.huge
      for v = 0, 1.0001, 0.005 do
        reaper.TrackFX_SetParamNormalized(vox, fx, p, v)
        local _, text = reaper.TrackFX_GetFormattedParamValue(vox, fx, p, "")
        local db = tonumber(text:match("[-+]?%d+%.?%d*"))
        if db and math.abs(db + 20) < gap then best, gap = v, math.abs(db + 20) end
      end
      reaper.TrackFX_SetParamNormalized(vox, fx, p, best)
      break
    end
  end
  reaper.TrackFX_SetEnabled(vox, fx, false)
end
audio(track("Music"), MEDIA .. "/bass.wav", 0, 6)
"#,
            read: "",
            teardown: None,
            generates: false,
            names: &["ReaComp", "1175", "Express"],
            expect: |seen| {
                let after = seen.after;
                let compressors: Vec<&Value> = track(after, "Vocal")
                    .map(|t| list(&t["fx"]))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|fx| {
                        fx["name"].as_str().is_some_and(|n| n.contains("ReaComp") || n.contains("1175") || n.contains("General Dynamics"))
                    })
                    .collect();
                let on: Vec<&str> = compressors.iter().filter(|fx| fx["enabled"] == true).filter_map(|fx| fx["name"].as_str()).collect();
                let listened = seen.tools.iter().any(|tool| matches!(tool.as_str(), "audition" | "hear" | "listen" | "judge"));
                vec![
                    Check::new("exactly one of the three compressors is left on", on.len() == 1, on.join(", ")),
                    Check::new("Kumi listened to them", listened, seen.tools.join(", ")),
                    Check::new("it made the choice as a change", seen.changes > 0, format!("{} changes", seen.changes)),
                    Check::new("it says how the levels were matched", mentions(seen.said, &["level", "loudness", "lufs", "match"]), ""),
                ]
            },
        },
        // REAPER: a tutorial watched and recreated (parity with Live).
        Scenario {
            id: "TR-REAPER",
            daw: Daw::Reaper,
            says: &["Watch https://www.youtube.com/watch?v=a1sh2o78XyY and set up what it shows here with my Kick and Bass"],
            before: None,
            fixture: r#"
audio(track("Kick"), MEDIA .. "/kickloop.wav", 0, 8)
audio(track("Bass"), MEDIA .. "/bass.wav", 0, 8)
"#,
            read: "",
            teardown: None,
            generates: false,
            names: &["ReaComp", "3/4"],
            expect: |seen| {
                let after = seen.after;
                let send = track(after, "Kick").and_then(|t| list(&t["sends"]).into_iter().find(|s| s["to"] == "Bass").cloned());
                let bass = track(after, "Bass");
                let detector = bass
                    .map(|t| list(&t["fx"]))
                    .unwrap_or_default()
                    .into_iter()
                    .find_map(|fx| fx["detector"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                vec![
                    Check::new("watched the video", seen.tools.iter().any(|t| t == "watch_video"), seen.tools.join(", ")),
                    Check::new(
                        "the kick sends to the bass on channels 3/4",
                        send.as_ref().is_some_and(|s| s["dstchan"].as_f64() == Some(2.0)),
                        format!("{send:?}"),
                    ),
                    Check::new("the bass has 4 channels or more", bass.is_some_and(|t| t["nchan"].as_f64().unwrap_or(0.0) >= 4.0), ""),
                    Check::new(
                        "the bass's compressor listens to an input other than its main one",
                        !detector.is_empty() && detector != "0",
                        detector,
                    ),
                    Check::new("the kick still reaches the master", reaches(after, "Kick", "master", 0), ""),
                ]
            },
        },
        // Renoise: R31 older effect spellings read as Renoise's.
        Scenario {
            id: "R31",
            daw: Daw::Renoise,
            says: &["Put an S40 on the second snare and 09C0 on line 20"],
            before: None,
            // Snare hits on lines 04, 12, 20 and 28 of the first pattern (LPB 4, 32 lines).
            fixture: r#"
song:track(1).name = "Snare"
sample_instrument(1, "Snare", MEDIA .. "/snare.wav")
local p = seq:pattern(1)
song:pattern(p).number_of_lines = 32
for _, line in ipairs({4, 12, 20, 28}) do note(p, 1, line, "C-4", 0) end
"#,
            read: r#"
local p = song.sequencer:pattern(1)
out.snare = {lines = {}, notes = notes_of(p, 1)}
local pt = song:pattern(p):track(1)
for l = 1, song:pattern(p).number_of_lines do
  local line, effects = pt:line(l), {}
  for c = 1, 8 do
    local fx = line:effect_column(c)
    if not fx.is_empty then effects[#effects + 1] = fx.number_string .. fx.amount_string end
  end
  for c = 1, 12 do
    local nc = line:note_column(c)
    if nc.effect_number_value ~= 0 or nc.effect_amount_value ~= 0 then effects[#effects + 1] = nc.effect_number_string .. nc.effect_amount_string end
  end
  if #effects > 0 then out.snare.lines[#out.snare.lines + 1] = {line = l - 1, effects = effects} end
end
"#,
            teardown: None,
            generates: false,
            names: &["0S40", "0SC0", "line 12"],
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let effects = |state: &Value, line: u64| -> Vec<String> {
                    list(&state["snare"]["lines"])
                        .into_iter()
                        .find(|row| row["line"] == line)
                        .map(|row| list(&row["effects"]).iter().filter_map(|fx| fx.as_str().map(str::to_owned)).collect())
                        .unwrap_or_default()
                };
                let others: Vec<u64> =
                    (0..32).filter(|line| ![12, 20].contains(line) && effects(before, *line) != effects(after, *line)).collect();
                vec![
                    Check::new(
                        "0S40 on the second snare (line 12)",
                        effects(after, 12).iter().any(|fx| fx == "0S40"),
                        format!("{:?}", effects(after, 12)),
                    ),
                    Check::new("0SC0 on line 20", effects(after, 20).iter().any(|fx| fx == "0SC0"), format!("{:?}", effects(after, 20))),
                    Check::new("the snare's notes are as they were", before["snare"]["notes"] == after["snare"]["notes"], ""),
                    Check::new("no other line's effects changed", others.is_empty(), format!("{others:?}")),
                ]
            },
        },
        // Renoise: R21 bounce a range into an instrument.
        Scenario {
            id: "R21",
            daw: Daw::Renoise,
            says: &["Bounce bars 1–4 of the break into a new instrument so I can chop it again"],
            before: None,
            fixture: r#"
song.transport.bpm = 170
sample_instrument(1, "Amen Break", MEDIA .. "/break.wav")
song:track(1).name = "Break"
note(seq:pattern(1), 1, 0, "C-4", 0)
"#,
            read: r#"out.lines = notes_of(song.sequencer:pattern(1), 1)"#,
            teardown: None,
            generates: false,
            names: &["Amen"],
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let count = |state: &Value| list(&state["instruments"]).len();
                let new = list(&after["instruments"]).into_iter().nth(count(before)).cloned().unwrap_or(Value::Null);
                let frames = list(&new["samples"]).first().and_then(|s| s["frames"].as_f64()).unwrap_or(0.0);
                let rate = list(&new["samples"]).first().and_then(|s| s["rate"].as_f64()).unwrap_or(44_100.0).max(1.0);
                let want = 16.0 * 60.0 / 170.0;
                let name = new["name"].as_str().unwrap_or_default().to_owned();
                vec![
                    Check::new("one new instrument", count(after) == count(before) + 1, format!("{} → {}", count(before), count(after))),
                    Check::new(
                        "its sample is the 4 bars (5.6 s at 170 BPM)",
                        (frames / rate) > want * 0.85 && (frames / rate) < want * 1.4,
                        format!("{:.2} s", frames / rate),
                    ),
                    Check::new("it's named", !name.is_empty() && name != "Amen Break", name),
                    Check::new(
                        "the source instrument and lines are as they were",
                        before["instruments"][0] == after["instruments"][0] && before["lines"] == after["lines"],
                        "",
                    ),
                ]
            },
        },
        // Renoise: R23 layer two breaks and glue them.
        Scenario {
            id: "R23",
            daw: Daw::Renoise,
            says: &["Layer the Think under the Amen and glue them"],
            before: None,
            fixture: r#"
song.transport.bpm = 170
sample_instrument(1, "Amen", MEDIA .. "/amen.wav")
sample_instrument(2, "Think", MEDIA .. "/think.wav")
song:track(1).name = "Amen"
song:track(2).name = "Think"
note(seq:pattern(1), 1, 0, "C-4", 0)
note(seq:pattern(1), 2, 0, "C-4", 1)
"#,
            read: r#"out.lines = {notes_of(song.sequencer:pattern(1), 1), notes_of(song.sequencer:pattern(1), 2)}"#,
            teardown: None,
            generates: false,
            names: &["Bus Compressor"],
            expect: |seen| {
                let after = seen.after;
                let (amen, think) = (track(after, "Amen"), track(after, "Think"));
                let group = amen.and_then(|t| t["group"].as_str()).unwrap_or_default().to_owned();
                let grouped = !group.is_empty() && think.and_then(|t| t["group"].as_str()) == Some(group.as_str());
                let compressor = track(after, &group).and_then(|g| {
                    list(&g["devices"])
                        .into_iter()
                        .find(|d| d["path"].as_str().is_some_and(|p| p.ends_with("Bus Compressor") || p.ends_with("/Compressor")))
                        .cloned()
                });
                let ratio = compressor.as_ref().and_then(|c| c["params"]["Ratio"].as_str().and_then(number_in));
                let level =
                    |t: Option<&Value>| t.map(|t| t["pre"].as_f64().unwrap_or(1.0) * t["post"].as_f64().unwrap_or(1.0)).unwrap_or(0.0);
                vec![
                    Check::new("the Amen and the Think are in one group", grouped, group.clone()),
                    Check::new("the group has a bus compressor", compressor.is_some(), ""),
                    Check::new("its ratio is about 2:1", ratio.is_some_and(|r| (1.5..=3.0).contains(&r)), format!("{ratio:?}")),
                    Check::new(
                        "the Think sits under the Amen",
                        level(think) <= level(amen),
                        format!("{:.2} vs {:.2}", level(think), level(amen)),
                    ),
                    Check::new("the notes are as they were", seen.before["lines"] == after["lines"], ""),
                ]
            },
        },
        // Renoise: R25 tidy duplicate patterns and merge hat columns.
        Scenario {
            id: "R25",
            daw: Daw::Renoise,
            says: &["Tidy this song: drop the duplicate patterns and merge the two hat columns"],
            before: None,
            fixture: r#"
song:track(1).name = "Kick"
song:track(2).name = "Hats"
song:track(2).visible_note_columns = 2
sample_instrument(1, "Kit", MEDIA .. "/kick.wav")
local function fill(p, variant)
  song:pattern(p).number_of_lines = 64
  for l = 0, 63, 16 do note(p, 1, l, "C-4", 0) end
  if variant == 2 then note(p, 1, 40, "C-4", 0) end
  for l = 0, 60, 4 do note(p, 2, l, "C-5", 0, 1) end
  for l = 2, 62, 4 do note(p, 2, l, variant == 4 and "D-5" or "C#5", 0, 2) end
end
fill(seq:pattern(1), 1)
fill(seq:insert_new_pattern_at(2), 2)
fill(seq:insert_new_pattern_at(3), 1)
fill(seq:insert_new_pattern_at(4), 4)
"#,
            read: r#"
out.slots = {}
for s, p in ipairs(song.sequencer.pattern_sequence) do
  out.slots[s] = {pattern = p, kick = notes_of(p, 1), hats1 = notes_of(p, 2, 1), hats2 = notes_of(p, 2, 2)}
end
"#,
            teardown: None,
            generates: false,
            names: &["03", "Hats"],
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let slots = |state: &Value| list(&state["slots"]).len();
                let slot = |state: &Value, s: usize| state["slots"][s].clone();
                let same = slot(after, 2)["pattern"] == slot(after, 0)["pattern"];
                let kicks = (0..4).all(|s| slot(before, s)["kick"] == slot(after, s)["kick"]);
                let merged = (0..slots(after)).all(|s| {
                    let mut was = sorted(&slot(before, s)["hats1"]);
                    was.extend(sorted(&slot(before, s)["hats2"]));
                    was.sort();
                    list(&slot(after, s)["hats2"]).is_empty() && sorted(&slot(after, s)["hats1"]) == was
                });
                vec![
                    Check::new("the sequence still has its 4 slots", slots(after) == 4, format!("{}", slots(after))),
                    Check::new(
                        "slot 3 plays the same pattern as slot 1",
                        same,
                        format!("{} / {}", slot(after, 0)["pattern"], slot(after, 2)["pattern"]),
                    ),
                    Check::new("the kick plays as it did", kicks, ""),
                    Check::new("the hats are one column, every hit kept", merged, ""),
                ]
            },
        },
        // Renoise: R29 a limit explained, with a way around.
        Scenario {
            id: "R29",
            daw: Daw::Renoise,
            says: &["Make my reverb send feed back into itself"],
            before: None,
            fixture: r#"
local reverb = song:insert_track_at(song.sequencer_track_count + 2)
reverb.name = "Reverb"
reverb:insert_device_at("Audio/Effects/Native/Reverb", 2)
local vox = song:track(1)
vox.name = "Vox"
local send = vox:insert_device_at("Audio/Effects/Native/#Send", 2)
for _, prm in ipairs(send.parameters) do if prm.name == "Receiver" then prm.value = 0 end end
sample_instrument(1, "Vox", MEDIA .. "/vocal.wav")
note(seq:pattern(1), 1, 0, "C-4", 0)
"#,
            read: "",
            teardown: None,
            generates: false,
            names: &["#Send", "Vox"],
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let sends_on = |state: &Value| {
                    track(state, "Reverb")
                        .map(|t| list(&t["devices"]).iter().filter(|d| d["path"].as_str().is_some_and(|p| p.contains("#Send"))).count())
                        .unwrap_or(0)
                };
                vec![
                    Check::new(
                        "no send set to feed the reverb back into itself",
                        sends_on(after) == sends_on(before),
                        format!("{}", sends_on(after)),
                    ),
                    Check::new(
                        "it says Renoise can't",
                        mentions(
                            seen.said,
                            &["can't", "cannot", "can not", "doesn't", "does not", "isn't possible", "not possible", "won't"],
                        ),
                        "",
                    ),
                    Check::new(
                        "it offers a way around (a Delay's feedback or another send)",
                        mentions(seen.said, &["delay", "another send", "second send"]),
                        "",
                    ),
                ]
            },
        },
        // Renoise: R35 a jam cleaned up, only what's new touched.
        Scenario {
            id: "R35",
            daw: Daw::Renoise,
            says: &["Clean up what I just recorded"],
            before: Some(
                r#"
song:track(1).name = "Keys"
song:track(2).name = "Drums"
sample_instrument(1, "Keys", MEDIA .. "/bass.wav")
sample_instrument(2, "Kit", MEDIA .. "/kick.wav")
local p1 = seq:pattern(1)
for l = 0, 60, 8 do note(p1, 1, l, "C-4", 0) end
for l = 0, 60, 4 do note(p1, 2, l, "C-4", 1) end
local p2 = seq:insert_new_pattern_at(2)
for l = 0, 60, 8 do note(p2, 1, l, "E-4", 0) end
for l = 0, 60, 4 do note(p2, 2, l, "C-4", 1) end
renoise.app():save_song_as(SONGS .. "/r35-jam.xrns")
"#,
            ),
            // While Kumi was closed: six patterns recorded after the two (near copies, empties, an exact copy, a fill).
            fixture: r#"
local seq = song.sequencer
local function note(p, t, line, text, inst)
  local nc = song:pattern(p):track(t):line(line + 1):note_column(1)
  nc.note_string = text
  nc.instrument_value = inst
end
local function copy_of(at, extra)
  local p = seq:insert_new_pattern_at(at)
  song:pattern(p):copy_from(song:pattern(seq:pattern(1)))
  if extra then note(p, 1, extra, "G-4", 0) end
  return p
end
copy_of(3, 12)
seq:insert_new_pattern_at(4)
copy_of(5, 20)
copy_of(6, nil)
seq:insert_new_pattern_at(7)
local fill = seq:insert_new_pattern_at(8)
for l = 48, 63 do note(fill, 2, l, "C-4", 1) end
"#,
            read: r#"
out.slots = {}
for s, p in ipairs(song.sequencer.pattern_sequence) do
  local keys, drums = notes_of(p, 1), notes_of(p, 2)
  out.slots[s] = {pattern = p, keys = keys, drums = drums, empty = #keys == 0 and #drums == 0}
end
"#,
            teardown: None,
            generates: false,
            names: &["04", "06", "07"],
            expect: |seen| {
                let (before, after) = (seen.before, seen.after);
                let content = |state: &Value, s: usize| (state["slots"][s]["keys"].clone(), state["slots"][s]["drums"].clone());
                let kept = (0..2).all(|s| content(before, s) == content(after, s));
                let empties = list(&after["slots"]).iter().filter(|slot| slot["empty"] == true).count();
                // (the proposal's item 4: Kumi says what's new and proposes the clean-up, or makes it; it never edits what came
                // before)
                let new =
                    &["6 new", "six new", "six patterns", "6 patterns", "slots 3", "3-8", "patterns 2-7", "patterns 02", "to 8 slots"];
                vec![
                    Check::new("the two patterns from before the jam are as they were", kept, ""),
                    Check::new("it says what was new (six patterns, slots 3 to 8)", mentions(seen.said, new), ""),
                    Check::new(
                        "it proposes or makes the clean-up (the empty patterns among it)",
                        empties == 0 || mentions(seen.said, &["empty"]),
                        format!("{empties} empty slots left"),
                    ),
                ]
            },
        },
        // Renoise: a tutorial watched and recreated (parity with Live).
        Scenario {
            id: "TR-RENOISE",
            daw: Daw::Renoise,
            says: &["Watch https://www.youtube.com/watch?v=gIw9rEGl1u8 and set up what it shows here with my Kick and Bass tracks"],
            before: None,
            fixture: r#"
sample_instrument(1, "Kick", MEDIA .. "/kick.wav")
sample_instrument(2, "Bass", MEDIA .. "/bass.wav")
song:track(1).name = "Kick"
song:track(2).name = "Bass"
for l = 0, 60, 4 do note(seq:pattern(1), 1, l, "C-4", 0) end
note(seq:pattern(1), 2, 0, "C-3", 1)
"#,
            read: r#"out.lines = {notes_of(song.sequencer:pattern(1), 1), notes_of(song.sequencer:pattern(1), 2)}"#,
            teardown: None,
            generates: false,
            names: &["#Sidechain"],
            expect: |seen| {
                let after = seen.after;
                let has = |name: &str, path: &str| {
                    track(after, name)
                        .is_some_and(|t| list(&t["devices"]).iter().any(|d| d["path"].as_str().is_some_and(|p| p.contains(path))))
                };
                vec![
                    Check::new("watched the video", seen.tools.iter().any(|t| t == "watch_video"), seen.tools.join(", ")),
                    Check::new("the bass has a compressor", has("Bass", "Compressor"), ""),
                    Check::new("the kick feeds it through a #Sidechain", has("Kick", "#Sidechain"), ""),
                    Check::new("the notes are as they were", seen.before["lines"] == after["lines"], ""),
                ]
            },
        },
    ]
}
