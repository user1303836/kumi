//! MCP over JSON-RPC: initialize, ping, tools/list and tools/call for the host's tools.
use crate::host::{Host, Pick};
use crate::listening::{MAX_CAPTURE_SECONDS, MAX_COPIES, MAX_PRE_ROLL_BEATS, MAX_SCRATCH_VALUES, MAX_SOURCES, MAX_WAIT_MS, SAMPLE_RATES};
use crate::tools::{kinds, DEFAULT_LIMIT, MAX_GRID, MAX_LIMIT};
use daw_bridge::Daw;
use serde_json::{json, Value};

pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The tools, described for the model.
/// Whether a DAW's tool list has this tool (the names read once per DAW).
fn listed(daw: Daw, name: &str) -> bool {
    static NAMES: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>> = std::sync::OnceLock::new();
    let names = NAMES.get_or_init(Default::default);
    let mut names = names.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let list = names.entry(daw.to_string()).or_insert_with(|| {
        tool_list(daw).as_array().into_iter().flatten().filter_map(|tool| tool["name"].as_str().map(str::to_owned)).collect()
    });
    list.iter().any(|listed| listed == name)
}

pub fn tools(host: &Host) -> Value {
    tool_list(host.daw())
}

fn tool_list(daw: Daw) -> Value {
    let kinds = kinds(daw);
    // Each DAW's own words: what it opens, what a read's parent holds, what a view is about.
    let (place, inside, parent, about) = match daw {
        Daw::Renoise => (
            "song",
            "a track's devices or a device's parameters",
            "The ref of what the rows are in: a track for its devices, a device for its parameters, an instrument for its samples or phrases, a pattern (with track) for its lines.",
            "the track, pattern or instrument it is about",
        ),
        _ => (
            "project",
            "a track's items or an FX's parameters",
            "The ref of what the rows are in: a track for its items or sends, an FX for its parameters.",
            "the take or track it is about",
        ),
    };
    // Renoise plays from a slot and line; REAPER from where it is (cursor_set moves it).
    let play = match daw {
        Daw::Renoise => (
            "Starts Renoise's playback from where it is, or from sequence (a slot, from 1) and line (from 0, as Renoise numbers lines).",
            json!({"sequence": {"type": "integer", "minimum": 1}, "line": {"type": "integer", "minimum": 0}, "epoch": {"type": "string"}}),
        ),
        _ => ("Starts REAPER's playback from where it is (cursor_set moves it first).", json!({"epoch": {"type": "string"}})),
    };
    let pick = json!({
        "pid": {"type": "integer", "minimum": 1, "description": format!("The {daw} process to use, from discover, when several run.")},
        "resource": {"type": "string", "description": format!("The {daw} resource or preferences folder to use, from discover, when several run.")},
    });
    let mut list = json!([
        {
            "name": "status",
            "title": format!("{daw} status"),
            "description": format!("Whether Kumi's bridge in {daw} is connected: the {daw} and bridge versions, the open {place}, and whether {daw} is busy (a dialog, a render). With several {daw} instances running, pass pid or resource from discover."),
            "inputSchema": {"type": "object", "properties": pick, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "discover",
            "title": format!("Find running {daw}"),
            "description": format!("Every running {daw} with Kumi's bridge: process id, port, resource folder, versions, open {place}, and whether it answers. Use it when status reports none, or to choose among several."),
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "read",
            "title": format!("Read {daw}"),
            "description": format!("Rows of one kind from the open {daw} {place}, a page at a time: {{items, nextCursor?, epoch}}. Pass parent (a row's ref) to read what is inside it, such as {inside}; pass the previous page's nextCursor as cursor for the next page. Or pass ref alone for the one object it names. Refs hold until the epoch changes. With several {daw} instances running, pass pid or resource from discover."),
            "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                "kind": {"type": "string", "enum": kinds.read, "description": format!("What to read in {daw}.")},
                "parent": {"type": "string", "description": parent},
                "ref": {"type": "string", "description": "The ref of one object to read instead of a page of rows."},
                "cursor": {"type": "string", "description": "The nextCursor an earlier page gave, unchanged."},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": format!("Rows in a page ({DEFAULT_LIMIT} by default).")},
                "grid": {"type": "integer", "minimum": 2, "maximum": MAX_GRID, "description": "For kind param: each parameter's text read at this many points across its range (grid), or its steps' texts (items)."},
            })), "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "snapshot",
            "title": format!("{daw} snapshot"),
            "description": format!("The open {daw} {place} on one screen: {{epoch, project, transport, tracks, selection, markers?}}. What is too big to include is named under truncated, to read with `read`."),
            "inputSchema": {"type": "object", "properties": pick, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "focus",
            "title": format!("{daw} focus"),
            "description": format!("What the producer is looking at in {daw}: {{epoch, view, selection, transport, changed}}. Cheap enough to poll about every 150 ms; read again only when changed moves."),
            "inputSchema": {"type": "object", "properties": pick, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": false, "openWorldHint": false},
        },
        {
            "name": "focus_body",
            "title": format!("{daw} view"),
            "description": format!("What {daw} shows in one of its views, ready to draw: pass kind, and ref ({about}) or window (the range to show) where the kind takes them. Long ones come in pages: pass the nextCursor as cursor."),
            "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                "kind": {"type": "string", "enum": kinds.bodies, "description": "Which view."},
                "ref": {"type": "string", "description": "The ref of what the view is about."},
                "around": {"type": "string", "description": "The ref of a track to centre the first page of tracks on (the arrange view): the page starts half a page above it, and says where in `from`."},
                "window": {"type": "object", "properties": {"from": {"type": "number"}, "to": {"type": "number"}}, "required": ["from", "to"], "additionalProperties": false, "description": "The range to show, in the view's own units."},
                "cursor": {"type": "string", "description": "The nextCursor an earlier page gave, unchanged."},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Rows in a page."},
            })), "required": ["kind"], "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "change_preview",
            "title": format!("Preview a {daw} change"),
            "description": format!("What one change would do in {daw}, without making it: {{transactionId, confirmation, prior, display, target, epoch}}. Apply it with change_apply."),
            "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                "kind": {"type": "string", "description": "The change kind."},
                "input": {"type": "object", "description": "What the kind takes."},
                "epoch": {"type": "string", "description": "The epoch the refs in input came from."},
            })), "required": ["kind", "input"], "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": false, "openWorldHint": false},
        },
        {
            "name": "change_apply",
            "title": format!("Apply a {daw} change"),
            "description": format!("Makes a previewed change in {daw}, as one named step of {daw}'s undo. The same idempotencyKey again returns the first answer and changes nothing. state is applied, or uncertain when {daw} didn't answer."),
            "inputSchema": {"type": "object", "properties": {
                "transactionId": {"type": "string"},
                "confirmation": {"type": "string", "description": "The confirmation change_preview gave."},
                "idempotencyKey": {"type": "string"},
                "title": {"type": "string", "description": format!("The change's name in {daw}'s undo history.")},
            }, "required": ["transactionId", "confirmation", "idempotencyKey"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "change_undo",
            "title": format!("Undo a {daw} change"),
            "description": format!("Takes a change back as a new named step in {daw}: exactly what it changed is put back. Refused (code changed) if those values changed since, unless discard."),
            "inputSchema": {"type": "object", "properties": {
                "transactionId": {"type": "string"},
                "confirmation": {"const": "undo"},
                "idempotencyKey": {"type": "string"},
                "discard": {"type": "boolean", "description": "Put the values back even if they changed since."},
            }, "required": ["transactionId", "confirmation", "idempotencyKey"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "transport_stop",
            "title": format!("Stop {daw}"),
            "description": format!("Stops {daw}'s playback and recording at once."),
            "inputSchema": {"type": "object", "properties": pick, "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "transport_play",
            "title": format!("Play {daw}"),
            "description": play.0,
            "inputSchema": {"type": "object", "properties": merged(&pick, play.1), "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "cursor_set",
            "title": format!("Move {daw}'s cursor"),
            "description": format!("Moves {daw}'s edit cursor to positionBeats (quarter notes from the start) or position (seconds); the play position follows while it plays."),
            "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                "positionBeats": {"type": "number", "minimum": 0},
                "position": {"type": "number", "minimum": 0},
                "epoch": {"type": "string"},
            })), "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "undo_step_begin",
            "title": format!("Begin one {daw} undo step"),
            "description": format!("Opens one step of {daw}'s undo around the changes that follow, where {daw} gives one: {{stepId}}. Close it with undo_step_end."),
            "inputSchema": {"type": "object", "properties": {"label": {"type": "string"}}, "required": ["label"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false},
        },
        {
            "name": "undo_step_end",
            "title": format!("End the {daw} undo step"),
            "description": format!("Closes the step undo_step_begin opened; label names it in {daw}'s undo by what it did."),
            "inputSchema": {"type": "object", "properties": {"stepId": {"type": "string"}, "label": {"type": "string", "maxLength": 200}}, "required": ["stepId"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "change_release",
            "title": "Forget changes",
            "description": "Changes that won't be undone any more: the host forgets them (at most 64 at a time).",
            "inputSchema": {"type": "object", "properties": {
                "transactionIds": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 64},
            }, "required": ["transactionIds"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        },
    ]);
    // (r-tracks) Saving the project, in either DAW: a file written, nothing in the project changed, so no change and no
    // undo. The bridge's `project_save`; Renoise refuses `version`.
    let (save_title, save) = match daw {
        Daw::Reaper => ("Save the REAPER project", "Saves the open REAPER project where it is, or as its next version (version: true). A project never saved is saved only under path, a .rpp inside the folder REAPER saves projects in, and never over a file."),
        _ => ("Save the Renoise song", "Saves the open Renoise song where it is, or as path (a whole path to a .xrns file that isn't there yet; the song is that file from then on). A song never saved is saved only under path, and never over a file. (version is REAPER's: Renoise refuses it.)"),
    };
    if let Some(tools) = list.as_array_mut() {
        tools.push(json!({
            "name": "project_save",
            "title": save_title,
            "description": save,
            "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                "version": {"type": "boolean"},
                "path": {"type": "string"},
                "epoch": {"type": "string", "description": "The epoch of the answer the project was read in: if the project open now is another, the save is refused as stale."},
            })), "required": ["epoch"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        }));
        tools.extend(listening_tools(daw, &pick));
    }
    if daw == Daw::Reaper {
        // REAPER's refs are GUIDs, which a copy of the project in another tab shares: the epoch says which project they came from.
        let epoch = json!({
            "type": "string",
            "description": "The epoch of the answer the ref, parent or cursor came from. If the project in the tab has changed since, the call is refused as stale instead of reading the other project.",
        });
        list[2]["inputSchema"]["properties"]["epoch"] = epoch.clone();
        list[5]["inputSchema"]["properties"]["epoch"] = epoch.clone();
        // An API function that only reads, by name (kind call).
        list[2]["inputSchema"]["properties"]["fn"] =
            json!({"type": "string", "description": "For kind call: a REAPER API function that only reads, by name."});
        list[2]["inputSchema"]["properties"]["args"] = json!({
            "type": ["array", "object"],
            "description": "For kind call: its arguments, in order or by the SDK's names; tracks, items, takes and envelopes as refs.",
        });
        // (r-tracks) Saving tracks as a template: a file written, nothing in the project changed, so no change and no undo.
        if let Some(tools) = list.as_array_mut() {
            tools.push(json!({
                "name": "track_template_save",
                "title": "Save REAPER tracks as a template",
                "description": "Writes tracks as a track template (.RTrackTemplate) in REAPER's TrackTemplates folder (name) or at path; never over a file unless replace.",
                "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                    "trackRefs": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                    "name": {"type": "string"},
                    "path": {"type": "string"},
                    "items": {"type": "boolean"},
                    "envelopes": {"type": "boolean"},
                    "replace": {"type": "boolean"},
                    "epoch": epoch,
                })), "required": ["trackRefs", "epoch"], "additionalProperties": false},
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false},
            }));
        }
    }
    if daw == Daw::Renoise {
        // A pattern's lines are read one track at a time.
        list[2]["inputSchema"]["properties"]["track"] = json!({
            "type": ["integer", "string"],
            "description": "For kind line: the track (its number, or its ref) whose lines of the parent pattern to read.",
        });
        // Track refs are positions: a request names the layout its refs were made in, and older ones are carried
        // through the tracks inserted, removed or moved since.
        let layout = json!({"type": "integer", "minimum": 0, "description": "The layout the refs were read in (each answer and tracks event says it): refs from an older one are carried through the track shifts since."});
        list[2]["inputSchema"]["properties"]["layout"] = layout.clone();
        list[5]["inputSchema"]["properties"]["layout"] = layout.clone();
        list[6]["inputSchema"]["properties"]["layout"] = layout;
        // Notes in columns the track doesn't show are listed under `hidden` for a line with nothing in the shown ones; for every
        // line when asked (about six times the reading).
        let hidden = json!({"type": "boolean", "description": "Also list data in the note and effect columns the track doesn't show, on every line (under `hidden`). Without it only a line with nothing in the shown columns is looked through."});
        list[2]["inputSchema"]["properties"]["hidden"] = hidden.clone();
        list[5]["inputSchema"]["properties"]["hidden"] = hidden;
        // Renoise's scripts can't hear what it plays: it only renders. Its tool has no edit cursor to move in beats or
        // seconds (transport_play takes a slot and line).
        if let Some(tools) = list.as_array_mut() {
            tools.retain(|tool| tool["name"] != "hear" && tool["name"] != "cursor_set");
        }
        // (M4 n-devices) A device read by ref says what a Doofer, Splitter or Notepad holds; its preset data on ask.
        list[2]["inputSchema"]["properties"]["presetData"] = json!({"type": "boolean", "description": "With ref of a device: also its preset data, its settings as Renoise's XML (for device_preset_data). A Doofer's, Splitter's or Notepad's contents come by themselves."});
        // (M4 n-devices) Actions on a device, no step of Renoise's undo: its own editor window, its settings saved as a preset.
        let own = |more: Value| {
            merged(
                &pick,
                merged(
                    &json!({"ref": {"type": "string", "description": "A device on a track"}, "epoch": {"type": "string"}, "layout": {"type": "integer", "minimum": 0}}),
                    more,
                ),
            )
        };
        if let Some(tools) = list.as_array_mut() {
            tools.push(json!({
                "name": "device_editor",
                "title": "Show a device's editor",
                "description": "Shows a plug-in's own editor window (or a *LFO's), or hides it with show false. Only when the producer asks to see it.",
                "inputSchema": {"type": "object", "properties": own(json!({"show": {"type": "boolean"}})), "required": ["ref"], "additionalProperties": false},
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            }));
            tools.push(json!({
                "name": "show_status",
                "title": "A line in Renoise's status bar",
                "description": "Shows one line of text in Renoise's status bar (never a dialog), for a producer working in Renoise: what Kumi finished, or that it needs them. Not a change.",
                "inputSchema": {"type": "object", "properties": merged(&pick, json!({"text": {"type": "string", "minLength": 1, "maxLength": 512}})), "required": ["text"], "additionalProperties": false},
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            }));
            tools.push(json!({
                "name": "preset_save",
                "title": "Save a device's preset",
                "description": "Saves a native device's settings as a preset file named name in Renoise's user library (Effect Presets, in the device's folder): {file, listed}. Renoise lists it in the device's presets from its next start. A preset of that name is replaced only with replace; plug-ins are refused (Renoise lists only the presets it saves for them).",
                "inputSchema": {"type": "object", "properties": own(json!({"name": {"type": "string", "maxLength": 128}, "replace": {"type": "boolean"}})), "required": ["ref", "name"], "additionalProperties": false},
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false},
            }));
        }
        // Renoise's actions (M4): a note auditioned, things selected or shown, an instrument saved to a file (n-instruments).
        // None is a change. (Saving the song is the neutral project_save, listed for both DAWs.)
        let same_as = |name: &str| json!({"type": "string", "description": format!("As {name}: pass one of the two.")});
        let actions = json!([
            {
                "name": "audition_note",
                "title": "Audition a note in Renoise",
                "description": "Plays a note (or notes, at once) on an instrument, out loud, for durationMs, on track (the selected one by default); sample plays that sample alone. stop: true ends what is playing.",
                "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                    "instrument": {"type": "string", "description": "The instrument's ref."},
                    "instrumentRef": {"type": "string", "description": "The instrument's ref (as instrument: pass one of the two)."},
                    "track": {"type": "string", "description": "The track's ref to play on."},
                    "trackRef": {"type": "string", "description": "The track's ref (as track: pass one of the two)."},
                    "note": {"type": ["string", "integer"], "description": "C-4, C#5 or 0 to 119 (48 is C-4)."},
                    "notes": {"type": "array", "items": {"type": ["string", "integer"]}, "minItems": 1, "maxItems": 16},
                    "volume": {"type": "number", "minimum": 0, "maximum": 1},
                    "durationMs": {"type": "number", "minimum": 1, "maximum": 10000},
                    "sample": {"type": "string", "description": "A sample's ref, to play it alone."},
                    "sampleRef": {"type": "string", "description": "A sample's ref (as sample: pass one of the two)."},
                    "stop": {"type": "boolean"},
                    "epoch": {"type": "string"},
                    "layout": {"type": "integer", "minimum": 0},
                })), "additionalProperties": false},
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false},
            },
            {
                "name": "select",
                "title": "Select or show in Renoise",
                "description": "Selects a track (or its device), an instrument (or its sample or phrase), a sequence slot (or the first slot that plays a pattern) and a line; shows a middle frame, the lower frame or the pattern matrix. Not an undo step.",
                "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                    "track": {"type": "string"}, "device": {"type": "string"}, "instrument": {"type": "string"},
                    "sample": {"type": "string"}, "phrase": {"type": "string"}, "sequence": {"type": "string"}, "pattern": {"type": "string"},
                    "trackRef": same_as("track"), "deviceRef": same_as("device"), "instrumentRef": same_as("instrument"),
                    "sampleRef": same_as("sample"), "phraseRef": same_as("phrase"), "sequenceRef": same_as("sequence"), "patternRef": same_as("pattern"),
                    "line": {"type": "integer", "minimum": 1},
                    "frame": {"type": "string", "enum": ["pattern", "mixer", "phrase", "keyzones", "sample", "modulation", "effects", "plugin", "midi"]},
                    "lower": {"type": "string", "enum": ["dsp", "automation", "hidden"]},
                    "matrix": {"type": "boolean"},
                    "epoch": {"type": "string"},
                    "layout": {"type": "integer", "minimum": 0},
                })), "additionalProperties": false},
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
            },
            {
                "name": "instrument_save",
                "title": "Save a Renoise instrument",
                "description": "Saves an instrument to an .xrni file at path (absolute; the extension is added when missing), never over a file already there unless path names that file exactly and overwrite: true. Not a change: no undo.",
                "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                    "instrumentRef": {"type": "string", "description": "The instrument."},
                    "ref": {"type": "string", "description": "The instrument (as instrumentRef)."},
                    "path": {"type": "string"},
                    "overwrite": {"type": "boolean"},
                    "epoch": {"type": "string"},
                    "layout": {"type": "integer", "minimum": 0},
                })), "required": ["path"], "additionalProperties": false},
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false},
            },
        ]);
        if let (Some(tools), Value::Array(more)) = (list.as_array_mut(), actions) {
            tools.extend(more);
        }
    }
    if daw == Daw::Reaper {
        // m4-r-fxenv: a chain saved as an FX chain file.
        list.as_array_mut().expect("a list").push(json!({
            "name": "fx_chain_save",
            "title": "Save an FX chain file",
            "description": "Saves a chain's FX (fxRefs: some of them, in the chain's order) as an FX chain file (.RfxChain) under name in REAPER's FXChains folder: no FX ids, no automation. A file of that name is replaced only with overwrite. A file, not the project: no step of REAPER's undo.",
            "inputSchema": {"type": "object", "properties": merged(&pick, json!({
                "ref": {"type": "string", "description": "The track, take, item or container whose chain to save."},
                "input": {"type": "boolean", "description": "The track's input FX chain."},
                "fxRefs": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 256},
                "name": {"type": "string", "minLength": 1, "maxLength": 256, "description": "The file's name in FXChains; sub-folders as \"Vocals/Lead\"."},
                "overwrite": {"type": "boolean"},
                "epoch": {"type": "string"},
            })), "required": ["ref", "name"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false},
        }));
    }
    // M6: what changed in the project while Kumi wasn't looking, and what the producer did while it watched.
    let catchup = json!([
        {
            "name": "catch_up",
            "title": format!("What changed in {daw}"),
            "description": format!("What changed in the open {daw} project since this host last kept its snapshot (edits made while Kumi was closed, or while {daw} was restarted): {{place, name, lastSeenAt, project: [details], changes: [added | removed | changed], more?, same?}}, or first: true when there is no snapshot yet. The snapshot is kept again afterwards."),
            "inputSchema": {"type": "object", "properties": pick, "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "idempotentHint": false, "openWorldHint": false},
        },
        {
            "name": "baseline_save",
            "title": format!("Keep the {daw} project's snapshot"),
            "description": format!("Keeps the open {daw} project's snapshot again, so the next catch_up tells only what changes after now. Refused (saved: false) until catch_up has compared this project, so what changed while Kumi was away is never lost."),
            "inputSchema": {"type": "object", "properties": pick, "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "idempotentHint": true, "openWorldHint": false},
        },
        {
            "name": "watch_start",
            "title": format!("Watch {daw}"),
            "description": format!("Starts watching the producer work in {daw}: the project as it is now is kept, and what they add is looked at as it comes. Nothing in {daw} changes."),
            "inputSchema": {"type": "object", "properties": pick, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": false, "openWorldHint": false},
        },
        {
            "name": "watch_stop",
            "title": format!("Stop watching {daw}"),
            "description": format!("Stops watching and says what the producer did since watch_start: {{watched, changes (with each value before and after, what an added thing is made of, the parameters they moved by name), project, selected (the tracks they selected)}}."),
            "inputSchema": {"type": "object", "properties": pick, "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": false, "openWorldHint": false},
        }
    ]);
    list.as_array_mut().expect("a list").extend(catchup.as_array().cloned().unwrap_or_default());
    list
}

/// The listening tools (M5): renders and captures as jobs, playing a file out loud, the audio behind a ref, scratch
/// copies. One `json!` each (the macro's recursion stops at one long list).
fn listening_tools(daw: Daw, pick: &Value) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "render",
            "title": format!("Render in {daw}"),
            "description": format!("Renders a stretch of the {daw} project offline (silently, faster than real time) to WAV files in Kumi's folder, as a job: {{jobId, state, seconds, files}}. Follow it with listen_job, stop it with listen_cancel. Every setting {daw} changes for it is put back."),
            "inputSchema": {"type": "object", "properties": merged(pick, json!({
                "sources": {"type": "array", "minItems": 1, "maxItems": MAX_SOURCES, "description": "What to render, each to its own file.", "items": {"type": "object", "properties": {
                    "id": {"type": "string", "pattern": "^[A-Za-z0-9_-]{1,32}$", "description": "Your name for the file."},
                    "what": {"enum": ["master", "track"], "description": "The mix as it leaves the master track, or one track on its own."},
                    "ref": {"type": "string", "description": "For a track: its ref."},
                    "stage": {"enum": ["pre-fader", "post-fader", "items"], "description": "For a track: before or after its fader (pre-fader by default), or its own items before its FX (alone)."},
                    "beforeChain": {"type": "boolean", "description": "For the master: the mix as it comes into the master's chain."},
                }, "required": ["id", "what"], "additionalProperties": false}},
                "range": {"type": "object", "properties": {"fromBeat": {"type": "number", "minimum": 0}, "beats": {"type": "number", "exclusiveMinimum": 0}}, "required": ["fromBeat", "beats"], "additionalProperties": false, "description": "Quarter notes from the project's start."},
                "preRollBeats": {"type": "number", "minimum": 0, "maximum": MAX_PRE_ROLL_BEATS, "description": "Rendered before the part so effects are warm; each file's start says where the part begins."},
                "sampleRate": {"enum": SAMPLE_RATES},
                "epoch": {"type": "string"},
                "layout": {"type": "integer", "minimum": 0},
            })), "required": ["sources", "range"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false},
        }),
        json!({
            "name": "hear",
            "title": format!("Hear {daw} playing"),
            "description": format!("Captures what {daw} is playing (the master output, as it plays) for some seconds, as a job: {{jobId, state, seconds, files}}. Follow it with listen_job: its marks say where the project was at each jump."),
            "inputSchema": {"type": "object", "properties": merged(pick, json!({
                "seconds": {"type": "number", "minimum": 1, "maximum": MAX_CAPTURE_SECONDS},
            })), "required": ["seconds"], "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": false, "openWorldHint": false},
        }),
        json!({
            "name": "listen_job",
            "title": "Listening job",
            "description": "Where a render or a capture stands: {jobId, kind, state (queued, running, done, failed, cancelled), progress 0 to 1, files when done, error when failed, notes}. waitMs waits for it to end (at most 10 s), answering sooner when it does.",
            "inputSchema": {"type": "object", "properties": merged(pick, json!({
                "jobId": {"type": "string"},
                "waitMs": {"type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS},
            })), "required": ["jobId"], "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": false, "openWorldHint": false},
        }),
        json!({
            "name": "listen_cancel",
            "title": "Stop a listening job",
            "description": format!("Stops a render or a capture as soon as {daw} can, puts back what it changed and deletes its files: {{jobId, state}}."),
            "inputSchema": {"type": "object", "properties": merged(pick, json!({"jobId": {"type": "string"}})), "required": ["jobId"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        }),
        json!({
            "name": "audition",
            "title": format!("Play out loud in {daw}"),
            "description": format!("Plays an audio file once, out loud, where the producer listens to {daw}, without changing the project: {{previewId, playing, seconds, via}}. A new one stops the one playing; it stops when {daw} starts playing too."),
            "inputSchema": {"type": "object", "properties": merged(pick, json!({
                "file": {"type": "string", "description": "An absolute path."},
                "fromSeconds": {"type": "number", "minimum": 0},
                "seconds": {"type": "number", "exclusiveMinimum": 0},
                "gainDb": {"type": "number", "minimum": -60, "maximum": 12},
            })), "required": ["file"], "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": false, "openWorldHint": false},
        }),
        json!({
            "name": "audition_stop",
            "title": "Stop playing out loud",
            "description": "Stops the file playing out loud (the one named, or whichever): {stopped}.",
            "inputSchema": {"type": "object", "properties": merged(pick, json!({"previewId": {"type": "string"}})), "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false},
        }),
        json!({
            "name": "audio_file",
            "title": format!("{daw} audio file"),
            "description": format!("The audio a ref stands for in {daw}: {{file, seconds, channels, sampleRate, kind, name}} (an item's source file, a sample's audio saved to Kumi's folder), or a render job ({{jobId, …}}) when it has to be rendered (a pattern)."),
            "inputSchema": {"type": "object", "properties": merged(pick, json!({
                "ref": {"type": "string"},
                "epoch": {"type": "string"},
                "layout": {"type": "integer", "minimum": 0},
            })), "required": ["ref"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
        }),
        json!({
            "name": "scratch",
            "title": format!("Scratch copies in {daw}"),
            "description": format!("Copies of a track for Kumi to hear settings side by side, never reaching the master while the producer plays: make (ref, count, label) → {{scratchId, copies}}, set (values on the copies by device and parameter place) and drop. Copies left by a crash are removed when {daw}'s bridge loads."),
            "inputSchema": {"type": "object", "properties": merged(pick, json!({
                "action": {"enum": ["make", "set", "drop"]},
                "ref": {"type": "string", "description": "make: the track to copy."},
                "count": {"type": "integer", "minimum": 1, "maximum": MAX_COPIES},
                "label": {"type": "string", "maxLength": 80},
                "range": {"type": "object", "properties": {"fromBeat": {"type": "number", "minimum": 0}, "beats": {"type": "number", "exclusiveMinimum": 0}}, "required": ["fromBeat", "beats"], "additionalProperties": false},
                "scratchId": {"type": "string"},
                "values": {"type": "array", "minItems": 1, "maxItems": MAX_SCRATCH_VALUES, "items": {"type": "object", "properties": {
                    "ref": {"type": "string", "description": "The copy."},
                    "device": {"type": "integer", "minimum": 0, "description": "The device's place in the copied track's chain, as read."},
                    "param": {"type": "integer", "minimum": 0},
                    "normalized": {"type": "number", "minimum": 0, "maximum": 1},
                    "value": {"type": "number"},
                }, "required": ["ref", "device", "param"], "additionalProperties": false}},
                "epoch": {"type": "string"},
                "layout": {"type": "integer", "minimum": 0},
            })), "required": ["action"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false},
        }),
    ];
    if daw == Daw::Reaper {
        tools.extend(delivery_tools(daw, pick));
    }
    tools
}

/// (#401) REAPER's renders for delivery: the producer's own files, named by its wildcards, in their folder.
fn delivery_tools(daw: Daw, pick: &Value) -> Vec<Value> {
    let spec = json!({
        "source": {"enum": ["master", "matrix", "stems"], "description": "The mix (the default), each region's tracks in the region render matrix, or the tracks named."},
        "bounds": {"description": "regions (the default), selected-regions, project, time-selection, or {start, end} in seconds.", "anyOf": [
            {"enum": ["regions", "selected-regions", "project", "time-selection"]},
            {"type": "object", "properties": {"start": {"type": "number", "minimum": 0}, "end": {"type": "number", "minimum": 0}}, "required": ["start", "end"], "additionalProperties": false}
        ]},
        "trackRefs": {"type": "array", "items": {"type": "string"}, "description": "A stems render's tracks."},
        "format": {"enum": ["wav", "project"], "description": "WAV (the default), or the format set in the render dialog."},
        "bits": {"enum": [16, 24, 32], "description": "A WAV's bit depth (24 by default; 32 is float)."},
        "sampleRate": {"description": "A rate, or project.", "anyOf": [{"enum": [22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000]}, {"enum": ["project"]}]},
        "channels": {"enum": [1, 2]},
        "tailMs": {"type": "integer", "minimum": 0, "maximum": 30000, "description": "Rendered past each region's end, for reverbs to ring out."},
        "pattern": {"type": "string", "minLength": 1, "maxLength": 512, "description": "The files' names, with the wildcards ($region, $track, $project, $timelineorder…)."},
        "folder": {"type": "string", "description": "A whole path; the project's render folder by default."},
        "overwrite": {"type": "boolean", "description": "Write over files that are there already."},
        "expect": {"type": "array", "items": {"type": "string"}, "description": "The files the dry run listed: the render refuses others."},
        "epoch": {"type": "string"},
    });
    vec![
        json!({
            "name": "deliver_targets",
            "title": format!("Files a {daw} delivery would write"),
            "description": format!("The files a delivery render would write, as {daw} lists them, without rendering: {{targets, folder, count, there (those already there)}}. Every render setting is put back before the answer."),
            "inputSchema": {"type": "object", "properties": merged(pick, spec.clone()), "required": ["pattern"], "additionalProperties": false},
            "annotations": {"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false},
        }),
        json!({
            "name": "deliver",
            "title": format!("Deliver from {daw}"),
            "description": format!("Renders the producer's files with {daw}'s own render, as a job: {{jobId, state}}, then listen_job gives {{files}}. It renders only the files the dry run listed (expect), never over a file that's there unless overwrite, and puts every render setting back after."),
            "inputSchema": {"type": "object", "properties": merged(pick, spec), "required": ["pattern"], "additionalProperties": false},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true},
        }),
    ]
}

/// The properties of a tool: the instance pick, then its own.
fn merged(pick: &Value, own: Value) -> Value {
    let mut all = pick.as_object().cloned().unwrap_or_default();
    all.extend(own.as_object().cloned().unwrap_or_default());
    Value::Object(all)
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn valid_id(id: &Value) -> bool {
    match id {
        Value::String(text) => !text.is_empty() && text.len() <= 256,
        Value::Number(n) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

fn tool_result(value: Value, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": serde_json::to_string(&value).expect("JSON serializes")}],
        "structuredContent": value,
        "isError": is_error,
    })
}

/// The answer to one message, or None for a notification.
pub async fn handle(host: &Host, message: &Value) -> Option<Value> {
    let Some(envelope) = message.as_object() else { return Some(error(Value::Null, -32600, "Invalid Request")) };
    let method = envelope.get("method").and_then(Value::as_str);
    let id = envelope.get("id").cloned();
    if envelope.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error(id.filter(valid_id).unwrap_or(Value::Null), -32600, "Invalid Request"));
    }
    let Some(method) = method else {
        // A response to a request this server never sends, or junk: nothing to answer.
        return id.is_none().then(|| error(Value::Null, -32600, "Invalid Request"));
    };
    let id = id?;
    if !valid_id(&id) {
        return Some(error(Value::Null, -32600, "Invalid Request"));
    }
    let params = envelope.get("params").cloned().unwrap_or_else(|| json!({}));
    if !params.is_object() {
        return Some(error(id, -32602, "Invalid params"));
    }
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(LATEST_PROTOCOL_VERSION);
            let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&asked) { asked } else { LATEST_PROTOCOL_VERSION };
            result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "daw-mcp-server", "title": format!("Kumi's {} bridge", host.daw()), "version": env!("CARGO_PKG_VERSION")},
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({"tools": tools(host)})),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else { return Some(error(id, -32602, "Invalid params")) };
            let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            if !arguments.is_object() {
                return Some(error(id, -32602, "Invalid params"));
            }
            // A tool this DAW's list doesn't have (the other DAW's, say) is unknown here: its bridge isn't asked.
            if !listed(host.daw(), name) {
                return Some(error(id, -32602, &format!("Unknown tool: {name}")));
            }
            match name {
                "status" => match Pick::from_arguments(&arguments) {
                    Ok(pick) => result(id, tool_result(host.status(&pick).await, false)),
                    Err(why) => result(id, tool_result(json!({"error": why}), true)),
                },
                "discover" => result(id, tool_result(host.discover().await, false)),
                "read" | "snapshot" | "focus" | "focus_body" => {
                    let (answer, is_error) = match name {
                        "read" => host.read(&arguments).await,
                        "snapshot" => host.snapshot(&arguments).await,
                        "focus" => host.focus(&arguments).await,
                        _ => host.focus_body(&arguments).await,
                    };
                    result(id, tool_result(answer, is_error))
                }
                "catch_up" | "baseline_save" | "watch_start" | "watch_stop" => {
                    let (answer, is_error) = match name {
                        "catch_up" => host.catch_up(&arguments).await,
                        "baseline_save" => host.baseline_save(&arguments).await,
                        "watch_start" => host.watch_start(&arguments).await,
                        _ => host.watch_stop(&arguments).await,
                    };
                    result(id, tool_result(answer, is_error))
                }
                "render" | "hear" | "listen_job" | "listen_cancel" | "audition" | "audition_stop" | "audio_file" | "scratch"
                | "deliver_targets" | "deliver" => {
                    let (answer, is_error) = match name {
                        "render" => host.render(&arguments).await,
                        "deliver_targets" => host.deliver_targets(&arguments).await,
                        "deliver" => host.deliver(&arguments).await,
                        "hear" => host.hear(&arguments).await,
                        "listen_job" => host.listen_job(&arguments).await,
                        "listen_cancel" => host.listen_cancel(&arguments).await,
                        "audition" => host.audition(&arguments).await,
                        "audition_stop" => host.audition_stop(&arguments).await,
                        "audio_file" => host.audio_file(&arguments).await,
                        _ => host.scratch(&arguments).await,
                    };
                    result(id, tool_result(answer, is_error))
                }
                // (M4 n-devices) Renoise's actions on a device.
                "device_editor" | "preset_save" | "show_status" if host.daw() == Daw::Renoise => {
                    let (answer, is_error) = host.renoise_action(name, &arguments).await;
                    result(id, tool_result(answer, is_error))
                }
                "change_preview" | "change_apply" | "change_undo" | "change_release" | "transport_stop" | "transport_play"
                | "cursor_set" | "undo_step_begin" | "undo_step_end" | "audition_note" | "select" | "instrument_save" => {
                    let (answer, is_error) = match name {
                        "audition_note" | "select" | "instrument_save" => host.action(name, &arguments).await,
                        "change_preview" => host.change_preview(&arguments).await,
                        "change_apply" => host.change_apply(&arguments).await,
                        "change_undo" => host.change_undo(&arguments).await,
                        "transport_stop" => host.transport_stop(&arguments).await,
                        "transport_play" | "cursor_set" => host.transport(name, &arguments).await,
                        "undo_step_begin" | "undo_step_end" => host.undo_step(name, &arguments).await,
                        _ => host.change_release(&arguments),
                    };
                    result(id, tool_result(answer, is_error))
                }
                // m4-r-fxenv
                "fx_chain_save" if host.daw() == Daw::Reaper => {
                    let (answer, is_error) = host.fx_chain_save(&arguments).await;
                    result(id, tool_result(answer, is_error))
                }
                // (r-tracks) Saving: the project in either DAW, tracks as a template in REAPER.
                "project_save" | "track_template_save" if name == "project_save" || host.daw() == Daw::Reaper => {
                    let (answer, is_error) = host.passthrough(name, &arguments).await;
                    result(id, tool_result(answer, is_error))
                }
                _ => error(id, -32602, &format!("Unknown tool: {name}")),
            }
        }
        _ => error(id, -32601, "Method not found"),
    })
}
