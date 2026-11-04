//! Kumi in a herdr pane: herdr hears Kumi working, waiting for the producer and idle, with the command that starts Kumi
//! again on its conversation once herdr restarts, and gets its pane back when the producer quits.
#[path = "../support/tui_app.rs"]
mod support;
use futures::FutureExt;
use kumi::{
    terminal::Terminal,
    tui::{app::TuiOptions, herdr::HerdrPane},
};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

/// What Kumi ran of herdr's, each run's arguments in order.
type Ran = Rc<RefCell<Vec<Vec<String>>>>;

const KEPT: &str = "mgj6k3cw0a0b0c";

/// Kumi in herdr's pane `w1:p2`, started as `kumi --daw reaper`; herdr's runs are kept, each held while `hold` is set.
fn in_herdr(control: Rc<Control>, hold: Rc<Cell<bool>>, configure: impl FnOnce(&mut TuiOptions)) -> (Harness, Ran) {
    let ran: Ran = Rc::default();
    let into = ran.clone();
    let h = Harness::with(120, 36, control, move |o| {
        o.herdr = Some(HerdrPane {
            program: "herdr".into(),
            pane: "w1:p2".into(),
            start: vec!["kumi".into(), "--daw".into(), "reaper".into()],
            run: Some(Rc::new(move |args, _| {
                into.borrow_mut().push(args);
                let hold = hold.clone();
                async move {
                    while hold.get() {
                        delay(5).await;
                    }
                }
                .boxed_local()
            })),
        });
        configure(o);
    });
    (h, ran)
}

/// Each run as words, its number as `#`.
fn told(ran: &Ran) -> Vec<String> {
    ran.borrow()
        .iter()
        .map(|args| {
            let seq = args.iter().position(|arg| arg == "--seq").unwrap() + 1;
            args.iter().enumerate().map(|(at, arg)| if at == seq { "#" } else { arg.as_str() }).collect::<Vec<_>>().join(" ")
        })
        .collect()
}

/// Each run's number.
fn numbers(ran: &Ran) -> Vec<u64> {
    ran.borrow().iter().map(|args| args[args.iter().position(|arg| arg == "--seq").unwrap() + 1].parse().unwrap()).collect()
}

async fn until_told(ran: &Ran, h: &Harness, last: &str) {
    for _ in 0..300 {
        h.screen();
        if told(ran).last().is_some_and(|said| said == last) {
            return;
        }
        delay(10).await;
    }
    panic!("herdr never heard {last:?}: {:#?}", told(ran));
}

const REPORT: &str = "pane report-agent w1:p2 --source kumi --agent kumi --state";

case!(herdr_hears_kumi_work_wait_and_rest_and_starts_it_again_on_its_conversation, async {
    let control = Rc::new(Control::default());
    let (h, ran) = in_herdr(control.clone(), Rc::default(), |_| {});
    h.start().await;
    h.connect();
    // Nothing kept yet: herdr's restart starts Kumi afresh, as it was started.
    until_told(&ran, &h, &format!("{REPORT} idle --seq # -- kumi --daw reaper")).await;
    h.type_text("make the bass darker\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    until_told(&ran, &h, &format!("{REPORT} working --seq # -- kumi --daw reaper")).await;
    // The request kept the conversation as it ended: a restart takes it up.
    control.set("now", json!({"id":KEPT,"title":"Darker bass","kept":true,"touched":true}));
    h.emit(json!({"type":"text","text":"The cutoff is down 2 kHz."}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    let kept = format!("--agent-session-id {KEPT} -- kumi --daw reaper -r {KEPT}");
    until_told(&ran, &h, &format!("{REPORT} idle --seq # {kept}")).await;
    // An answer that asks the producer to pick waits on them.
    h.type_text("which pad?\r").await;
    h.emit(json!({"type":"state","state":"running"}));
    h.emit(json!({"type":"text","text":"Which pad?\n\n1. Warm\n2. Glassy"}));
    h.emit(json!({"type":"turn-complete","result":{"stopReason":"completed"},"elapsedMs":900}));
    h.emit(json!({"type":"state","state":"idle"}));
    until_told(&ran, &h, &format!("{REPORT} blocked --seq # --message Your answer, in Kumi {kept}")).await;
    h.type_text("1\r").await;
    h.wait_for_call("submit:Warm").await;
    until_told(&ran, &h, &format!("{REPORT} idle --seq # {kept}")).await;
    // Quitting gives the pane back, numbered after every report before it.
    h.close().await;
    assert_eq!(told(&ran).last().unwrap(), "pane release-agent w1:p2 --source kumi --agent kumi --seq #");
    let numbers = numbers(&ran);
    assert!(numbers.windows(2).all(|pair| pair[1] > pair[0]), "{numbers:?}");
});

case!(a_new_conversation_is_what_herdrs_restart_starts_not_the_one_before, async {
    let control = Rc::new(Control::default());
    control.set("now", json!({"id":KEPT,"title":"Darker bass","kept":true,"touched":true}));
    let (h, ran) = in_herdr(control.clone(), Rc::default(), |_| {});
    h.start().await;
    h.connect();
    until_told(&ran, &h, &format!("{REPORT} idle --seq # --agent-session-id {KEPT} -- kumi --daw reaper -r {KEPT}")).await;
    control.set("now", json!({"id":"mgj7f0f0f0f0f0","title":"","kept":false,"touched":false}));
    h.type_text("/new\r").await;
    h.wait_for_call("new").await;
    until_told(&ran, &h, &format!("{REPORT} idle --seq # -- kumi --daw reaper")).await;
    h.close().await;
});

case!(kumi_closing_to_open_again_keeps_herdrs_pane, async {
    let (h, ran) = in_herdr(Rc::new(Control::default()), Rc::default(), |o| o.switch_daw = Some(Rc::new(|_| {})));
    let done = h.app.run();
    delay(5).await;
    h.connect();
    until_told(&ran, &h, &format!("{REPORT} idle --seq # -- kumi --daw reaper")).await;
    // The Kumi that opens tells herdr itself: given back now, the pane would show no Kumi meanwhile.
    h.type_text("/daw renoise\r").await;
    assert_eq!(done.await, 0);
    assert!(told(&ran).iter().all(|said| !said.contains("release-agent")), "{:#?}", told(&ran));
    h.close().await;
});

case!(while_herdr_takes_a_report_only_the_newest_state_waits, async {
    let hold = Rc::new(Cell::new(true));
    let (h, ran) = in_herdr(Rc::new(Control::default()), hold.clone(), |_| {});
    h.start().await;
    h.connect();
    until_told(&ran, &h, &format!("{REPORT} idle --seq # -- kumi --daw reaper")).await;
    // Working, idle and working again while herdr still takes the first report: only the last goes, once it has.
    for state in ["running", "idle", "running"] {
        h.emit(json!({"type":"state","state":state}));
        h.screen();
    }
    assert_eq!(ran.borrow().len(), 1, "{:#?}", told(&ran));
    hold.set(false);
    until_told(&ran, &h, &format!("{REPORT} working --seq # -- kumi --daw reaper")).await;
    delay(50).await;
    h.screen();
    assert_eq!(ran.borrow().len(), 2, "{:#?}", told(&ran));
    h.close().await;
});
