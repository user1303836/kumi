//! Controller and playbook cases.
//! Session orchestration cases remain with the session port.
use kumi_runtime::core::{
    contracts::{AuditionEvent, MIX_CANDIDATE},
    goal::Best,
    match_run::*,
    playbook::*,
};
use serde_json::{json, Value};
use std::rc::Rc;
fn event(score: f64) -> Value {
    json!({"round":1,"best":{"label":"Drift","score":score},"takes":[{"label":"Drift","score":score},{"label":"Other","score":score-10.0}],"gaps":["attack too slow"],"request":{"candidates":[{"track":"track:1","label":"Drift"}],"fromBeat":16,"beats":4,"reference":"~/ref.wav"}})
}
fn heard(run: &mut MatchRun, value: Value) {
    run.auditioned(serde_json::from_value::<AuditionEvent>(value).unwrap(), None);
}
fn run() -> MatchRun {
    MatchRun::with_clock("make it sound like this", MATCH_BUDGET, Rc::new(|| 0))
}
fn next(decision: MatchDecision) -> String {
    match decision {
        MatchDecision::Next { next } => next,
        _ => panic!("expected continuation"),
    }
}
#[test]
fn only_requests_with_something_to_match_start_runs() {
    for text in [
        "match this reference",
        "recreate the sound from https://youtu.be/x",
        "make it sound like this",
        "make my pad sound like ~/ref.wav",
        "recreate this sound",
    ] {
        assert!(starts_match(text), "{text}");
    }
    for text in ["match the kick level to the snare", "make it sound like a cathedral", "recreate the chorus with more energy"] {
        assert!(!starts_match(text), "{text}");
    }
    for text in ["keep going", "Carry on.", "keep trying with a wavetable"] {
        assert!(KEEP_GOING.is_match(text), "{text}");
    }
    for text in ["more reverb", "again, but darker", "continue the bassline into bar 9"] {
        assert!(!KEEP_GOING.is_match(text), "{text}");
    }
}
#[test]
fn no_comparison_asks_for_an_audition_twice_then_stops() {
    let mut r = run();
    assert!(next(r.decide()).contains("audition what you built"));
    assert!(matches!(r.decide(), MatchDecision::Next { .. }));
    assert_eq!(r.decide(), MatchDecision::Stop { stop: MatchStop::NoAudition, wrap_up: None });
}
#[test]
fn best_track_and_clip_names_survive_lower_scoring_auditions_and_polish_happens_once() {
    let mut r = run();
    let mut e = event(60.0);
    e["takes"][0]["where"] = json!({"track":"Cand Drift","clip":"scene:2"});
    heard(&mut r, e);
    assert_eq!(serde_json::to_value(&r.best_candidate).unwrap(), json!({"track":"Cand Drift","label":"Drift","clip":"scene:2"}));
    assert!(r.polishes());
    let mut e = event(55.0);
    e["best"]["label"] = json!("Other");
    e["takes"] = json!([{"label":"Other","score":55,"where":{"track":"Cand Other"}}]);
    heard(&mut r, e);
    assert_eq!(r.best_candidate.as_ref().unwrap().track, "Cand Drift");
    r.tuned("Drift, tuned", 66.0);
    assert_eq!(r.best, Some(Best { label: "Drift, tuned".into(), score: 66.0 }));
    assert!(!r.polishes());
}
#[test]
fn single_candidate_starts_wide_once_and_structural_gaps_lead_the_next_round() {
    let mut r = run();
    let mut e = event(58.0);
    e["takes"] = json!([{"label":"Drift","score":58}]);
    heard(&mut r, e.clone());
    let prompt = next(r.decide());
    assert!(prompt.contains("with a single candidate."));
    assert!(prompt.contains("build 2–3 more genuinely different candidates"));
    e["best"]["score"] = json!(60);
    heard(&mut r, e);
    assert!(next(r.decide()).contains("Keep going"));
    let mut r = run();
    let mut e = event(60.0);
    e["structural"] = json!({"gap":"sub −20.0 dB against the reference","move":"add a sub layer (an Operator sine …)"});
    heard(&mut r, e);
    let prompt = next(r.decide());
    assert!(prompt.contains("Knobs can't close this: sub −20.0 dB against the reference. Change the structure: add a sub layer"));
    assert!(!prompt.contains("Keep going"));
}
#[test]
fn whole_mix_never_goes_to_the_knob_search() {
    let mut r = run();
    let mut e = event(71.0);
    e["best"]["label"] = json!("The whole mix");
    e["takes"] = json!([{"label":"The whole mix","score":71,"where":{"track":MIX_CANDIDATE}}]);
    e["request"]["candidates"] = json!([{"track":MIX_CANDIDATE,"mix":true}]);
    heard(&mut r, e);
    assert_eq!(serde_json::to_value(&r.best_candidate).unwrap(), json!({"track":MIX_CANDIDATE,"label":"The whole mix","mix":true}));
    assert!(!r.polishes());
}
#[test]
fn controller_preserves_exact_prompts_target_plateau_budget_and_carry_on_state() {
    let mut r = run();
    heard(&mut r, event(50.0));
    assert!(next(r.decide()).starts_with(
        "[Kumi] Score 50% (best: Drift). Budget left: 12 rounds, about 45 minutes. Biggest gaps: attack too slow. Keep going"
    ));
    heard(&mut r, event(60.0));
    assert!(next(r.decide()).starts_with("[Kumi] Score 50% → 60%"));
    heard(&mut r, event(93.0));
    let MatchDecision::Stop { stop, wrap_up: Some(prompt) } = r.decide() else { panic!() };
    assert_eq!(stop, MatchStop::Reached);
    assert!(prompt.contains("the score before and after (50% → 93%)"));
    r.changed();
    assert!(r.needs_audition());
    let continued = MatchRun::carry_on(&r, MATCH_BUDGET, Rc::new(|| 100));
    assert_eq!(continued.first, Some(50.0));
    assert_eq!(continued.best, r.best);
    assert_eq!(continued.status(MatchState::Running, None).rounds_left, 12);
    assert!(continued.changed_since);
    let mut r = run();
    for score in [50.0, 51.0] {
        heard(&mut r, event(score));
        next(r.decide());
    }
    heard(&mut r, event(51.0));
    assert!(next(r.decide()).contains("refining has stalled"));
    heard(&mut r, event(52.0));
    next(r.decide());
    heard(&mut r, event(52.0));
    assert!(matches!(r.decide(), MatchDecision::Stop { stop: MatchStop::Plateau, .. }));
    let mut r = MatchRun::with_clock("match this reference", MatchBudget { rounds: 0, ..MATCH_BUDGET }, Rc::new(|| 0));
    heard(&mut r, event(50.0));
    assert!(matches!(r.decide(), MatchDecision::Stop { stop: MatchStop::Budget, .. }));
}
#[tokio::test]
async fn lessons_are_private_checked_and_selected_by_shared_words() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("playbook.json");
    let store = create_playbook_store(&file);
    let lesson = |id: &str, matched: &str, winner: &str| Lesson {
        id: id.into(),
        at: 1.0,
        matched: matched.into(),
        winner: winner.into(),
        from: 40.0,
        to: 70.0,
        moves: vec![],
        reaction: None,
    };
    store
        .save(&[
            lesson("l00000001", "a wobbly reese bass", "Operator + LFO filter"),
            lesson("l00000002", "an airy pad", "Wavetable + reverb"),
            lesson("bad", "x", "y"),
        ])
        .await
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let lessons = store.list().await.unwrap();
    assert_eq!(lessons.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), vec!["l00000001", "l00000002"]);
    let brief = playbook_brief(&lessons, "recreate this reese bass", 1);
    assert!(brief.contains("a wobbly reese bass: Operator + LFO filter won"));
    assert!(!brief.contains("airy pad"));
    assert_eq!(playbook_brief(&[], "anything", 5), "");
    for (request,expected) in [
        ("Make my pad sound like this reference: ~/ref.wav","pad"),("recreate this sound","sound"),
        ("Make a new MIDI track with a sound that sounds like this reference: ~/ref.wav. It's a chord.","sound"),
        ("build me a warm pad that sounds like the intro","warm pad"),
        ("https://www.youtube.com/watch?v=abc Listen to 0:05 - 0:25 of this track. Recreate the sound and the sequence on this track using native devices.","sound and the sequence")
    ] {assert_eq!(matched_from(request),expected);}
}
#[test]
fn lessons_keep_improvements_and_goal_scores_round_like_javascript() {
    let mut r = run();
    assert!(lesson_from(&r, 1.0).is_none());
    for score in [50.0, 55.0, 54.0, 62.0] {
        heard(&mut r, event(score));
    }
    let lesson = lesson_from(&r, 123.0).unwrap();
    assert_eq!(lesson.moves.iter().map(|m| m.score).collect::<Vec<_>>(), vec![50.0, 55.0, 62.0]);
    assert_eq!(lesson.from, 50.0);
    assert_eq!(lesson.to, 62.0);
    let goal = lesson_from_goal(
        "recreate this pad",
        Some("bright"),
        Some(GoalLeader { label: "A", chain: "Drift", score: 70.5 }),
        Some(40.5),
        &[40.5, 40.0, 55.5, 70.5],
        1.0,
    )
    .unwrap();
    assert_eq!((goal.from, goal.to), (41.0, 71.0));
    assert_eq!(goal.moves.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(), vec!["generation 1", "generation 3", "generation 4"]);
}
