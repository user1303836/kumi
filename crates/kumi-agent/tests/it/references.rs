//! `@` references: what the producer can name in a message, ranked as they type, and the library's references in the
//! turn's context.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use kumi_agent::core::contracts::{PinKind, PinnedNode, Reference};
use kumi_agent::core::references::{with_references, Candidate, CandidateKind, Listed, Referable};
use serde_json::{json, Value};

fn track(name: &str) -> Candidate {
    Candidate {
        label: name.into(),
        kind: CandidateKind::Track,
        detail: None,
        reference: Reference::Daw {
            node: PinnedNode {
                track_ref: format!("1:track:{name}"),
                r#ref: format!("1:track:{name}"),
                node: PinKind::Track,
                name: name.into(),
                trail: vec![],
                siblings: vec![],
                track: Some(name.into()),
                live: None,
                time: None,
                layout: None,
            },
        },
    }
}

/// A library's sounds by name, as its index lists them, counting the candidates it's asked to make.
struct Sounds {
    names: Vec<String>,
    lower: Vec<String>,
    made: Cell<usize>,
}

impl Sounds {
    fn new(names: Vec<String>) -> Rc<Sounds> {
        let lower = names.iter().map(|name| name.to_lowercase()).collect();
        Rc::new(Sounds { names, lower, made: Cell::new(0) })
    }
}

impl Listed for Sounds {
    fn count(&self) -> usize {
        self.names.len()
    }
    fn lower(&self, at: usize) -> &str {
        &self.lower[at]
    }
    fn candidate(&self, at: usize) -> Option<Candidate> {
        self.made.set(self.made.get() + 1);
        let name = self.names.get(at)?.clone();
        Some(Candidate {
            label: name.clone(),
            kind: CandidateKind::Sample,
            detail: None,
            reference: Reference::Sample { path: format!("/Samples/{name}.wav"), name },
        })
    }
}

fn named(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn labels(found: &[Candidate]) -> Vec<(&str, CandidateKind)> {
    found.iter().map(|candidate| (candidate.label.as_str(), candidate.kind)).collect()
}

fn drums() -> Referable {
    let sounds = Sounds::new(named(&["Drum Loop 01", "Hydrant Drums", "dark rumble", "Drums"]));
    Referable::new(vec![track("Drums"), track("Bass"), track("Synth Drums"), track("Drive")], Some(sounds))
}

#[test]
fn a_prefix_beats_a_word_start_which_beats_letters_apart_and_the_projects_come_first() {
    use CandidateKind::{Sample, Track};
    assert_eq!(
        labels(&drums().rank("DRU", 50)),
        [
            // A prefix of the whole name, the project's before the library's, then the longer name.
            ("Drums", Track),
            ("Drums", Sample),
            ("Drum Loop 01", Sample),
            // A word's start, matched there even after letters an earlier word has, then letters apart.
            ("Synth Drums", Track),
            ("Hydrant Drums", Sample),
            ("dark rumble", Sample),
        ]
    );
    // `dr` names Hydra Drums by its second word, before Adrenaline's letters in a row.
    let mut both = Referable::new(vec![track("Adrenaline"), track("Hydra Drums")], None);
    assert_eq!(labels(&both.rank("dr", 50)), [("Hydra Drums", CandidateKind::Track), ("Adrenaline", CandidateKind::Track)]);
    // Letters out of order name nothing.
    assert!(drums().rank("urd", 50).is_empty());
}

#[test]
fn one_letter_is_ranked_from_the_names_letters_as_a_whole_read_would_rank_it() {
    let sounds = Sounds::new(named(&["Adrenaline", "Kick", "Hydra Drums", "Drums", "Odd 9", "9 Drums"]));
    let mut referable = Referable::new(vec![], Some(sounds));
    // The name's start, then a word's, then anywhere; a name without it isn't there.
    assert_eq!(
        labels(&referable.rank("d", 50)).iter().map(|(label, _)| *label).collect::<Vec<_>>(),
        ["Drums", "9 Drums", "Hydra Drums", "Odd 9", "Adrenaline"]
    );
    assert_eq!(labels(&referable.rank("9", 50)).iter().map(|(label, _)| *label).collect::<Vec<_>>(), ["9 Drums", "Odd 9"]);
    // A letter beyond a to z and 0 to 9 is matched by reading the names.
    let mut accents = Referable::new(vec![track("Élan"), track("Rêve")], None);
    assert_eq!(labels(&accents.rank("é", 50)), [("Élan", CandidateKind::Track)]);
}

#[test]
fn an_empty_query_keeps_the_lists_order_with_the_projects_first() {
    let found = drums().rank("", 5);
    assert_eq!(
        labels(&found),
        [
            ("Drums", CandidateKind::Track),
            ("Bass", CandidateKind::Track),
            ("Synth Drums", CandidateKind::Track),
            ("Drive", CandidateKind::Track),
            ("Drum Loop 01", CandidateKind::Sample)
        ]
    );
}

#[test]
fn a_growing_query_finds_what_a_fresh_one_does_and_a_key_taken_back_widens_again() {
    let mut typed = drums();
    for query in ["d", "dr", "dru", "drum"] {
        typed.rank(query, 50);
    }
    assert_eq!(typed.rank("drums", 50), drums().rank("drums", 50));
    // Back to "dr" and on to "dri": Drive, which "drums" had left out, is matched again.
    assert_eq!(typed.rank("dr", 50), drums().rank("dr", 50));
    assert_eq!(labels(&typed.rank("dri", 50)), [("Drive", CandidateKind::Track)]);
}

#[test]
fn only_the_best_come_back_and_only_they_are_made_into_candidates() {
    let sounds = Sounds::new((0..1000).map(|n| format!("Kick {n:03}")).collect());
    let mut referable = Referable::new(vec![], Some(sounds.clone()));
    let found = referable.rank("kick 9", 5);
    assert_eq!(
        labels(&found).iter().map(|(label, _)| *label).collect::<Vec<_>>(),
        ["Kick 900", "Kick 901", "Kick 902", "Kick 903", "Kick 904"]
    );
    assert_eq!(sounds.made.get(), 5, "the index is asked for the five it gives, not for every match");
}

/// A key typed into a library of 300,000 sounds, once the query has narrowed it to what it names, costs what one typed
/// into a library of 1,000 does: the match goes on among the last key's matches, not the whole library. Measured as the
/// fastest of rounds taken by turns, so a busy machine slows both alike and can't fail it, with no clock bound to miss
/// (#646's shape).
#[test]
fn a_key_typed_into_a_big_library_costs_what_one_typed_into_a_small_one_does() {
    let kicks: Vec<String> = (0..1000).map(|n| format!("Kick {n:03}")).collect();
    let pads = (0..299_000).map(|n| format!("Pad texture {n}"));
    let big = Sounds::new(kicks.iter().cloned().chain(pads).collect());
    let small = Sounds::new(kicks);
    let (mut big, mut small) = (Referable::new(vec![], Some(big)), Referable::new(vec![], Some(small)));
    let (mut fastest_big, mut fastest_small) = (Duration::MAX, Duration::MAX);
    for _ in 0..10 {
        for (referable, fastest) in [(&mut big, &mut fastest_big), (&mut small, &mut fastest_small)] {
            // The first key reads the whole list (untimed): only Kick's names have a k.
            referable.rank("k", 50);
            let started = Instant::now();
            for query in ["ki", "kic", "kick", "kick ", "kick 0"] {
                referable.rank(query, 50);
            }
            *fastest = (*fastest).min(started.elapsed());
        }
    }
    assert!(
        fastest_big < fastest_small * 10 + Duration::from_micros(200),
        "five keys took {fastest_big:?} in 300,000 sounds, {fastest_small:?} in 1,000"
    );
    let found = big.rank("kick 0", 50);
    assert_eq!((found.len(), found[0].label.as_str()), (50, "Kick 000"));
}

#[test]
fn the_librarys_references_go_in_the_contexts_references_after_the_projects() {
    let project = json!({"daw": "REAPER", "references": [{"said": "@Drums", "ref": "track:1"}]});
    let library = vec![json!({"said": "@Kick 808", "sound": "/Samples/Kick 808.wav"})];
    let context: Value = serde_json::from_str(&with_references(&project.to_string(), library.clone())).unwrap();
    assert_eq!(
        context["references"],
        json!([{"said": "@Drums", "ref": "track:1"}, {"said": "@Kick 808", "sound": "/Samples/Kick 808.wav"}])
    );
    assert_eq!(context["daw"], "REAPER");
    // A look that names nothing with @ gets the key, and one with no references at all is left as it was, byte for byte.
    let alone: Value = serde_json::from_str(&with_references(r#"{"daw":"REAPER"}"#, library.clone())).unwrap();
    assert_eq!(alone["references"], json!(library));
    assert_eq!(with_references(r#"{"daw": "REAPER"}"#, vec![]), r#"{"daw": "REAPER"}"#);
    // A context that isn't an object (a test's words) has them after it.
    assert_eq!(
        with_references("fixture", library),
        "fixture\n{\"references\":[{\"said\":\"@Kick 808\",\"sound\":\"/Samples/Kick 808.wav\"}]}"
    );
}
