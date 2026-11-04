//! `focus_body`: what FOCUS draws for the view the producer is in, from the session as a read sees it: the Edit window's
//! tracks and clips (`edit`), a MIDI clip's notes (`midi`), a track's plug-ins (`fx`) and the Mix window's strips
//! (`mixer`). Every position is in quarter notes, through the session's tempo map ([`Timebase`]): a session whose map
//! holds what 0.17.0's step maps don't has no beats, and its Edit window and MIDI clips are refused rather than drawn in
//! seconds. The Edit window and the mixer page by track, a clip's notes by note, with the cursors `read` pages by; refs
//! are `read`'s, in the epoch the body carries. SoundCraft has no routing window of a track's (its I/O and sends are
//! sections of the Mix window), so there's no routing body: `read` reads a track's routing.
use super::{
    bad, fill, gone, insert_rows, limit, list, now, text, thousandths, tracks, Cursor, Reads, Refusal, ANSWER_BYTES, DEFAULT_LIMIT,
    MAX_LIMIT,
};
use crate::refs::{Id, Kind, Lives, Missing, Ref};
use crate::timebase::{Timebase, PPQ};
use daw_bridge::wire::code;
use serde_json::{json, Value};
use std::time::Instant;

/// The bodies SoundCraft's bridge draws.
pub const BODY_KINDS: [&str; 4] = ["edit", "midi", "fx", "mixer"];
/// What an Edit page's first answer keeps for the markers, regions and meter.
const EDIT_EXTRAS_BYTES: usize = 48 * 1024;
/// Clips one track lists in the window, and the bytes they may take: the page's first track always goes in.
const ITEMS_PER_TRACK: usize = 400;
const ITEMS_BYTES_PER_TRACK: usize = 64 * 1024;
/// Markers and regions in the window, and the bytes they may take, whatever their names; meter changes.
const WINDOW_MARKERS: usize = 200;
const MARKERS_BYTES: usize = 32 * 1024;
const WINDOW_METERS: usize = 200;
/// The longest Edit window asked for, in quarter notes.
const MAX_WINDOW: f64 = 100.0 * 3600.0;
/// Notes in a page by default and at most, and what the clip's own fields keep of an answer.
const DEFAULT_NOTES: usize = 500;
const MAX_NOTES: usize = 2000;
const NOTES_EXTRAS_BYTES: usize = 16 * 1024;
/// Track strips in a mixer page by default and at most (the master faders' come on top).
const MIXER_STRIPS: usize = 24;
const MAX_MIXER_STRIPS: usize = 64;
const MASTERS_BYTES: usize = 16 * 1024;

/// A body asked for, its refs checked for their kind before the app is asked.
enum Asked {
    Edit { around: Option<Id>, window: Option<(f64, f64)> },
    Midi { clip: Option<Id> },
    Fx { track: Option<Id> },
    Mixer { around: Option<Id> },
}

impl Reads {
    /// One body: `{kind, ref?, around?, cursor?, window?, limit?, epoch?}`. Refs from `epoch`, when it's given, are read
    /// only in that epoch; without it, in the epoch now. Not started after `until`. `ui` is the app's last `ui.inspect`:
    /// the plug-in window an `fx` body is of by default, and the stretch the Edit window shows.
    pub async fn focus_body(&mut self, params: &Value, ui: Option<&Value>, until: Instant) -> Result<Value, Refusal> {
        let kind = text(params, "kind")?.ok_or_else(|| bad(format!("kind: one of {}", BODY_KINDS.join(", "))))?;
        let named = |key: &str| text(params, key)?.map(|named| named.parse::<Ref>().map_err(bad)).transpose();
        let (reference, around) = (named("ref")?, named("around")?);
        let asked = match kind.as_str() {
            "edit" => Asked::Edit { around: track_ref("around", around)?, window: window(params)? },
            "midi" => Asked::Midi {
                clip: match reference {
                    None => None,
                    Some(Ref::Object(clip @ Id { kind: Kind::Clip, .. })) => Some(clip),
                    Some(other) => return Err(bad(format!("ref: a MIDI clip (s:clip:…), not {other}"))),
                },
            },
            "fx" => Asked::Fx {
                track: match reference {
                    Some(Ref::Insert { track, .. }) => Some(track),
                    other => track_ref("ref", other)?,
                },
            },
            "mixer" => Asked::Mixer { around: track_ref("around", around)? },
            other => return Err(bad(format!("SoundCraft has no {other:?} body: use one of {}", BODY_KINDS.join(", ")))),
        };
        let most = match asked {
            Asked::Edit { .. } => limit(params, DEFAULT_LIMIT, MAX_LIMIT)?,
            Asked::Midi { .. } => limit(params, DEFAULT_NOTES, MAX_NOTES)?,
            Asked::Mixer { .. } => limit(params, MIXER_STRIPS, MAX_MIXER_STRIPS)?,
            Asked::Fx { .. } => 0,
        };
        let cursor = text(params, "cursor")?.as_deref().map(Cursor::parse).transpose()?;
        let epoch = text(params, "epoch")?;
        let session = self.inspect(until).await?;
        self.same_epoch(epoch)?;
        let mut body = match asked {
            Asked::Edit { around, window } => self.edit_body(&session, ui, window, around, cursor.as_ref(), most)?,
            Asked::Midi { clip } => self.midi_body(&session, clip, cursor.as_ref(), most, until).await?,
            Asked::Fx { track } => self.fx_body(&session, ui, track, until).await?,
            Asked::Mixer { around } => self.mixer_body(&session, around, cursor.as_ref(), most)?,
        };
        body["epoch"] = self.epoch().into();
        body["kind"] = kind.into();
        Ok(body)
    }

    /// The clip SoundCraft's MIDI Editor shows of `session` (a look's, in the refs of the epoch now), as the app chooses
    /// it (`target_clip`): the first selected MIDI clip; else a clip on a selected MIDI or instrument track under the
    /// insertion point or in the selection; else the first clip on such a track. None when it shows none.
    pub fn midi_editor_clip(&self, session: &Value) -> Option<String> {
        midi_editor_clip(session, &self.lives).map(|clip| Ref::Object(clip).to_string())
    }

    /// The Edit window: the page of tracks from the cursor's, or centred on `around`'s, else from the first, each with the
    /// clips that touch the window (`from`, `to` in quarter notes; the whole session when none is asked for). The first
    /// page has the rest: the playhead, the selection, the markers and regions in the window, the meter, and the stretch
    /// the Edit window shows now (`view`), from its timeline's width (`ui.inspect`) and the session's zoom.
    fn edit_body(
        &self,
        session: &Value,
        ui: Option<&Value>,
        window: Option<(f64, f64)>,
        around: Option<Id>,
        cursor: Option<&Cursor>,
        most: usize,
    ) -> Result<Value, Refusal> {
        let timebase = Timebase::of(session).ok_or_else(no_beats)?;
        let beats = |samples: &Value| samples.as_f64().map(|samples| timebase.beats(samples));
        let length = beats(&session["length_samples"]).unwrap_or(0.0);
        let (from, to) = window.unwrap_or((0.0, length.max(16.0)));
        let keyed = self.keyed(tracks(session).iter());
        let start = self.page_start(session, &keyed, cursor, around, most)?;
        let first_page = cursor.is_none();
        let budget = if first_page { ANSWER_BYTES - EDIT_EXTRAS_BYTES } else { ANSWER_BYTES };
        let rows = keyed[start..].iter().map(|(key, track)| edit_track(session, &timebase, &self.lives, key, track, from, to));
        let (rows, more) = fill(rows, most, budget);
        let next = more.then(|| Cursor::at(self.epoch(), &keyed, start + rows.len()).to_string());
        let mut body = json!({
            "window": {"from": thousandths(from), "to": thousandths(to)},
            "length": thousandths(length),
            "beatsPerBar": timebase.beats_a_bar(from),
            "tracks": rows,
            "trackCount": keyed.len(),
            // (the place of the first track listed among them all, from 0)
            "from": start,
        });
        if let Some(next) = next {
            body["nextCursor"] = next.into();
        }
        if !first_page {
            return Ok(body);
        }
        let playhead = ui.and_then(|ui| ui["position"].as_f64()).or_else(|| session["transport"]["position"].as_f64()).unwrap_or(0.0);
        body["playhead"] = thousandths(timebase.beats(playhead)).into();
        body["playing"] = ui.is_some_and(|ui| ui["playing"] == true).into();
        let selection = &session["selection"];
        body["timeSelection"] = match (beats(&selection["start"]), beats(&selection["end"])) {
            (Some(start), Some(end)) if end > start => json!({"start": thousandths(start), "end": thousandths(end)}),
            _ => Value::Null,
        };
        if let Some((shown_from, shown_to)) = ui.and_then(|ui| shown(session, ui, &timebase)) {
            body["view"] = json!({"from": thousandths(shown_from), "to": thousandths(shown_to)});
        }
        // (a marker with an end is a region: a memory location of a selection)
        let (mut markers, mut regions, mut bytes, mut left_out) = (vec![], vec![], 0, 0);
        for marker in list(session, "markers") {
            let (Some(start), Some(key)) = (beats(&marker["start"]), now(&self.lives, Kind::Marker, marker)) else { continue };
            let end = beats(&marker["end"]).filter(|end| *end > start);
            if end.unwrap_or(start) < from || start > to {
                continue;
            }
            let list = if end.is_some() { &mut regions } else { &mut markers };
            if list.len() >= WINDOW_MARKERS || bytes >= MARKERS_BYTES {
                left_out += 1;
                continue;
            }
            let mut row = json!({"ref": Ref::Object(key).to_string(), "number": marker["number"], "name": marker["name"], "position": thousandths(start)});
            if let Some(end) = end {
                row["end"] = thousandths(end).into();
            }
            bytes += row.to_string().len() + 1;
            list.push(row);
        }
        body["markers"] = markers.into();
        body["regions"] = regions.into();
        if left_out > 0 {
            body["markersOmitted"] = left_out.into();
        }
        let meter = timebase.meter_marks(from, to).into_iter().take(WINDOW_METERS).map(|mark| {
            json!({"position": thousandths(mark.position), "bar": mark.bar, "beat": thousandths(mark.beat), "beatsPerBar": mark.beats_per_bar})
        });
        body["meter"] = meter.collect::<Vec<_>>().into();
        Ok(body)
    }

    /// A MIDI clip's notes in quarter notes from its start (`midi.notes`, whose ticks are 960 a quarter note): the clip
    /// `clip` names, else the one the MIDI Editor shows. A page from the cursor's note; SoundCraft doesn't say which notes
    /// its editor has selected.
    async fn midi_body(
        &mut self,
        session: &Value,
        clip: Option<Id>,
        cursor: Option<&Cursor>,
        most: usize,
        until: Instant,
    ) -> Result<Value, Refusal> {
        let clip = match clip {
            Some(clip) => clip,
            None => midi_editor_clip(session, &self.lives).ok_or_else(|| {
                bad("midi: pass the clip as ref (the MIDI Editor shows none: no MIDI clip or MIDI track is selected)".into())
            })?,
        };
        let named = Ref::Object(clip);
        let id = self.lives.resolve(&clip).map_err(|missing| gone(missing, &named))?;
        let (track, shown) = tracks(session)
            .iter()
            .find_map(|track| list(track, "clips").iter().find(|shown| shown["id"].as_u64() == Some(id)).map(|shown| (track, shown)))
            .ok_or_else(|| gone(Missing::Gone, &named))?;
        if shown.get("notes").is_none() {
            return Err(bad(format!("ref: {named} isn't a MIDI clip, so it has no notes")));
        }
        let timebase = Timebase::of(session).ok_or_else(no_beats)?;
        let rows = self.notes(session, &clip, until).await?;
        let start = match cursor {
            Some(cursor) => self.resume(cursor, &rows)?,
            None => 0,
        };
        let quarters = |ticks: &Value| thousandths(ticks.as_f64().unwrap_or_default() / PPQ);
        let notes = rows[start..].iter().map(|(_, row)| {
            let mut note = json!({
                "ref": row["ref"],
                "pitch": row["pitch"],
                "start": quarters(&row["start"]),
                "length": quarters(&row["length"]),
                "velocity": row["velocity"],
                "channel": row["channel"],
                "selected": false,
            });
            if let Some(tie) = row.get("tie") {
                note["tie"] = tie.clone();
            }
            note
        });
        let (notes, more) = fill(notes, most, ANSWER_BYTES - NOTES_EXTRAS_BYTES);
        let next = more.then(|| Cursor::at(self.epoch(), &rows, start + notes.len()).to_string());
        let placed = shown["start"].as_f64().unwrap_or_default();
        let begins = timebase.beats(placed);
        let mut body = json!({
            "ref": named.to_string(),
            "trackRef": now(&self.lives, Kind::Track, track).map(|track| Ref::Object(track).to_string()),
            "name": shown["name"],
            "length": thousandths(timebase.beats(placed + shown["length"].as_f64().unwrap_or_default()) - begins),
            "beatsPerBar": timebase.beats_a_bar(begins),
            "noteCount": rows.len(),
            "notes": notes,
        });
        if let Some(next) = next {
            body["nextCursor"] = next.into();
        }
        Ok(body)
    }

    /// A track's plug-ins: its instrument first, then its inserts by slot, each named as `engine.plugins` names a
    /// built-in (else by its id). The track `track` names, else the one whose plug-in window was opened last, else the
    /// first selected one.
    async fn fx_body(&mut self, session: &Value, ui: Option<&Value>, track: Option<Id>, until: Instant) -> Result<Value, Refusal> {
        let track = match track {
            Some(id) => self.track(session, &id)?,
            None => {
                let window =
                    ui.and_then(|ui| ui["ui"]["plugin_windows"].as_array()).and_then(|open| open.last()).and_then(|open| open[0].as_u64());
                let first = list(&session["selection"], "tracks").first().and_then(Value::as_u64);
                window
                    .or(first)
                    .and_then(|id| tracks(session).iter().find(|track| track["id"].as_u64() == Some(id)))
                    .ok_or_else(|| bad("fx: pass the track as ref (no plug-in window is open and no track is selected)".into()))?
            }
        };
        let key = now(&self.lives, Kind::Track, track).map(|track| Ref::Object(track).to_string());
        let _ = self.know_plugins(until).await;
        let nodes: Vec<Value> = insert_rows(track, &self.lives)
            .into_iter()
            .map(|(key, row)| {
                let mut node = json!({"ref": key, "name": self.plugin_name(&row["plugin"]), "plugin": true});
                // (an instrument has no bypass SoundCraft shows)
                if let Some(bypass) = row["bypass"].as_bool() {
                    node["enabled"] = (!bypass).into();
                }
                node
            })
            .collect();
        Ok(json!({"ref": key, "name": track["name"], "nodes": nodes}))
    }

    /// The Mix window's strips: its master faders', then a page of the other tracks it shows (all but those hidden and the
    /// video tracks), from the cursor's or centred on `around`'s. SoundCraft has no meters to read, so no levels.
    fn mixer_body(&self, session: &Value, around: Option<Id>, cursor: Option<&Cursor>, most: usize) -> Result<Value, Refusal> {
        let shown = |track: &&Value| track["hidden"] != true && track["kind"] != "video";
        let master = |track: &&Value| track["kind"] == "master";
        let keyed = self.keyed(tracks(session).iter().filter(shown).filter(|track| !master(track)));
        let start = self.page_start(session, &keyed, cursor, around, most)?;
        let selected = list(&session["selection"], "tracks");
        let strip = |key: &str, track: &Value| {
            let mut row = json!({
                "ref": key,
                "name": track["name"],
                "type": track["kind"],
                "volumeDb": track["volume_db"],
                "pan": pan(&track["pan"]),
                "mute": track["mute"],
                "solo": track["solo"],
                "selected": selected.contains(&track["id"]),
            });
            match track["kind"] == "master" {
                true => row["master"] = true.into(),
                false => row["arm"] = track["record_arm"].clone(),
            }
            row
        };
        let masters = self.keyed(tracks(session).iter().filter(shown).filter(master));
        let (mut strips, _) = fill(masters.iter().map(|(key, track)| strip(key, track)), MAX_MIXER_STRIPS, MASTERS_BYTES);
        let (page, more) = fill(keyed[start..].iter().map(|(key, track)| strip(key, track)), most, ANSWER_BYTES - MASTERS_BYTES);
        let next = more.then(|| Cursor::at(self.epoch(), &keyed, start + page.len()).to_string());
        strips.extend(page);
        let mut body = json!({"strips": strips, "trackCount": keyed.len(), "from": start, "levels": false});
        if let Some(next) = next {
            body["nextCursor"] = next.into();
        }
        Ok(body)
    }

    /// Tracks by their refs now, which a cursor names them by.
    fn keyed<'a>(&self, tracks: impl Iterator<Item = &'a Value>) -> Vec<(String, &'a Value)> {
        tracks.filter_map(|track| Some((Ref::Object(now(&self.lives, Kind::Track, track)?).to_string(), track))).collect()
    }

    /// Where a page of `most` tracks starts among `keyed`: at the cursor's track; centred on the track `around` names
    /// (half the page above it, within the first track and the last page; from the first when it isn't among them); else
    /// at the first.
    fn page_start(
        &self,
        session: &Value,
        keyed: &[(String, &Value)],
        cursor: Option<&Cursor>,
        around: Option<Id>,
        most: usize,
    ) -> Result<usize, Refusal> {
        match (cursor, around) {
            (Some(cursor), _) => self.resume(cursor, keyed),
            (None, Some(around)) => {
                let track = self.track(session, &around)?;
                let place = keyed.iter().position(|(_, listed)| std::ptr::eq(*listed, track)).unwrap_or(0);
                Ok(place.saturating_sub(most / 2).min(keyed.len().saturating_sub(most)))
            }
            (None, None) => Ok(0),
        }
    }
}

/// A track's ref, where only a track's is taken.
fn track_ref(key: &str, named: Option<Ref>) -> Result<Option<Id>, Refusal> {
    match named {
        None => Ok(None),
        Some(Ref::Object(track @ Id { kind: Kind::Track, .. })) => Ok(Some(track)),
        Some(other) => Err(bad(format!("{key}: a track (s:track:…), not {other}"))),
    }
}

/// The stretch of the timeline asked for, in quarter notes.
fn window(params: &Value) -> Result<Option<(f64, f64)>, Refusal> {
    let Some(window) = params.get("window").filter(|window| !window.is_null()) else { return Ok(None) };
    let number = |key: &str| window.get(key).and_then(Value::as_f64).filter(|number| number.is_finite());
    match (number("from"), number("to")) {
        (Some(from), Some(to)) if to > from && to - from <= MAX_WINDOW => Ok(Some((from, to))),
        (Some(_), Some(_)) => Err(bad(format!("window: to must be after from, and no more than {MAX_WINDOW} apart"))),
        _ => Err(bad("window: {from, to} as numbers (quarter notes)".into())),
    }
}

/// What a body in beats gets from a session whose tempo or meter map has no beats the bridge can count.
fn no_beats() -> Refusal {
    (
        code::FAILED,
        "SoundCraft's tempo or meter map holds more than the steps SoundCraft 0.17.0 makes, so the bridge can't count its beats: \
         this view isn't drawn (read its clips for their places in samples and seconds)"
            .into(),
    )
}

/// The stretch of the timeline SoundCraft's Edit window shows: from its scroll, its timeline's width (`ui.inspect`'s
/// `edit_layout.timeline`, `[left, top, right, bottom]`) in the session's samples a pixel.
fn shown(session: &Value, ui: &Value, timebase: &Timebase) -> Option<(f64, f64)> {
    let timeline = ui["edit_layout"]["timeline"].as_array()?;
    let width = timeline.get(2)?.as_f64()? - timeline.first()?.as_f64()?;
    let zoom = &session["zoom"];
    let (per_pixel, scroll) = (zoom["samples_per_px"].as_f64().filter(|per| *per > 0.0)?, zoom["scroll"].as_f64()?);
    (width > 0.0).then(|| (timebase.beats(scroll), timebase.beats(scroll + width * per_pixel)))
}

/// A track in the Edit window: the clips that touch `from` to `to` (quarter notes), each where it starts and how long it
/// is in quarter notes; up to a track's share of the page, the rest counted.
fn edit_track(session: &Value, timebase: &Timebase, lives: &Lives, key: &str, track: &Value, from: f64, to: f64) -> Value {
    let selected = |key: &str, id: &Value| list(&session["selection"], key).contains(id);
    let (mut items, mut bytes, mut omitted) = (vec![], 0, 0);
    for clip in list(track, "clips") {
        let (Some(start), Some(length), Some(id)) = (clip["start"].as_f64(), clip["length"].as_f64(), now(lives, Kind::Clip, clip)) else {
            continue;
        };
        let (position, end) = (timebase.beats(start), timebase.beats(start + length));
        if end <= from || position >= to {
            continue;
        }
        if items.len() >= ITEMS_PER_TRACK || bytes >= ITEMS_BYTES_PER_TRACK {
            omitted += 1;
            continue;
        }
        let item = json!({
            "ref": Ref::Object(id).to_string(),
            "position": thousandths(position),
            "length": thousandths(end - position),
            "name": clip["name"],
            "muted": clip["muted"],
            "selected": selected("clips", &clip["id"]),
        });
        bytes += item.to_string().len() + 1;
        items.push(item);
    }
    let mut row = json!({
        "ref": key,
        "name": track["name"],
        "type": track["kind"],
        // (SoundCraft's folders don't nest tracks in the Edit window: every track is at the top)
        "depth": 0,
        "shown": track["hidden"] != true,
        "selected": selected("tracks", &track["id"]),
        "items": items,
    });
    if omitted > 0 {
        row["itemsOmitted"] = omitted.into();
    }
    row
}

/// The clip the MIDI Editor shows, by SoundCraft's own rule (`target_clip` in its MIDI Editor).
fn midi_editor_clip(session: &Value, lives: &Lives) -> Option<Id> {
    let selection = &session["selection"];
    let clips = || tracks(session).iter().flat_map(|track| list(track, "clips"));
    let midi =
        list(selection, "clips").iter().find_map(|id| clips().find(|clip| clip["id"] == *id).filter(|clip| clip.get("notes").is_some()));
    if let Some(clip) = midi {
        return now(lives, Kind::Clip, clip);
    }
    let selected = list(selection, "tracks");
    let on_midi_tracks = || {
        tracks(session)
            .iter()
            .filter(|track| selected.contains(&track["id"]) && matches!(track["kind"].as_str(), Some("midi" | "instrument")))
            .flat_map(|track| list(track, "clips"))
    };
    let at = |key: &str| selection[key].as_i64().unwrap_or_default();
    let (start, end) = (at("start"), at("end"));
    let span = |clip: &Value| {
        let begins = clip["start"].as_i64().unwrap_or_default();
        (begins, begins + clip["length"].as_i64().unwrap_or_default())
    };
    let under = on_midi_tracks().find(|clip| {
        let (begins, ends) = span(clip);
        // (the insertion point in it, or the selection overlapping it)
        start < ends && (begins <= start || begins < end)
    });
    now(lives, Kind::Clip, under.or_else(|| on_midi_tracks().next())?)
}

/// A strip's pan as one number: a mono track's, or the middle of a stereo track's two.
fn pan(pan: &Value) -> Value {
    let sides: Vec<f64> = match pan {
        Value::Array(sides) => sides.iter().filter_map(Value::as_f64).collect(),
        other => other.as_f64().into_iter().collect(),
    };
    match sides.is_empty() {
        true => Value::Null,
        false => ((sides.iter().sum::<f64>() / sides.len() as f64 * 10_000.0).round() / 10_000.0).into(),
    }
}
