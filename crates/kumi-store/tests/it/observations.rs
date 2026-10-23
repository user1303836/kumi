use kumi_store::{
    observations::{self, Kind, Observation},
    Store,
};
use serde_json::json;

fn observation(at: i64, kind: Kind) -> Observation {
    let mut observation = Observation::new(at, "20261006-120000-abcd", Some("0123456789abcdef0123456789abcdef".into()), kind);
    observation.weight = Some(-2.0);
    observation.heard = Some(true);
    observation.subject = json!({"change":"c3","family":"parameter","title":"Reverb Dry/Wet 20 → 45 %","track":"Drums"});
    observation.facts = json!({"by":"key","sinceChange":4200,"sinceHeard":3100});
    observation.context =
        json!({"set":{"tempo":172.0,"meter":"4/4","roles":{"drums":2}},"requests":["make the drums wetter — ok, «less»"]});
    observation
}

#[test]
fn observations_are_kept_whole_and_read_back_newest_first() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let first = observation(1_000, Kind::Undo);
    let mut second = observation(2_000, Kind::Words);
    second.project = None;
    second.weight = None;
    second.heard = None;
    second.facts = json!({"quote":"never put reverb on the kick","lean":"never"});
    let kept = vec![first.clone(), second.clone()];
    store.write_wait(move |c| kept.iter().try_for_each(|o| observations::append(c, o))).unwrap();
    assert_eq!(store.read(|c| observations::recent(c, 10)).unwrap(), vec![second.clone(), first]);
    assert_eq!(store.read(|c| observations::recent(c, 1)).unwrap(), vec![second]);
}

#[test]
fn every_kind_is_kept_and_nothing_else_is() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path().join("kumi.db")).unwrap();
    let kinds = [Kind::Words, Kind::Pick, Kind::TechniqueOffer, Kind::Undo, Kind::EditAfter, Kind::Reuse];
    let kept: Vec<_> = kinds.iter().enumerate().map(|(at, kind)| observation(at as i64, *kind)).collect();
    store.write_wait(move |c| kept.iter().try_for_each(|o| observations::append(c, o))).unwrap();
    let read: Vec<_> = store.read(|c| observations::recent(c, 10)).unwrap().into_iter().map(|o| o.kind).rev().collect();
    assert_eq!(read, kinds);
    // Silence isn't a reaction.
    let refused = store.write_wait(|c| {
        Ok(c.execute(
            "INSERT INTO observations (id, at, session, kind, subject, facts, context) VALUES ('x', 1, 's', 'silence', jsonb('{}'), jsonb('{}'), jsonb('{}'))",
            [],
        )?)
    });
    assert!(refused.is_err());
}
