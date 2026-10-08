//! The loop decides, not the model: it stops when the checklist is met, when changes stop helping, or at its budget,
//! and otherwise sends the model back with the round's numbers and the next target.
use kumi_runtime::{
    core::{
        goal_mode::{after_turn, gap_closed, measured_check, read_audit, Objective, ObjectiveBudget, ObjectiveState, Verdict},
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
    // Kumi ended it: the done its wrap-up asks for goes through, and nothing else is judged.
    assert_eq!(run.over(), Some("the last answers judged no change"));
    assert!(run.holds_done().is_none());
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
    // Turns without progress in a row are stuck: a measurement alone isn't progress.
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
    let mut measured = Objective::new("master it", budget);
    assert_eq!(after_turn(&mut measured, check.clone(), false, 1_000).verdict, Verdict::Continue);
    assert_eq!(after_turn(&mut measured, check, false, 2_000).verdict, Verdict::Stuck);
    judged.met = true;
    assert_eq!(after_turn(&mut goal, measured_check(Some(&judged)).unwrap(), false, 5_000).verdict, Verdict::Complete);
    assert_eq!(goal.state, ObjectiveState::Done);
    // Time is a budget too.
    let mut goal = Objective::new("master it", budget);
    assert_eq!(after_turn(&mut goal, go_on(), true, 60_000).verdict, Verdict::Budget);
}

#[test]
fn the_loop_says_when_its_over_so_only_the_runs_end_is_judged_after_that() {
    let mut run = LoopRun::new("master it", BUDGET);
    assert_eq!(run.over(), None);
    run.judged(round(0, RoundKind::Start, None, 0., false));
    for number in 1..=4 {
        run.judged(round(number, RoundKind::Judged, Some(true), 1.5, false));
        assert_eq!(run.over(), None, "round {number}");
    }
    // The fifth judged round spends the budget, even inside one long answer.
    run.judged(round(5, RoundKind::Judged, Some(true), 1.5, false));
    assert_eq!(run.over(), Some("its budget is spent"));
    // Met is over too; and once the run has ended, there's nothing to hold.
    let mut met = LoopRun::new("master it", BUDGET);
    met.judged(round(0, RoundKind::Start, None, 0., false));
    met.judged(round(1, RoundKind::Judged, Some(true), 2.5, true));
    assert_eq!(met.over(), Some("every item on the checklist is within tolerance"));
    met.judged(round(2, RoundKind::Done, None, 0., true));
    assert_eq!(met.over(), None);
}

#[test]
fn a_run_met_with_something_unread_names_it() {
    let mut done = round(2, RoundKind::Done, None, 2.5, true);
    assert!(done.lines().iter().any(|line| line == "  every item is within tolerance"), "{:?}", done.lines());
    let mut hidden = done.rows[0].clone();
    (hidden.label, hidden.after) = ("Decay, as the reference".into(), None);
    done.rows.push(hidden);
    assert!(
        done.lines().iter().any(|line| line == "  every item it could read is within tolerance; it couldn't read decay, as the reference"),
        "{:?}",
        done.lines()
    );
    let note = kumi_runtime::integrations::ableton::judge_tool::judge_reply(&done)["note"].as_str().unwrap().to_string();
    assert!(note.starts_with("Every item Kumi could read is within tolerance, but it couldn't read decay, as the reference"), "{note}");
}

#[test]
fn a_new_judged_run_inside_the_loop_keeps_its_counts_and_an_early_done_is_held_back() {
    let mut run = LoopRun::new("master it", BUDGET);
    assert!(!run.started() && run.holds_done().is_none());
    run.judged(round(0, RoundKind::Start, None, 0., false));
    run.judged(round(1, RoundKind::Judged, Some(true), 1.5, false));
    assert!(run.started());
    let held = run.holds_done().unwrap();
    assert!(
        held.contains("isn't over") && held.contains("Next: loudness (-10.5 now, wants -9 LUFS ±0.5)") && held.contains("4 rounds"),
        "{held}"
    );
    // Another run started in it doesn't start the counts again: its rounds spend the same budget.
    run.judged(round(0, RoundKind::Start, None, 0., false));
    for number in 1..=4 {
        run.judged(round(number, RoundKind::Judged, Some(true), 1.5, false));
    }
    assert_eq!(run.over(), Some("its budget is spent"));
    assert!(run.holds_done().is_none(), "over: done goes through");
    let status = run.status(kumi_runtime::core::loop_run::LoopState::Running, None);
    assert_eq!((status.rounds, status.kept, status.listens), (5, 5, 2 + 5));
    // Once its run has ended, nothing is held.
    run.judged(round(5, RoundKind::Done, None, 0., false));
    assert!(!run.started() && run.holds_done().is_none());
}

#[test]
fn the_gap_a_goals_turn_closed_is_read_off_the_judges_rounds_on_one_checklist() {
    let before = round(1, RoundKind::Judged, Some(true), 1., false);
    let mut after = round(2, RoundKind::Judged, Some(true), 2., false);
    // Kept: what stands is each round's after (1.5 → 0.5).
    assert_eq!(gap_closed(Some(&before), Some(&after)), Some(1.));
    // Taken back: what stands is the gap it found.
    after.kept = Some(false);
    assert_eq!(gap_closed(Some(&before), Some(&after)), Some(-1.));
    // Nothing before, or another checklist: nothing to compare.
    assert_eq!(gap_closed(None, Some(&after)), None);
    after.rows[0].id = "true-peak".into();
    assert_eq!(gap_closed(Some(&before), Some(&after)), None);
}
