//! The control channel's client against a fake of SoundCraft's server: the envelope every command goes in, the token,
//! and the traps. An answer out of step closes its connection (#894). A missed deadline keeps its connection until the
//! answer the app owes it comes, never opens more connections than the limit, and sends nothing twice. The app's
//! refusals say whether the request may have run.
use super::fake::{ok, refused, Answer, Fake};
use serde_json::json;
use soundcraft_kumi::control::{Control, Error};
use std::time::Duration;

const SECOND: Duration = Duration::from_secs(1);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The command a request runs (`engine.execute`'s `command`).
fn command(request: &serde_json::Value) -> &str {
    request["params"]["command"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn every_command_goes_inside_engine_execute_and_every_request_carries_the_token() {
    let fake = Fake::start(16, |request| Answer::Now(ok(request["method"].clone()))).await;
    let control = Control::new(fake.port(), Some("t0ken".into()));
    let result = control.execute("mix.volume", json!({"tracks": ["Kick"], "db": -6}), SECOND).await;
    assert_eq!(result, Ok(json!("engine.execute")));
    assert_eq!(control.call("ui.inspect", json!({}), SECOND).await, Ok(json!("ui.inspect")));
    let seen = fake.requests();
    assert_eq!(
        seen[0],
        json!({"id": 1, "method": "engine.execute", "params": {"command": "mix.volume", "params": {"tracks": ["Kick"], "db": -6}}, "token": "t0ken"})
    );
    assert_eq!(seen[1], json!({"id": 2, "method": "ui.inspect", "params": {}, "token": "t0ken"}));
    assert_eq!(fake.connections(), 1, "one connection, used again");
}

#[tokio::test]
async fn an_answer_out_of_step_closes_its_connection_and_leaves_the_request_uncertain() {
    let fake = Fake::start(16, |request| match command(request) {
        "first" => Answer::OutOfStep(ok(json!(1))),
        _ => Answer::Now(ok(json!(2))),
    })
    .await;
    let control = Control::new(fake.port(), None);
    let error = control.execute("first", json!({}), SECOND).await.unwrap_err();
    assert!(matches!(&error, Error::Garbled(said) if said.contains("out of step")), "{error:?}");
    assert!(error.uncertain(), "the app has the request, and runs it");
    assert_eq!(control.open(), 0);
    assert_eq!(control.execute("second", json!({}), SECOND).await, Ok(json!(2)), "a fresh connection answers in step");
    assert_eq!(fake.connections(), 2);
}

#[tokio::test]
async fn a_missed_deadline_keeps_its_connection_until_the_owed_answer_comes_and_nothing_is_sent_twice() {
    let fake = Fake::start(16, |request| match command(request) {
        "slow" => Answer::After(ms(100), ok(json!("slow"))),
        _ => Answer::Now(ok(json!("quick"))),
    })
    .await;
    let control = Control::new(fake.port(), None);
    let missed = control.execute("slow", json!({}), ms(50)).await.unwrap_err();
    assert_eq!(missed, Error::Deadline);
    assert!(missed.uncertain(), "the app may still run it");
    assert_eq!(control.execute("quick", json!({}), SECOND).await, Ok(json!("quick")), "on another connection");
    assert_eq!(fake.connections(), 2);
    // Once the owed answer has come (and been thrown away), its connection takes requests again.
    tokio::time::sleep(SECOND).await;
    let (a, b) = tokio::join!(control.execute("quick", json!({}), SECOND), control.execute("quick", json!({}), SECOND));
    assert_eq!((a, b), (Ok(json!("quick")), Ok(json!("quick"))));
    assert_eq!(fake.connections(), 2, "both connections at work again, none opened");
    assert_eq!(fake.requests().iter().filter(|request| command(request) == "slow").count(), 1, "sent once");
}

/// Waits up to two seconds for `done`.
async fn soon(done: impl Fn() -> bool) -> bool {
    for _ in 0..80 {
        if done() {
            return true;
        }
        tokio::time::sleep(ms(25)).await;
    }
    done()
}

#[tokio::test]
async fn a_request_whose_caller_stops_waiting_once_it_is_sent_waits_for_its_answer_and_its_connection_works_on() {
    let fake = Fake::start(16, |request| match command(request) {
        "late" => Answer::After(ms(300), ok(json!("late"))),
        _ => Answer::Now(ok(json!("quick"))),
    })
    .await;
    let control = Control::with_limits(fake.port(), None, 2, SECOND);
    // (the caller gives up once the request has gone, as an outer timeout or a stop does: its future is dropped)
    assert!(tokio::time::timeout(ms(100), control.execute("late", json!({}), SECOND)).await.is_err(), "no answer yet");
    assert_eq!(fake.requests().iter().map(command).collect::<Vec<_>>(), ["late"], "sent");
    assert_eq!(control.open(), 1, "kept for the answer the app owes");
    tokio::time::sleep(ms(600)).await;
    assert_eq!(control.execute("quick", json!({}), SECOND).await, Ok(json!("quick")));
    assert_eq!(fake.connections(), 1, "the owed answer came, and its connection took the next request");
}

#[tokio::test]
async fn requests_whose_callers_stop_waiting_give_their_connections_back_and_nothing_stays_busy() {
    let fake = Fake::start(16, |request| match command(request) {
        "stuck" => Answer::Never,
        _ => Answer::Now(ok(json!("quick"))),
    })
    .await;
    let control = Control::with_limits(fake.port(), None, 2, ms(200));
    for _ in 0..3 {
        let _ = tokio::time::timeout(ms(100), control.execute("stuck", json!({}), SECOND)).await;
    }
    assert_eq!(fake.requests().len(), 2, "two went, and the third found no connection to go on");
    assert!(soon(|| control.open() == 0).await, "each connection waited its owed while, then closed");
    assert_eq!(control.execute("quick", json!({}), SECOND).await, Ok(json!("quick")), "and the slots came back");
}

#[tokio::test]
async fn once_every_control_has_gone_its_idle_connections_close() {
    let fake = Fake::start(16, |_| Answer::Now(ok(json!(1)))).await;
    let control = Control::new(fake.port(), None);
    let (a, b) = tokio::join!(control.execute("one", json!({}), SECOND), control.execute("two", json!({}), SECOND));
    assert_eq!((a, b), (Ok(json!(1)), Ok(json!(1))));
    assert_eq!((control.open(), fake.open()), (2, 2), "both connections idle");
    let clone = control.clone();
    drop(control);
    tokio::time::sleep(ms(100)).await;
    assert_eq!(fake.open(), 2, "a clone still has them");
    drop(clone);
    assert!(soon(|| fake.open() == 0).await, "closed with the last clone: {} open", fake.open());
}

#[tokio::test]
async fn missed_deadlines_never_open_more_connections_than_the_limit_and_what_found_none_did_not_run() {
    let fake = Fake::start(16, |_| Answer::Never).await;
    let control = Control::with_limits(fake.port(), None, 3, Duration::from_secs(5));
    let mut errors = vec![];
    for _ in 0..6 {
        errors.push(control.execute("stuck", json!({}), ms(30)).await.unwrap_err());
    }
    use Error::{Busy, Deadline};
    assert_eq!(errors, [Deadline, Deadline, Deadline, Busy, Busy, Busy]);
    assert!(!Busy.uncertain());
    assert_eq!((fake.connections(), fake.requests().len(), control.open()), (3, 3, 3));
}

#[tokio::test]
async fn an_owed_answer_that_never_comes_closes_its_connection_after_the_owed_wait() {
    let fake = Fake::start(16, |_| Answer::Never).await;
    let control = Control::with_limits(fake.port(), None, 8, ms(200));
    assert_eq!(control.execute("stuck", json!({}), ms(30)).await, Err(Error::Deadline));
    assert_eq!(control.open(), 1, "waiting for the answer it's owed");
    for _ in 0..40 {
        if control.open() == 0 && fake.open() == 0 {
            break;
        }
        tokio::time::sleep(ms(25)).await;
    }
    assert_eq!((control.open(), fake.open()), (0, 0), "given up on and closed");
}

#[tokio::test]
async fn a_crowded_app_refuses_the_connection_and_nothing_runs() {
    let fake = Fake::start(1, |_| Answer::After(ms(500), ok(json!(1)))).await;
    let first = Control::new(fake.port(), None);
    let held = tokio::spawn(async move { first.execute("long", json!({}), SECOND).await });
    for _ in 0..40 {
        if fake.open() == 1 {
            break;
        }
        tokio::time::sleep(ms(10)).await;
    }
    let other = Control::new(fake.port(), None);
    let error = other.execute("short", json!({}), SECOND).await.unwrap_err();
    assert_eq!(error, Error::Crowded);
    assert!(!error.uncertain());
    assert_eq!(other.open(), 0);
    assert_eq!(held.await.unwrap(), Ok(json!(1)));
    assert_eq!(fake.requests().len(), 1, "the crowded request never reached the app");
}

#[tokio::test]
async fn a_connection_the_app_closed_while_idle_is_not_used() {
    let fake = Fake::start(16, |_| Answer::Now(ok(json!(1)))).await;
    let control = Control::new(fake.port(), None);
    assert_eq!(control.execute("a", json!({}), SECOND).await, Ok(json!(1)));
    fake.drop_connections();
    for _ in 0..40 {
        if fake.open() == 0 {
            break;
        }
        tokio::time::sleep(ms(10)).await;
    }
    tokio::time::sleep(ms(50)).await;
    assert_eq!(control.execute("b", json!({}), SECOND).await, Ok(json!(1)), "a new connection, not an uncertain write");
    assert_eq!(fake.connections(), 2);
}

#[tokio::test]
async fn a_refusal_keeps_its_connection_but_a_wrong_token_closes_it() {
    let fake = Fake::start(16, |request| match command(request) {
        "late" => Answer::Now(refused("timeout")),
        "token" => Answer::ThenClose(refused("unauthorized: the token is wrong")),
        _ => Answer::Now(refused("unknown command `mix.nope`")),
    })
    .await;
    let control = Control::new(fake.port(), Some("wrong".into()));
    let stalled = control.execute("late", json!({}), SECOND).await.unwrap_err();
    assert_eq!(stalled, Error::Stalled);
    assert!(stalled.uncertain(), "still queued in the app, and may run");
    let unknown = control.execute("mix.nope", json!({}), SECOND).await.unwrap_err();
    assert_eq!(unknown.unknown_command(), Some("mix.nope"));
    assert_eq!(fake.connections(), 1, "refusals keep their connection");
    let unauthorized = control.execute("token", json!({}), SECOND).await.unwrap_err();
    assert_eq!(unauthorized, Error::Unauthorized("unauthorized: the token is wrong".into()));
    assert_eq!(control.open(), 0);
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let port = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let error = Control::new(port, None).call("ui.inspect", json!({}), SECOND).await.unwrap_err();
    assert!(matches!(error, Error::Unreachable(_)), "{error:?}");
    assert!(!error.uncertain());
}
