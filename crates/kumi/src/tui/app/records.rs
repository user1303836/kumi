//! HISTORY's records: Kumi's changes and what it kept, with a revision every change to them bumps. HISTORY lays its
//! rows out again only when the revision moves, so nothing changes them except through these methods.
use super::Kept;
use kumi_types::contracts::{ChangeRecord, ChangeState};
use kumi_types::picture::across;
use std::{cell::RefCell, rc::Rc};

#[derive(Default)]
pub(super) struct Records {
    changes: Vec<ChangeRecord>,
    kept: Vec<Rc<RefCell<Kept>>>,
    revision: u64,
}

impl Records {
    pub(super) fn changes(&self) -> &[ChangeRecord] {
        &self.changes
    }
    pub(super) fn kept(&self) -> &[Rc<RefCell<Kept>>] {
        &self.kept
    }
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }
    /// The changes, to change in any way.
    pub(super) fn changes_mut(&mut self) -> &mut Vec<ChangeRecord> {
        self.revision += 1;
        &mut self.changes
    }
    /// What's kept, to change in any way.
    pub(super) fn kept_mut(&mut self) -> &mut Vec<Rc<RefCell<Kept>>> {
        self.revision += 1;
        &mut self.kept
    }
    /// A kept entry forgotten, through its own cell.
    pub(super) fn set_forgotten(&mut self, entry: &RefCell<Kept>) {
        self.revision += 1;
        entry.borrow_mut().forgotten = true;
    }
}

/// One of HISTORY's entries: a change, or a change a plan made alike across several tracks (`runs`).
pub(super) enum Shown<'a> {
    One(&'a ChangeRecord),
    /// The run's changes, oldest first, and the one record HISTORY shows for them (`run_record`).
    Run(Vec<&'a ChangeRecord>, ChangeRecord),
}

/// HISTORY's entries, newest first. Changes one plan made one after another, of one kind (`ChangeRecord::batch`), each
/// on a track of its own and all standing alike (applied, undone…), are one entry: "mute the drums, the bass and the
/// keys" is one change across three tracks.
pub(super) fn runs(changes: &[ChangeRecord]) -> Vec<Shown<'_>> {
    let mut shown = Vec::new();
    let mut at = 0;
    while at < changes.len() {
        let first = &changes[at];
        let mut end = at + 1;
        if first.batch.is_some() {
            while end < changes.len() && changes[end].batch == first.batch && changes[end].state == first.state {
                end += 1;
            }
        }
        let run: Vec<&ChangeRecord> = changes[at..end].iter().collect();
        let names: Vec<&str> = run.iter().filter_map(|change| change.track.as_ref()).map(|track| track.name.as_str()).collect();
        let apart = names.len() == run.len() && names.iter().all(|name| !name.is_empty()) && {
            let mut seen = std::collections::HashSet::new();
            names.iter().all(|name| seen.insert(*name))
        };
        if run.len() > 1 && apart {
            let record = run_record(&run);
            shown.push(Shown::Run(run, record));
        } else {
            shown.extend(run.into_iter().map(Shown::One));
        }
        at = end;
    }
    shown.reverse();
    shown
}

/// The one record HISTORY shows for a run (oldest first): its words name the tracks, and its picture is each track's
/// values that moved, when each change's picture is values or a route.
pub(super) fn run_record(run: &[&ChangeRecord]) -> ChangeRecord {
    let (first, last) = (run[0], run[run.len() - 1]);
    let parts: Option<Vec<(String, kumi_types::contracts::ChangePicture)>> =
        run.iter().map(|change| Some((change.track.as_ref()?.name.clone(), change.picture.clone()?))).collect();
    ChangeRecord {
        id: format!("run:{}", first.id),
        title: run_title(run),
        track: None,
        from: None,
        to: None,
        range: None,
        colors: None,
        clip: None,
        devices: None,
        score: None,
        note: run.iter().find_map(|change| change.note.clone()),
        at: last.at,
        picture: parts.and_then(|parts| across(&parts)),
        ..first.clone()
    }
}

/// A run's words: the tracks' names with what each change did when it did the same ("Drums, Bass and Keys muted"),
/// else how many changes ("Drums and Bass: 2 changes").
fn run_title(run: &[&ChangeRecord]) -> String {
    let names: Vec<&str> = run.iter().filter_map(|change| change.track.as_ref()).map(|track| track.name.as_str()).collect();
    let listed = match names.as_slice() {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] if rest.len() <= 2 => format!("{} and {last}", rest.join(", ")),
        _ => format!("{} and {} more", names[..2].join(", "), names.len() - 2),
    };
    // What each said after its track's name: the same for each, said once.
    let rests: Vec<Option<&str>> =
        run.iter().map(|change| change.track.as_ref().and_then(|track| change.title.strip_prefix(track.name.as_str()))).collect();
    match rests.first() {
        Some(Some(rest)) if !rest.trim().is_empty() && rests.iter().all(|other| other == &Some(*rest)) => format!("{listed}{rest}"),
        _ => format!("{listed}: {} changes", run.len()),
    }
}

/// Whether a run can be undone: each of its changes still can.
pub(super) fn run_undoable(run: &[&ChangeRecord]) -> bool {
    run.iter().all(|change| change.state == ChangeState::Applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_types::contracts::{ChangeFamily, ChangePicture, TrackChip};
    use kumi_types::picture::{MixLevels, PictureValue};

    fn change(id: &str, track: &str, title: &str, batch: Option<&str>) -> ChangeRecord {
        let levels = |mute: bool| MixLevels { mute: Some(mute), ..Default::default() };
        ChangeRecord {
            id: id.into(),
            family: ChangeFamily::Mixer,
            title: title.into(),
            track: Some(TrackChip { name: track.into(), color: None }),
            from: None,
            to: None,
            range: None,
            colors: None,
            clip: None,
            devices: None,
            state: ChangeState::Applied,
            score: None,
            note: None,
            at: 1,
            picture: Some(ChangePicture::Mix { before: levels(false), after: levels(true) }),
            batch: batch.map(str::to_owned),
        }
    }

    fn shown(changes: &[ChangeRecord]) -> Vec<String> {
        runs(changes)
            .iter()
            .map(|shown| match shown {
                Shown::One(change) => change.title.clone(),
                Shown::Run(run, record) => format!("{} [{}]", record.title, run.len()),
            })
            .collect()
    }

    #[test]
    fn a_plans_change_across_several_tracks_is_one_entry_with_one_picture() {
        let changes = [
            change("c1", "Drums", "Drums muted", Some("p1:set_mixer")),
            change("c2", "Bass", "Bass muted", Some("p1:set_mixer")),
            change("c3", "Keys", "Keys muted", Some("p1:set_mixer")),
            change("c4", "Keys", "Keys volume 0 dB → -3 dB", Some("p2:set_mixer")),
        ];
        assert_eq!(shown(&changes), ["Keys volume 0 dB → -3 dB", "Drums, Bass and Keys muted [3]"], "newest first");
        let Shown::Run(run, record) = &runs(&changes)[1] else { panic!() };
        assert_eq!(record.id, "run:c1");
        assert!(run_undoable(run));
        let Some(ChangePicture::Values { values }) = &record.picture else { panic!("{:?}", record.picture) };
        assert_eq!(values.iter().map(|value| value.name.as_str()).collect::<Vec<_>>(), ["Drums · Mute", "Bass · Mute", "Keys · Mute"]);
        assert_eq!(
            values[0],
            PictureValue { name: "Drums · Mute".into(), from_text: Some("off".into()), to_text: Some("on".into()), ..Default::default() }
        );
    }

    #[test]
    fn changes_on_one_track_or_standing_apart_stay_entries_of_their_own() {
        // Two changes on one track aren't a change across tracks; nor are changes no plan made, or one undone since.
        let same =
            [change("c1", "Drums", "Drums muted", Some("p1:set_mixer")), change("c2", "Drums", "Drums soloed", Some("p1:set_mixer"))];
        assert_eq!(shown(&same), ["Drums soloed", "Drums muted"]);
        let unplanned = [change("c1", "Drums", "Drums muted", None), change("c2", "Bass", "Bass muted", None)];
        assert_eq!(shown(&unplanned), ["Bass muted", "Drums muted"]);
        let mut undone = change("c2", "Bass", "Bass muted", Some("p1:set_mixer"));
        undone.state = ChangeState::Undone;
        let apart =
            [change("c1", "Drums", "Drums muted", Some("p1:set_mixer")), undone, change("c3", "Keys", "Keys muted", Some("p1:set_mixer"))];
        assert_eq!(shown(&apart), ["Keys muted", "Bass muted", "Drums muted"]);
    }

    #[test]
    fn a_run_says_its_tracks_and_what_each_change_did() {
        let different = [
            change("c1", "Drums", "Drums volume 0 dB → -3 dB", Some("p1:set_mixer")),
            change("c2", "Bass", "Bass volume 0 dB → -6 dB", Some("p1:set_mixer")),
        ];
        assert_eq!(shown(&different), ["Drums and Bass: 2 changes"]);
        let many: Vec<ChangeRecord> = ["Kick", "Snare", "Hats", "Perc", "Toms"]
            .iter()
            .enumerate()
            .map(|(n, name)| change(&format!("c{n}"), name, &format!("{name}: armed"), Some("p1:set_routing")))
            .collect();
        assert_eq!(shown(&many), ["Kick, Snare and 3 more: armed [5]"]);
        // Without a picture of values for each, the run has none.
        let mut colored = many.clone();
        colored[1].picture = None;
        let Shown::Run(_, record) = &runs(&colored)[0] else { panic!() };
        assert_eq!(record.picture, None);
    }
}
