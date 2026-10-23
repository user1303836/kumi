//! `read` and `snapshot`: the project as rows, paged, in the shapes the host's simulator answers with
//! (`crates/daw-mcp-server/src/simulator/logic.rs`): `items`, `total`, `epoch` in every answer, `nextCursor` when
//! there is more; `ref` alone reads one row; `parent` filters a kind under its parent; `note` needs its region; the
//! codes `bad-params`, `failed` and `stale` with the simulator's wording. The rows themselves come from a [`Catalog`],
//! what the readers gathered of the project in the current epoch.

use serde_json::{json, Value};

use crate::refs::{self, bad, check_epoch, Fault, Ref};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;
pub const KINDS: [&str; 12] =
    ["project", "track", "region", "note", "plugin", "send", "marker", "tempo", "param", "smart", "automation", "recent"];
/// The kinds read only under a parent, and what the parent is: a region's notes, a plug-in's parameters (from its
/// window's Controls View), a track's Smart Controls and automation (from their panes).
const NEED_PARENT: [(&str, &str); 4] = [("note", "region"), ("param", "plug-in"), ("smart", "track"), ("automation", "track")];

/// What the readers gathered of the project, in one epoch: the rows of every kind, in the order they are numbered.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub epoch: u32,
    pub project: Option<Value>,
    pub tracks: Vec<Value>,
    pub regions: Vec<Value>,
    /// The notes of each region read so far, by the region's number.
    pub notes: Vec<(usize, Vec<Value>)>,
    pub plugins: Vec<Value>,
    pub sends: Vec<Value>,
    pub markers: Vec<Value>,
    pub tempo: Option<Value>,
    /// The selected track's and region's refs.
    pub selection: Value,
    /// The transport as the snapshot carries it.
    pub transport: Value,
}

/// The key a kind's rows name their parent by.
pub fn parent_key(kind: &str) -> Option<&'static str> {
    match kind {
        "region" | "plugin" | "send" | "smart" | "automation" => Some("trackRef"),
        "note" => Some("regionRef"),
        "param" => Some("pluginRef"),
        _ => None,
    }
}

impl Catalog {
    pub fn epoch_text(&self) -> String {
        self.epoch.to_string()
    }

    /// Every row of a kind.
    pub fn rows(&self, kind: &str) -> Vec<Value> {
        match kind {
            "project" => self.project.iter().cloned().collect(),
            "track" => self.tracks.clone(),
            "region" => self.regions.clone(),
            "note" => self.notes.iter().flat_map(|(_, notes)| notes.iter().cloned()).collect(),
            "plugin" => self.plugins.clone(),
            "send" => self.sends.clone(),
            "marker" => self.markers.clone(),
            "tempo" => self.tempo.iter().cloned().collect(),
            _ => vec![],
        }
    }

    /// The row a ref names, of any kind.
    pub fn row(&self, reference: &str) -> Option<Value> {
        KINDS.iter().flat_map(|kind| self.rows(kind)).find(|row| row["ref"] == reference)
    }

    /// The notes of a region, if they were read.
    pub fn notes_of(&self, region: usize) -> Option<&Vec<Value>> {
        self.notes.iter().find(|(n, _)| *n == region).map(|(_, notes)| notes)
    }

    /// The rows of a kind under a parent ref.
    pub fn under(&self, kind: &str, parent: &str) -> Vec<Value> {
        let Some(key) = parent_key(kind) else { return self.rows(kind) };
        self.rows(kind).into_iter().filter(|row| row[key] == parent).collect()
    }
}

/// The page size a request asks for, clamped as the simulator clamps it.
pub fn limit_of(params: &Value, most: usize) -> usize {
    params.get("limit").and_then(Value::as_u64).map_or(DEFAULT_LIMIT.min(most), |n| (n as usize).clamp(1, most))
}

/// Where a page starts: the position a cursor (`<epoch>.<n>`, this epoch's) names, or 0.
pub fn start_of(params: &Value) -> Result<usize, Fault> {
    match params.get("cursor").and_then(Value::as_str) {
        None => Ok(0),
        Some(cursor) => refs::parse_cursor(cursor)
            .map(|(_, start)| start)
            .ok_or_else(|| bad("cursor: pass the nextCursor an earlier read gave, unchanged")),
    }
}

/// A page of rows: `items`, `total`, `epoch`, and `nextCursor` when there is more.
pub fn page(all: &[Value], params: &Value, epoch: u32) -> Result<Value, Fault> {
    let (limit, start) = (limit_of(params, MAX_LIMIT), start_of(params)?);
    let items: Vec<Value> = all.iter().skip(start).take(limit).cloned().collect();
    let mut page = json!({"items": items, "total": all.len(), "epoch": epoch.to_string()});
    if start + limit < all.len() {
        page["nextCursor"] = refs::cursor(epoch, start + limit).into();
    }
    Ok(page)
}

/// What a `read` asks for, checked: the kind and, for notes, the region it wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// One row by its ref.
    One(String),
    /// A kind's rows, under a parent when one is named (the region's number for notes).
    Kind { kind: String, parent: Option<Ref> },
}

/// Reads the request: the simulator's refusals, in its words.
pub fn want(params: &Value, epoch: u32) -> Result<Want, Fault> {
    check_epoch(params, epoch)?;
    if let Some(reference) = params.get("ref").and_then(Value::as_str) {
        return Ok(Want::One(reference.to_string()));
    }
    let kind = params.get("kind").and_then(Value::as_str).ok_or_else(|| bad("kind: one of the kinds"))?;
    if !KINDS.contains(&kind) {
        return Err(bad(format!("Logic Pro has no kind {kind:?} to read")));
    }
    let parent = params.get("parent").and_then(Value::as_str);
    if let Some((_, what)) = NEED_PARENT.iter().find(|(needs, _)| *needs == kind).filter(|_| parent.is_none()) {
        return Err(bad(format!("{kind}: pass the parent {what}")));
    }
    let parent = match (parent_key(kind), parent) {
        (Some(_), Some(parent)) => Ref::parse(parent),
        _ => None,
    };
    Ok(Want::Kind { kind: kind.into(), parent })
}

/// Answers a `read` from the catalog. The notes of a region not read yet are the caller's to gather first (see
/// [`want`] and [`Catalog::notes_of`]).
pub fn read(catalog: &Catalog, params: &Value) -> Result<Value, Fault> {
    match want(params, catalog.epoch)? {
        Want::One(reference) => match catalog.row(&reference) {
            Some(row) => Ok(json!({"items": [row], "epoch": catalog.epoch_text()})),
            None => Err(refs::not_in_project(&reference)),
        },
        Want::Kind { kind, parent } => {
            let all = match (parent_key(&kind), params.get("parent").and_then(Value::as_str)) {
                (Some(_), Some(parent_text)) => {
                    let _ = parent;
                    catalog.under(&kind, parent_text)
                }
                _ => catalog.rows(&kind),
            };
            page(&all, params, catalog.epoch)
        }
    }
}

/// The kinds a bulk read answers: the rows that need no parent and no window of their own.
pub const BULK_KINDS: [&str; 7] = ["project", "track", "region", "plugin", "send", "marker", "tempo"];
/// The most rows of a kind a bulk read answers: past them the kind is named under `truncated`, to page with `read`.
pub const BULK_MOST: usize = 10000;

/// The kinds a bulk read asks for (`rows`), checked: some of [`BULK_KINDS`], each once, in the order asked.
pub fn bulk_kinds(rows: &Value) -> Result<Vec<&'static str>, Fault> {
    let asked = rows
        .as_array()
        .filter(|asked| !asked.is_empty())
        .ok_or_else(|| bad(format!("rows: a list of the kinds to read whole, of {}", BULK_KINDS.join(", "))))?;
    let mut kinds: Vec<&'static str> = vec![];
    for kind in asked {
        let name = kind.as_str().unwrap_or_default();
        let Some(known) = BULK_KINDS.iter().find(|known| **known == name) else {
            return Err(bad(format!(
                "rows: Logic Pro's bulk read takes {}; {} is read with read{}",
                BULK_KINDS.join(", "),
                if name.is_empty() { kind.to_string() } else { name.to_string() },
                if NEED_PARENT.iter().any(|(needs, _)| *needs == name) { ", under its parent" } else { "" }
            )));
        };
        if !kinds.contains(known) {
            kinds.push(known);
        }
    }
    Ok(kinds)
}

/// The bulk read, `snapshot {rows}`: every row of each kind asked, as `read` pages them (the same rows, in the same
/// order, from one catalog of one epoch), with each kind's `totals` and the kinds cut at `limit` (at most
/// [`BULK_MOST`]) under `truncated`.
pub fn bulk(catalog: &Catalog, kinds: &[&str], params: &Value) -> Value {
    let most = params.get("limit").and_then(Value::as_u64).map_or(BULK_MOST, |n| (n as usize).clamp(1, BULK_MOST));
    let (mut rows, mut totals, mut truncated) = (serde_json::Map::new(), serde_json::Map::new(), vec![]);
    for kind in kinds {
        let all = catalog.rows(kind);
        totals.insert(kind.to_string(), all.len().into());
        if all.len() > most {
            truncated.push(Value::from(*kind));
        }
        rows.insert(kind.to_string(), Value::Array(all.into_iter().take(most).collect()));
    }
    let mut answer = json!({"epoch": catalog.epoch_text(), "rows": rows, "totals": totals});
    if !truncated.is_empty() {
        answer["truncated"] = Value::Array(truncated);
    }
    answer
}

/// `snapshot`: the project, the transport, the tracks, the selection and the markers, in one answer.
pub fn snapshot(catalog: &Catalog) -> Value {
    json!({
        "epoch": catalog.epoch_text(),
        "project": catalog.project.clone().unwrap_or(Value::Null),
        "transport": catalog.transport,
        "tracks": catalog.tracks,
        "selection": catalog.selection,
        "markers": catalog.markers,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn position(bar: u32, beat: u32, division: u32, tick: u32) -> Value {
        json!({"bar": bar, "beat": beat, "division": division, "tick": tick})
    }

    /// The simulator's Scratch, as a catalog: what the real bridge must answer the same.
    pub fn scratch(epoch: u32) -> Catalog {
        let r = |kind: &str, n: &str| format!("{epoch}:{kind}:{n}");
        Catalog {
            epoch,
            project: Some(json!({"id": "3f9c2a7b1d4e5f60718293a4b5c6d7e8", "identity": "Scratch", "path": "/lab/Scratch.logicx", "tempo": 160.0, "timeSignature": "4/4", "keySignature": "C major", "sampleRate": 48000, "trackCount": 2, "middleC": 3})),
            tracks: vec![
                json!({"ref": r("track", "1"), "index": 1, "name": "Vocal", "kind": "audio", "mute": false, "solo": false, "armed": false, "monitoring": false, "volumeDb": 0.0, "pan": 0, "color": "#5b8def", "output": "Stereo Out", "inputs": ["Input 1"], "selected": true}),
                json!({"ref": r("track", "2"), "index": 2, "name": "Keys", "kind": "instrument", "mute": false, "solo": false, "armed": false, "monitoring": false, "volumeDb": -3.0, "pan": 0, "color": null, "output": "Stereo Out", "inputs": [], "selected": false}),
            ],
            regions: vec![json!({"ref": r("region", "1"), "trackRef": r("track", "2"), "name": "Keys", "startBeats": 0.0, "endBeats": 16.0, "kind": "midi", "selected": true, "looped": false})],
            notes: vec![(1, [(1, 60, 100, 240), (2, 64, 96, 240), (3, 67, 92, 240), (4, 72, 110, 480)].iter().map(|(beat, pitch, velocity, length)| json!({"ref": r("note", &format!("1.{beat}")), "regionRef": r("region", "1"), "position": position(1, *beat, 1, 1), "pitch": pitch, "velocity": velocity, "lengthTicks": length, "channel": 1})).collect())],
            plugins: vec![json!({"ref": r("plugin", "1.1"), "trackRef": r("track", "1"), "slot": 1, "name": "Channel EQ", "bypassed": false, "kind": "effect"})],
            sends: vec![json!({"ref": r("send", "1.1"), "trackRef": r("track", "1"), "slot": 1, "destination": "Bus 1", "level": -6.0})],
            markers: vec![json!({"ref": r("marker", "1"), "position": position(9, 1, 1, 1), "name": "Chorus", "length": 8.0, "text": ""})],
            tempo: Some(json!({"bpm": 160.0, "tempos": [{"position": position(1, 1, 1, 1), "bpm": 160.0}], "signatures": [{"position": position(1, 1, 1, 1), "time": "4/4", "key": "C major"}]})),
            selection: json!({"trackRef": r("track", "1"), "regionRef": r("region", "1")}),
            transport: json!({"playing": false, "recording": false, "cycle": false, "playhead": {"bar": 1, "beat": 1}, "tempo": 160.0, "timeSignature": "4/4"}),
        }
    }

    #[test]
    fn reads_page_filter_and_refuse_as_the_simulator_does() {
        let catalog = scratch(1);
        let tracks = read(&catalog, &json!({"kind": "track"})).unwrap();
        assert_eq!(
            (tracks["items"].as_array().unwrap().len(), &tracks["total"], &tracks["epoch"], tracks.get("nextCursor")),
            (2, &json!(2), &json!("1"), None)
        );
        let first = read(&catalog, &json!({"kind": "track", "limit": 1})).unwrap();
        assert_eq!((first["items"][0]["name"].as_str(), first["nextCursor"].as_str()), (Some("Vocal"), Some("1.1")));
        let second = read(&catalog, &json!({"kind": "track", "cursor": "1.1"})).unwrap();
        assert_eq!((second["items"][0]["name"].as_str(), second.get("nextCursor")), (Some("Keys"), None));
        let regions = read(&catalog, &json!({"kind": "region", "parent": "1:track:2"})).unwrap();
        assert_eq!(regions["total"], 1);
        assert_eq!(read(&catalog, &json!({"kind": "region", "parent": "1:track:1"})).unwrap()["total"], 0);
        assert_eq!(read(&catalog, &json!({"kind": "note"})), Err(bad("note: pass the parent region")));
        let notes = read(&catalog, &json!({"kind": "note", "parent": "1:region:1"})).unwrap();
        assert_eq!(notes["items"][3]["lengthTicks"], 480);
        assert_eq!(read(&catalog, &json!({"kind": "plugin", "parent": "1:track:1"})).unwrap()["items"][0]["name"], "Channel EQ");
        assert_eq!(read(&catalog, &json!({"kind": "send", "parent": "1:track:1"})).unwrap()["items"][0]["level"], -6.0);
        assert_eq!(read(&catalog, &json!({"kind": "marker"})).unwrap()["items"][0]["name"], "Chorus");
        assert_eq!(read(&catalog, &json!({"kind": "tempo"})).unwrap()["items"][0]["bpm"], 160.0);
        let one = read(&catalog, &json!({"ref": "1:track:2"})).unwrap();
        assert_eq!(one, json!({"items": [catalog.tracks[1]], "epoch": "1"}));
        assert_eq!(
            read(&catalog, &json!({"ref": "1:track:9"})),
            Err(("failed".into(), "1:track:9 isn't in the project (deleted, or another project)".into()))
        );
        assert_eq!(read(&catalog, &json!({"kind": "bogus"})), Err(bad("Logic Pro has no kind \"bogus\" to read")));
        assert_eq!(
            read(&catalog, &json!({"kind": "region", "parent": "0:track:2"})),
            Err(("stale".into(), "that ref is from an earlier epoch (0; now 1): read again".into()))
        );
        assert_eq!(read(&catalog, &json!({})), Err(bad("kind: one of the kinds")));
        assert_eq!(
            read(&catalog, &json!({"kind": "track", "cursor": "x"})),
            Err(bad("cursor: pass the nextCursor an earlier read gave, unchanged"))
        );
        assert_eq!(limit_of(&json!({"limit": 1000}), MAX_LIMIT), MAX_LIMIT);
        assert_eq!(limit_of(&json!({"limit": 0}), MAX_LIMIT), 1);
        assert_eq!(limit_of(&json!({}), MAX_LIMIT), DEFAULT_LIMIT);
        let snapshot = snapshot(&catalog);
        assert_eq!(snapshot["tracks"].as_array().unwrap().len(), 2);
        assert_eq!(snapshot["selection"]["regionRef"], "1:region:1");
        assert_eq!(snapshot["markers"][0]["name"], "Chorus");
        assert_eq!(
            snapshot.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["epoch", "project", "transport", "tracks", "selection", "markers"]
        );
        // The bulk read: every row of each kind asked, as the pages give them, from one catalog.
        let kinds = bulk_kinds(&json!(["project", "track", "region", "plugin", "marker", "tempo", "track"])).unwrap();
        assert_eq!(kinds, ["project", "track", "region", "plugin", "marker", "tempo"]);
        let all = bulk(&catalog, &kinds, &json!({}));
        assert_eq!(all["epoch"], "1");
        for kind in &kinds {
            let paged = read(&catalog, &json!({"kind": kind, "limit": 200})).unwrap();
            assert_eq!(all["rows"][*kind], paged["items"], "{kind}");
            assert_eq!(all["totals"][*kind], paged["total"], "{kind}");
        }
        assert!(all.get("truncated").is_none());
        let cut = bulk(&catalog, &["track"], &json!({"limit": 1}));
        assert_eq!((cut["rows"]["track"].as_array().unwrap().len(), &cut["truncated"]), (1, &json!(["track"])));
        assert_eq!(bulk_kinds(&json!(["note"])).unwrap_err().1, "rows: Logic Pro's bulk read takes project, track, region, plugin, send, marker, tempo; note is read with read, under its parent");
        assert_eq!(bulk_kinds(&json!([])).unwrap_err().0, "bad-params");
        assert_eq!(
            want(&json!({"kind": "note", "parent": "1:region:1"}), 1),
            Ok(Want::Kind { kind: "note".into(), parent: Some(Ref::new(1, "region", &[1])) })
        );
    }
}
