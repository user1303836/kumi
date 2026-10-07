//! The loop decides, not the model: it stops when the checklist is met, when changes stop helping, or at its budget,
//! and otherwise sends the model back with the round's numbers and the next target.
use kumi_runtime::{
    core::{
        goal_mode::{after_turn, measured_check, read_audit, Objective, ObjectiveBudget, ObjectiveState, Verdict},
        loop_run::{wants_loop, LoopBudget, LoopDecision, LoopRun, LoopStop},
    },
    listening::{
        checklist::{Change, Row},
        round::{Next, Round, RoundKind},
    },
};
use std::{cell::Cell, rc::Rc};

fn round(number: u32, kind: RoundKind, kept: Option<bool>, closed: f64, met: bool) -> Round {
    Round {
        round: number,
        kind,
        heard: "the mix, bars 49–57".into(),
        target: Some("Loudness".into()),
        change: Some("Limiter gain +2 dB".into()),
        changes: vec![],
        rows: vec![Row {
            id: "loudness".into(),
            label: "Loudness".into(),
            unit: "LUFS".into(),
            wanted: "-9 LUFS ±0.5".into(),
            before: Some(-12.),
            after: Some(-12. + closed),
            gap_before: 2.5,
            gap_after: 2.5 - closed,
            change: if closed > 0. { Change::Better } else { Change::Same },
        }],
        kept,
        why: Some(if kept == Some(false) { "it hurt punch (crest)".into() } else { "loudness improved".into() }),
        rebalanced: None,
        listener: None,
        problems: vec![],
        next: (!met).then(|| Next {
            id: "loudness".into(),
            label: "Loudness".into(),
            gap: 1.,
            wanted: "-9 LUFS ±0.5".into(),
            now: Some(-10.5),
            fix: None,
        }),
        met,
        listens: number + 1,
        elapsed_ms: 0,
    }
}

const BUDGET: LoopBudget = LoopBudget { rounds: 5, ms: 60_000, stall: 3 };

#[test]
fn the_loop_goes_on_with_the_next_target_and_stops_when_the_checklist_is_met() {
    let mut run = LoopRun::new("master it", BUDGET);
    run.judged(round(0, RoundKind::Start, None, 0., false));
    run.judged(round(1, RoundKind::Judged, Some(true), 1.5, false));
    match run.decide() {
        LoopDecision::Next(text) => {
            assert!(
                text.contains("Round 1") && text.contains("Kept") && text.contains("Next: loudness") && text.contains("4 rounds"),
                "{text}"
            );
        }
        other => panic!("{other:?}"),
    }
    run.judged(round(2, RoundKind::Judged, Some(true), 1., true));
    match run.decide() {
        LoopDecision::Stop { stop: LoopStop::Met, wrap_up: Some(text) } => assert!(text.contains("done: true"), "{text}"),
        other => panic!("{other:?}"),
    }
    let status = run.status(kumi_runtime::core::loop_run::LoopState::Done, Some(LoopStop::Met));
    assert_eq!((status.rounds, status.kept, status.reverted, status.listens), (2, 2, 0, 3));
}

#[test]
fn the_loop_stops_when_changes_stop_helping_or_at_its_budget() {
    // Three taken back in a row: stalled.
    let mut run = LoopRun::new("fix the harshness", BUDGET);
    run.judged(round(0, RoundKind::Start, None, 0., false));
    for number in 1..=3 {
        run.judged(round(number, RoundKind::Judged, Some(false), 0., false));
        let decision = run.decide();
        if number < 3 {
            assert!(matches!(&decision, LoopDecision::Next(text) if text.contains("Taken back")), "{decision:?}");
        } else {
            assert!(matches!(decision, LoopDecision::Stop { stop: LoopStop::Stalled, .. }), "{decision:?}");
        }
    }
    // Five judged rounds is the budget, however well they go; and the clock is a budget too.
    let now = Rc::new(Cell::new(0));
    let clock = now.clone();
    let mut run = LoopRun::with_clock("master it", BUDGET, Rc::new(move || clock.get()));
    run.judged(round(0, RoundKind::Start, None, 0., false));
    for number in 1..=5 {
        run.judged(round(number, RoundKind::Judged, Some(true), 2., false));
    }
    assert!(matches!(run.decide(), LoopDecision::Stop { stop: LoopStop::Budget, .. }));
    let mut run = LoopRun::with_clock("master it", BUDGET, Rc::new(move || now.get()));
    run.judged(round(0, RoundKind::Start, None, 0., false));
    run.judged(round(1, RoundKind::Judged, Some(true), 2., false));
    let later = run.status(kumi_runtime::core::loop_run::LoopState::Running, None);
    assert_eq!(later.rounds_left, 4);
}

#[test]
fn an_answer_without_a_judged_change_is_nudged_once_then_the_loop_ends() {
    // Nothing judged at all: asked to start a judged run, once.
    let mut run = LoopRun::new("make the vocal cut through", BUDGET);
    assert!(matches!(run.decide(), LoopDecision::Next(text) if text.contains("Start a judged run")));
    assert!(matches!(run.decide(), LoopDecision::Stop { stop: LoopStop::Unjudged, wrap_up: None }));
    // A run that judged, then answers twice with nothing new: reminded twice, then it ends.
    let mut run = LoopRun::new("master it", BUDGET);
    run.judged(round(0, RoundKind::Start, None, 0., false));
    assert!(matches!(run.decide(), LoopDecision::Next(_)));
    assert!(matches!(run.decide(), LoopDecision::Next(text) if text.contains("Judge the change you made")));
    assert!(matches!(run.decide(), LoopDecision::Next(text) if text.contains("Judge the change you made")));
    assert!(matches!(run.decide(), LoopDecision::Stop { stop: LoopStop::Unjudged, .. }));
    // A run ended with done is over.
    let mut run = LoopRun::new("master it", BUDGET);
    run.judged(round(0, RoundKind::Start, None, 0., false));
    run.judged(round(3, RoundKind::Done, None, 0., false));
    assert!(matches!(run.decide(), LoopDecision::Stop { stop: LoopStop::Ended, wrap_up: None }));
}

#[test]
fn requests_that_call_for_the_loop_by_themselves() {
    for request in
        ["master this track to -9 LUFS", "make the mix less muddy", "fix the harshness in the hats", "make the vocal cut through"]
    {
        assert!(wants_loop(request), "{request}");
    }
    for request in ["add a reverb to the snare", "what's the tempo?", "make a 4-bar drum beat"] {
        assert!(!wants_loop(request), "{request}");
    }
}

#[test]
fn a_goals_check_reads_the_models_line_and_code_applies_the_budget_and_no_progress_rules() {
    let said = read_audit("Looked at the meters.\nBLOCKED: open the Set with the vocal stems first");
    assert_eq!((said.verdict, said.next.as_deref()), (Verdict::Blocked, Some("open the Set with the vocal stems first")));
    assert_eq!(read_audit("**COMPLETE** — master reads -9.1 LUFS").verdict, Verdict::Complete);
    let vague = read_audit("I think it's nearly there.");
    assert_eq!((vague.verdict, vague.reason.as_str()), (Verdict::Continue, "the check got no clear answer"));
    // Turns that change nothing in a row are stuck; a change, or a judged measurement, is progress.
    let budget = ObjectiveBudget { turns: 10, ms: 60_000, idle: 2 };
    let mut goal = Objective::new("make the vocal cut through", budget);
    let go_on = || read_audit("CONTINUE: cut the pads at 300 Hz");
    assert_eq!(after_turn(&mut goal, go_on(), false, 1_000).verdict, Verdict::Continue);
    assert_eq!(after_turn(&mut goal, go_on(), true, 2_000).verdict, Verdict::Continue);
    assert_eq!(after_turn(&mut goal, go_on(), false, 3_000).verdict, Verdict::Continue);
    let stuck = after_turn(&mut goal, go_on(), false, 4_000);
    assert_eq!(
        (stuck.verdict, stuck.next.as_deref(), goal.state),
        (Verdict::Stuck, Some("cut the pads at 300 Hz"), ObjectiveState::Paused)
    );
    // The judge's numbers: not met goes on toward the next target, met is complete.
    let mut judged = round(4, RoundKind::Judged, Some(true), 1., false);
    let check = measured_check(Some(&judged)).unwrap();
    assert!(
        check.measured && check.verdict == Verdict::Continue && check.next.as_deref() == Some("loudness (-10.5 now, wants -9 LUFS ±0.5)"),
        "{check:?}"
    );
    judged.met = true;
    assert_eq!(after_turn(&mut goal, measured_check(Some(&judged)).unwrap(), false, 5_000).verdict, Verdict::Complete);
    assert_eq!(goal.state, ObjectiveState::Done);
    // Time is a budget too.
    let mut goal = Objective::new("master it", budget);
    assert_eq!(after_turn(&mut goal, go_on(), true, 60_000).verdict, Verdict::Budget);
}
