//! The real bridge process, with a disposable helper: host EOF cleans up even while a helper request is unanswered;
//! a direct development run remains independent of stdin. No native helper, Logic, or Accessibility is used.
#![cfg(unix)]

use daw_bridge::{files, paths::BridgePaths, Daw};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const HELPER: &str = r#"#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

root = Path(os.environ["KUMI_LOGIC_FIXTURE"])
def marker(name, value):
    path = root / name
    partial = root / (name + ".part")
    partial.write_text(str(value))
    partial.replace(path)

marker("helper-pid", os.getpid())
looks = 0
try:
    for line in sys.stdin:
        request = json.loads(line)
        op = request["op"]
        reply = {"id": request["id"], "ok": True, "ms": 0}
        if op == "quit":
            marker("quit", "asked")
            print(json.dumps(reply), flush=True)
            break
        if op == "apps":
            reply["apps"] = [{"bundle": "com.apple.logic10", "pid": int(os.environ["KUMI_LOGIC_FIXTURE_PID"]),
                              "name": "Logic Pro", "version": "12.3.1", "active": True, "hidden": False,
                              "path": str(root / "No Logic.app")}]
        elif op == "trusted":
            reply["trusted"] = True
        elif op == "windows":
            if (root / "stall").exists():
                marker("stalled", "windows")
                continue
            looks += 1
            reply["windows"] = [{"title": "Fixture - Tracks", "role": "AXWindow", "subrole": "AXStandardWindow",
                                 "main": True, "focused": True, "document": "file:///fixture/Fixture.logicx"}]
        print(json.dumps(reply), flush=True)
        marker("looks", looks)
finally:
    marker("helper-ended", "done")
"#;

struct Run {
    child: Child,
    paths: BridgePaths,
    fixture: tempfile::TempDir,
}

impl Run {
    fn start(hosted: bool) -> Self {
        let fixture = tempfile::tempdir().unwrap();
        let paths = BridgePaths::new(&fixture.path().join("kumi"), Daw::Logic);
        files::ensure_secret(&paths.secret()).unwrap();
        let reference = fixture.path().join("reference.json");
        fs::write(&reference, files::Reference::new(Daw::Logic, &paths).to_json()).unwrap();
        let helper = fixture.path().join("helper with spaces");
        fs::write(&helper, HELPER).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_logic-kumi"));
        command
            .arg("--kumi-home")
            .arg(fixture.path().join("kumi"))
            .arg("--helper")
            .arg(helper)
            .arg("--reference")
            .arg(reference)
            .env("KUMI_LOGIC_FIXTURE", fixture.path())
            .env("KUMI_LOGIC_FIXTURE_PID", std::process::id().to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if hosted {
            command.arg("--hosted");
        }
        let child = command.spawn().unwrap();
        let mut run = Self { child, paths, fixture };
        run.wait("the fixture project in discovery", |run| {
            files::scan_discovery(&run.paths.discovery(), Daw::Logic).into_iter().any(|found| {
                found.discovery.is_ok_and(|discovery| discovery.project.is_some_and(|project| project.name.as_deref() == Some("Fixture")))
            })
        });
        run
    }

    fn marker(&self, name: &str) -> PathBuf {
        self.fixture.path().join(name)
    }

    fn looks(&self) -> u32 {
        fs::read_to_string(self.marker("looks")).ok().and_then(|text| text.parse().ok()).unwrap_or(0)
    }

    fn wait(&mut self, what: &str, mut reached: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !reached(self) {
            assert!(self.child.try_wait().unwrap().is_none(), "bridge exited before {what}");
            assert!(Instant::now() < deadline, "no {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn ended_cleanly(&mut self) {
        self.wait("bridge exit", |run| run.child.try_wait().unwrap().is_some());
        assert!(self.child.wait().unwrap().success());
        assert!(fs::read_dir(self.paths.discovery()).unwrap().next().is_none(), "the actual discovery file was removed");
        assert_eq!(fs::read_to_string(self.marker("quit")).unwrap(), "asked", "the helper was asked to quit");
        assert!(self.marker("helper-ended").is_file());
        let pid = fs::read_to_string(self.marker("helper-pid")).unwrap().parse().unwrap();
        assert!(!files::process_alive(pid), "the helper was reaped before the bridge ended");
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[test]
fn a_hosted_process_cleans_discovery_and_its_helper_when_stdin_ends() {
    for stalled in [false, true] {
        let mut run = Run::start(true);
        if stalled {
            fs::write(run.marker("stall"), "stall the next windows request").unwrap();
            run.wait("the unanswered helper request", |run| run.marker("stalled").is_file());
        }
        drop(run.child.stdin.take());
        run.ended_cleanly();
    }
}

#[test]
fn a_development_process_keeps_serving_after_stdin_eof_until_signalled() {
    let mut run = Run::start(false);
    drop(run.child.stdin.take());
    let before = run.looks();
    run.wait("two further project looks after EOF", |run| run.looks() >= before + 2);
    assert!(run.child.try_wait().unwrap().is_none());
    // SAFETY: this is the child process this test owns and has just confirmed is still running.
    assert_eq!(unsafe { libc::kill(run.child.id() as libc::pid_t, libc::SIGTERM) }, 0);
    run.ended_cleanly();
}
