//! A bridge the host starts itself, for a DAW nothing of Kumi's can be loaded into. It's started for a running DAW when
//! none answers, once however many calls ask, never waited on past a moment, stopped with the session, and ended with a
//! host that is killed. The first tests run simulated bridges in this process; the last run the test binary as the bridge
//! and as the host, to kill a real host.
use daw_bridge::files::{ensure_secret, process_alive, read_secret, write_discovery, Discovery, DISCOVERY_SCHEMA};
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::{Host, Pick};
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use daw_mcp_server::spawn::{self, Spawn, SpawnedBridge, Times};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const STARTING: &str = "Kumi's Renoise bridge is starting: ask again in a moment";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn anyone() -> Pick {
    Pick::default()
}

/// What the bridges the host starts do: serve after a while, never serve, or end at once saying why.
#[derive(Clone, Copy)]
enum How {
    Serve(Duration),
    Never,
    End(&'static str),
}

/// The simulated DAW: whether it runs, what its bridges do, and what became of them.
struct World {
    running: AtomicBool,
    how: How,
    starts: AtomicUsize,
    /// When the host last started a bridge.
    started: Mutex<Option<Instant>>,
    /// Bridges the host asked to end.
    stopped: AtomicUsize,
    serving: Mutex<Vec<Arc<Bridge>>>,
}

impl World {
    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    fn stopped(&self) -> usize {
        self.stopped.load(Ordering::SeqCst)
    }

    /// How long ago the host last started a bridge.
    fn since_start(&self) -> Duration {
        self.started.lock().unwrap().expect("a bridge started").elapsed()
    }

    /// The DAW quits: its bridges end, taking their discovery files with them.
    fn quit(&self) {
        self.running.store(false, Ordering::SeqCst);
        for bridge in self.serving.lock().unwrap().drain(..) {
            bridge.end("its DAW quit");
        }
    }

    /// The DAW quits and its bridges' files go (as the DAW's pid, in them, ends), but the bridges themselves run on.
    fn quit_leaving_bridges(&self) {
        self.running.store(false, Ordering::SeqCst);
        for bridge in self.serving.lock().unwrap().drain(..) {
            if let Some(simulator) = bridge.simulator.lock().unwrap().take() {
                simulator.stop();
            }
        }
    }
}

/// One simulated bridge: its simulator once it serves, and how it ended.
#[derive(Default)]
struct Bridge {
    simulator: Mutex<Option<Simulator>>,
    ended: Mutex<Option<String>>,
}

impl Bridge {
    fn end(&self, why: &str) {
        if let Some(simulator) = self.simulator.lock().unwrap().take() {
            simulator.stop();
        }
        self.ended.lock().unwrap().get_or_insert_with(|| why.to_string());
    }
}

struct Started(Arc<Bridge>, Arc<World>);

impl SpawnedBridge for Started {
    fn exited(&mut self) -> Option<String> {
        self.0.ended.lock().unwrap().clone()
    }

    fn stop(&mut self) {
        self.1.stopped.fetch_add(1, Ordering::SeqCst);
        self.0.end("asked to end");
    }

    fn kill(&mut self) {
        self.0.end("killed");
    }
}

/// A host for a simulated Renoise whose bridge the host starts, in a Kumi folder of its own.
struct Lab {
    home: tempfile::TempDir,
    world: Arc<World>,
}

impl Lab {
    fn new(how: How) -> Self {
        let world = World {
            running: AtomicBool::new(true),
            how,
            starts: 0.into(),
            started: Mutex::default(),
            stopped: 0.into(),
            serving: Mutex::default(),
        };
        let home = tempfile::tempdir().unwrap();
        // (made here, the secret's owner-only permissions are no part of any call's time: a PowerShell run on Windows)
        ensure_secret(&BridgePaths::new(home.path(), Daw::Renoise).secret()).unwrap();
        Self { home, world: Arc::new(world) }
    }

    fn paths(&self) -> BridgePaths {
        BridgePaths::new(self.home.path(), Daw::Renoise)
    }

    fn host(&self, times: Times) -> Host {
        let (world, paths) = (self.world.clone(), self.paths());
        let running = self.world.clone();
        let spawn = Spawn::new(
            move || running.running.load(Ordering::SeqCst),
            move || {
                let (world, paths) = (world.clone(), paths.clone());
                Box::pin(async move {
                    world.starts.fetch_add(1, Ordering::SeqCst);
                    *world.started.lock().unwrap() = Some(Instant::now());
                    let bridge = Arc::new(Bridge::default());
                    match world.how {
                        How::End(why) => bridge.end(why),
                        How::Never => {}
                        How::Serve(after) => {
                            let (bridge, world) = (bridge.clone(), world.clone());
                            tokio::spawn(async move {
                                tokio::time::sleep(after).await;
                                let mut config = SimulatorConfig::renoise(read_secret(&paths.secret()).unwrap(), "/lab/Renoise");
                                config.discovery_dir = Some(paths.discovery());
                                let simulator = Simulator::start(config).await.unwrap();
                                if bridge.ended.lock().unwrap().is_some() {
                                    simulator.stop();
                                    return;
                                }
                                *bridge.simulator.lock().unwrap() = Some(simulator);
                                world.serving.lock().unwrap().push(bridge);
                            });
                        }
                    }
                    Ok(Box::new(Started(bridge, world)) as Box<dyn SpawnedBridge>)
                })
            },
        );
        Host::new(Daw::Renoise, self.home.path(), daw_mcp_server::host::ListeningMode::Standalone).with_spawn(spawn.with_times(times))
    }

    fn discovery_files(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.paths().discovery()) else { return vec![] };
        entries.flatten().map(|entry| entry.path()).collect()
    }

    /// A gone bridge's discovery file: its DAW (this test's process) runs on, and nothing listens on its port.
    fn gone_file(&self) -> PathBuf {
        let closed = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let gone = Discovery {
            schema: DISCOVERY_SCHEMA.into(),
            daw: "renoise".into(),
            pid: std::process::id(),
            host: "127.0.0.1".into(),
            port: closed,
            protocol: daw_bridge::wire::PROTOCOL.into(),
            version: "0.1.0".into(),
            daw_version: "3.5.4".into(),
            resource: "/lab/Renoise-before".into(),
            project: None,
            started_ms: 1,
        };
        write_discovery(&self.paths().discovery(), &gone).unwrap()
    }
}

fn reason(status: &Value) -> &str {
    status["reason"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn a_running_daw_gets_one_bridge_started_and_the_session_stops_it() {
    let lab = Lab::new(How::Serve(ms(100)));
    let host = lab.host(Times::default());
    let status = host.status(&anyone()).await;
    assert_eq!(status["connected"], true, "{status}");
    for _ in 0..5 {
        assert_eq!(host.status(&anyone()).await["connected"], true);
    }
    assert_eq!(lab.world.starts(), 1, "started once, then used");
    let files = lab.discovery_files();
    assert_eq!(files.len(), 1);
    host.close().await;
    assert_eq!(lab.world.stopped(), 1);
    assert!(!files[0].exists(), "the bridge took its discovery file with it");
}

#[tokio::test]
async fn a_closed_daw_starts_nothing_and_no_call_waits() {
    let lab = Lab::new(How::Serve(Duration::ZERO));
    lab.world.running.store(false, Ordering::SeqCst);
    let host = lab.host(Times::default());
    let began = Instant::now();
    for _ in 0..50 {
        let status = host.status(&anyone()).await;
        assert_eq!((&status["connected"], reason(&status)), (&json!(false), "no Renoise is running: open Renoise"));
    }
    let found = host.discover().await;
    assert_eq!((&found["instances"], &found["hint"]), (&json!([]), &json!("no Renoise is running: open Renoise")));
    assert_eq!(lab.world.starts(), 0);
    // (one call that waited for a bridge would take the 2 s a call may wait)
    assert!(began.elapsed() < Duration::from_secs(2), "51 calls took {:?}", began.elapsed());
}

#[tokio::test]
async fn calls_at_once_share_one_starting_bridge() {
    let lab = Lab::new(How::Serve(ms(300)));
    let host = Arc::new(lab.host(Times::default()));
    let calls: Vec<_> = (0..8)
        .map(|_| {
            let host = host.clone();
            tokio::spawn(async move { host.status(&anyone()).await })
        })
        .collect();
    for call in calls {
        let status = call.await.unwrap();
        assert_eq!(status["connected"], true, "{status}");
    }
    assert_eq!(lab.world.starts(), 1);
}

#[tokio::test]
async fn a_bridge_slow_to_serve_holds_a_call_only_a_moment_and_is_stopped_after_the_serve_wait() {
    let times = Times { call: ms(300), serve: ms(1500), retry: ms(1000), stop: ms(200) };
    let lab = Lab::new(How::Never);
    let host = lab.host(times);
    let began = Instant::now();
    let first = host.status(&anyone()).await;
    assert_eq!(reason(&first), STARTING);
    assert!(began.elapsed() < ms(1200), "a call waits a moment for a starting bridge, not its whole start: {:?}", began.elapsed());
    assert_eq!(reason(&host.status(&anyone()).await), STARTING);
    assert_eq!(lab.world.starts(), 1, "a starting bridge is waited for, not started again");
    // (timed from the bridge's start, which the first call set going)
    tokio::time::sleep(times.serve.saturating_sub(lab.world.since_start()) + ms(150)).await;
    let failed = host.status(&anyone()).await;
    assert_eq!(reason(&failed), "Kumi's Renoise bridge didn't start serving within 1.5 s");
    // (stopped beside the call)
    let until = Instant::now() + Duration::from_secs(2);
    while lab.world.stopped() == 0 && Instant::now() < until {
        tokio::time::sleep(ms(20)).await;
    }
    assert_eq!(lab.world.stopped(), 1);
    assert_eq!(reason(&host.status(&anyone()).await), reason(&failed), "told why at once, with no bridge started");
    assert_eq!(lab.world.starts(), 1);
    tokio::time::sleep(times.retry).await;
    assert_eq!(reason(&host.status(&anyone()).await), STARTING);
    assert_eq!(lab.world.starts(), 2, "started again once the retry wait is over");
}

#[tokio::test]
async fn a_bridge_that_ends_before_it_serves_says_why_and_is_not_started_again_at_once() {
    let times = Times { retry: ms(500), ..Times::default() };
    let lab = Lab::new(How::End("Renoise is open without its control channel"));
    let host = lab.host(times);
    let said = "Kumi's Renoise bridge ended before it served: Renoise is open without its control channel";
    assert_eq!(reason(&host.status(&anyone()).await), said);
    assert_eq!(reason(&host.status(&anyone()).await), said);
    assert_eq!(lab.world.starts(), 1);
    tokio::time::sleep(times.retry).await;
    assert_eq!(reason(&host.status(&anyone()).await), said);
    assert_eq!(lab.world.starts(), 2);
}

#[tokio::test]
async fn a_bridge_goes_with_its_daw_and_a_reopened_daw_gets_a_new_one() {
    let lab = Lab::new(How::Serve(ms(50)));
    let host = lab.host(Times::default());
    assert_eq!(host.status(&anyone()).await["connected"], true);
    lab.world.quit();
    // (the first call after may be the one that finds the connection closed)
    let mut status = host.status(&anyone()).await;
    for _ in 0..20 {
        if reason(&status) == "no Renoise is running: open Renoise" {
            break;
        }
        tokio::time::sleep(ms(50)).await;
        status = host.status(&anyone()).await;
    }
    assert_eq!(reason(&status), "no Renoise is running: open Renoise", "{status}");
    assert_eq!(lab.world.starts(), 1);
    lab.world.running.store(true, Ordering::SeqCst);
    let status = host.status(&anyone()).await;
    assert_eq!(status["connected"], true, "{status}");
    assert_eq!(lab.world.starts(), 2);
}

/// Asks for `status` until it says `reason`, for a few seconds (the first call after a change may be the one that finds
/// the connection closed, or the one that starts a bridge).
async fn status_saying(host: &Host, reason_said: &str) -> Value {
    let until = Instant::now() + Duration::from_secs(5);
    let mut status = host.status(&anyone()).await;
    while reason(&status) != reason_said && Instant::now() < until {
        tokio::time::sleep(ms(50)).await;
        status = host.status(&anyone()).await;
    }
    status
}

#[tokio::test]
async fn a_gone_bridges_file_is_removed_and_one_bridge_is_started_in_its_place() {
    let lab = Lab::new(How::Serve(ms(50)));
    let file = lab.gone_file();
    let host = lab.host(Times::default());
    // (on Windows the refused connect takes its 2 s, the call's whole wait: then the next call connects)
    let mut status = host.status(&anyone()).await;
    if status["connected"] != true {
        status = host.status(&anyone()).await;
    }
    assert_eq!(status["connected"], true, "{status}");
    assert!(!file.exists(), "the gone bridge's file was removed");
    assert_eq!(lab.world.starts(), 1, "one bridge started in its place");
}

#[tokio::test]
async fn discover_removes_a_gone_bridges_file_and_starts_one_bridge_in_its_place() {
    let lab = Lab::new(How::Serve(ms(50)));
    let file = lab.gone_file();
    let host = lab.host(Times::default());
    let found = host.discover().await;
    assert_eq!(found["staleRemoved"], 1, "{found}");
    assert!(!file.exists(), "the gone bridge's file was removed");
    // (the new bridge is listed when it serves within the call's wait; on Windows the refused connect takes that wait, and
    // the call answers that the bridge is starting before its start's task has run)
    let listed = found["instances"].as_array().map_or(0, Vec::len);
    assert!(listed == 1 || found["hint"] == STARTING, "{found}");
    let until = Instant::now() + Duration::from_secs(5);
    while lab.world.starts() == 0 && Instant::now() < until {
        tokio::time::sleep(ms(20)).await;
    }
    assert_eq!(lab.world.starts(), 1, "one bridge started in its place");
}

#[tokio::test]
async fn a_bridge_left_running_after_its_daw_quit_is_stopped() {
    let lab = Lab::new(How::Serve(ms(50)));
    let host = lab.host(Times::default());
    assert_eq!(host.status(&anyone()).await["connected"], true);
    lab.world.quit_leaving_bridges();
    let status = status_saying(&host, "no Renoise is running: open Renoise").await;
    assert_eq!(reason(&status), "no Renoise is running: open Renoise", "{status}");
    let until = Instant::now() + Duration::from_secs(2);
    while lab.world.stopped() == 0 && Instant::now() < until {
        tokio::time::sleep(ms(20)).await;
    }
    assert_eq!((lab.world.stopped(), lab.world.starts()), (1, 1), "the bridge was stopped, and none started");
}

/// One of this binary's ignored tests, run as a program.
fn this_test(name: &str) -> (PathBuf, Vec<OsString>) {
    let args = ["--exact", name, "--ignored", "--nocapture", "--test-threads=1"];
    (std::env::current_exe().unwrap(), args.into_iter().map(OsString::from).collect())
}

#[tokio::test]
async fn a_bridge_program_that_ends_before_it_serves_is_quoted() {
    let home = tempfile::tempdir().unwrap();
    let (program, args) = this_test("spawn::a_bridge_that_ends_saying_why");
    let starts = Spawn::new(|| true, spawn::program(program, args)).with_times(Times { call: Duration::from_secs(20), ..Times::default() });
    let host = Host::new(Daw::Renoise, home.path(), daw_mcp_server::host::ListeningMode::Standalone).with_spawn(starts);
    let status = host.status(&anyone()).await;
    assert_eq!(reason(&status), "Kumi's Renoise bridge ended before it served: Renoise is open without its control channel");
}

#[test]
#[ignore = "run by a_bridge_program_that_ends_before_it_serves_is_quoted"]
fn a_bridge_that_ends_saying_why() {
    eprintln!("starting");
    eprintln!("Renoise is open without its control channel");
    std::process::exit(3);
}

/// A process this test started, killed if the test ends first.
struct Running(std::process::Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_bridge_ends_with_its_host_when_the_host_is_killed_and_leaves_no_discovery_file() {
    use std::io::BufRead;
    let home = tempfile::tempdir().unwrap();
    let (program, args) = this_test("spawn::a_host_that_starts_its_bridge_and_waits");
    let mut host = Running(
        std::process::Command::new(program)
            .args(args)
            .env("KUMI_SPAWN_TEST_HOME", home.path())
            // (the bridge's DAW: this test, which goes on running when the host is killed)
            .env("KUMI_SPAWN_TEST_DAW_PID", std::process::id().to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = host.0.stdout.take().unwrap();
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            if said.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match heard.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) if line.contains("the host is connected") => break,
            Ok(_) => {}
            Err(_) => panic!("the host never connected to the bridge it started"),
        }
    }
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let files: Vec<PathBuf> = std::fs::read_dir(paths.discovery()).unwrap().flatten().map(|entry| entry.path()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let bridge: u32 = std::fs::read_to_string(home.path().join("bridge.pid")).unwrap().parse().unwrap();
    assert!(process_alive(bridge));
    host.0.kill().unwrap();
    host.0.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while (files[0].exists() || process_alive(bridge)) && Instant::now() < deadline {
        std::thread::sleep(ms(50));
    }
    assert!(!files[0].exists(), "the bridge removed its discovery file");
    assert!(!process_alive(bridge), "the bridge ended with its host");
}

#[tokio::test]
#[ignore = "run by a_bridge_ends_with_its_host_when_the_host_is_killed_and_leaves_no_discovery_file"]
async fn a_host_that_starts_its_bridge_and_waits() {
    let home = PathBuf::from(std::env::var_os("KUMI_SPAWN_TEST_HOME").unwrap());
    let (program, args) = this_test("spawn::a_bridge_that_serves_until_its_stdin_ends");
    let times = Times { call: Duration::from_secs(30), serve: Duration::from_secs(30), ..Times::default() };
    let host = Host::new(Daw::Renoise, &home, daw_mcp_server::host::ListeningMode::Standalone)
        .with_spawn(Spawn::new(|| true, spawn::program(program, args)).with_times(times));
    let status = host.status(&anyone()).await;
    assert_eq!(status["connected"], true, "{status}");
    println!("\nthe host is connected");
    std::future::pending::<()>().await;
}

#[tokio::test]
#[ignore = "run by a_bridge_ends_with_its_host_when_the_host_is_killed_and_leaves_no_discovery_file"]
async fn a_bridge_that_serves_until_its_stdin_ends() {
    use tokio::io::AsyncReadExt;
    let home = PathBuf::from(std::env::var_os("KUMI_SPAWN_TEST_HOME").unwrap());
    std::fs::write(home.join("bridge.pid"), std::process::id().to_string()).unwrap();
    let paths = BridgePaths::new(&home, Daw::Renoise);
    let mut config = SimulatorConfig::renoise(read_secret(&paths.secret()).unwrap(), "/lab/Renoise");
    config.identity.pid = std::env::var("KUMI_SPAWN_TEST_DAW_PID").unwrap().parse().unwrap();
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let _ = tokio::io::stdin().read_to_end(&mut Vec::new()).await;
    simulator.stop();
}
