//! Keeping the computer awake while Kumi works (an answer, a /goal, a /loop or sound match, a render, a wait
//! for a usage limit to reset), so a producer who walks away comes back to finished work. The display may
//! sleep; the system doesn't. Released whenever Kumi rests, and by the system if Kumi ends.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use kumi_agent::core::contracts::{SessionEvent, TurnState};

/// The name the system shows for Kumi's hold (`pmset -g assertions` on a Mac).
pub const REASON: &str = "Kumi is working";

/// The system's hold on sleep: taken and given back.
pub trait SleepGuard {
    /// True when the system took it.
    fn acquire(&self) -> bool;
    fn release(&self);
}

/// Holds the computer awake while anything Kumi does needs it: each holder is named ("answer"), and the hold
/// goes once none is left.
pub struct KeepAwake {
    enabled: bool,
    guard: Rc<dyn SleepGuard>,
    holders: RefCell<BTreeSet<&'static str>>,
    held: RefCell<bool>,
    /// The background jobs running now, by id: the "jobs" holder stays while any does.
    jobs: RefCell<BTreeSet<u32>>,
}

impl KeepAwake {
    pub fn new(enabled: bool, guard: Rc<dyn SleepGuard>) -> Self {
        Self { enabled, guard, holders: RefCell::default(), held: RefCell::new(false), jobs: RefCell::default() }
    }

    /// The system's own hold (an IOKit assertion, the thread's execution state, systemd-inhibit).
    pub fn system(enabled: bool) -> Self {
        Self::new(enabled, Rc::new(SystemGuard::default()))
    }

    pub fn hold(&self, holder: &'static str) {
        self.holders.borrow_mut().insert(holder);
        self.update();
    }

    pub fn release(&self, holder: &'static str) {
        self.holders.borrow_mut().remove(holder);
        self.update();
    }

    /// Whether the system's hold is taken now.
    pub fn is_held(&self) -> bool {
        *self.held.borrow()
    }

    /// Follows the session: held while an answer (or a run of them) is under way, or a background job runs, released
    /// when Kumi rests.
    pub fn follow(&self, event: &SessionEvent) {
        match event {
            SessionEvent::State { state } => match state {
                TurnState::Running | TurnState::Cancelling => self.hold("answer"),
                TurnState::Idle | TurnState::Closed => self.release("answer"),
            },
            SessionEvent::Job(status) => {
                let none = {
                    let mut jobs = self.jobs.borrow_mut();
                    if status.state.ended() {
                        jobs.remove(&status.id);
                    } else {
                        jobs.insert(status.id);
                    }
                    jobs.is_empty()
                };
                if none {
                    self.release("jobs");
                } else {
                    self.hold("jobs");
                }
            }
            _ => {}
        }
    }

    fn update(&self) {
        let want = self.enabled && !self.holders.borrow().is_empty();
        let mut held = self.held.borrow_mut();
        if want && !*held {
            *held = self.guard.acquire();
        } else if !want && *held {
            self.guard.release();
            *held = false;
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        if *self.held.borrow() {
            self.guard.release();
        }
    }
}

/// The system's hold on this platform.
#[derive(Default)]
pub struct SystemGuard {
    taken: RefCell<Option<imp::Hold>>,
}

impl SleepGuard for SystemGuard {
    fn acquire(&self) -> bool {
        let mut taken = self.taken.borrow_mut();
        if taken.is_none() {
            *taken = imp::Hold::take(REASON);
        }
        taken.is_some()
    }

    fn release(&self) {
        if let Some(hold) = self.taken.borrow_mut().take() {
            hold.give_back();
        }
    }
}

/// An IOKit power assertion: the system ends it if Kumi ends.
#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void, CString};

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const LEVEL_ON: u32 = 255;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(kind: CFStringRef, level: u32, name: CFStringRef, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator: *const c_void, text: *const c_char, encoding: u32) -> CFStringRef;
        fn CFRelease(object: *const c_void);
    }

    pub struct Hold(u32);

    impl Hold {
        pub fn take(reason: &str) -> Option<Hold> {
            let kind = CString::new("PreventUserIdleSystemSleep").ok()?;
            let name = CString::new(reason).ok()?;
            // SAFETY: both strings are created here and released after the call; the id is written by IOKit.
            unsafe {
                let kind = CFStringCreateWithCString(std::ptr::null(), kind.as_ptr(), UTF8);
                let name = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), UTF8);
                if kind.is_null() || name.is_null() {
                    for string in [kind, name].into_iter().filter(|s| !s.is_null()) {
                        CFRelease(string);
                    }
                    return None;
                }
                let mut id = 0u32;
                let result = IOPMAssertionCreateWithName(kind, LEVEL_ON, name, &mut id);
                CFRelease(kind);
                CFRelease(name);
                (result == 0).then_some(Hold(id))
            }
        }

        pub fn give_back(self) {
            // SAFETY: the id came from IOPMAssertionCreateWithName and is released once.
            unsafe {
                IOPMAssertionRelease(self.0);
            }
        }
    }
}

/// The calling thread's execution state: the session's thread holds and lets go of it (KeepAwake lives there, with the
/// session), and Windows ends it with that thread.
#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};

    pub struct Hold;

    impl Hold {
        pub fn take(_reason: &str) -> Option<Hold> {
            // SAFETY: a plain call with flags; it returns 0 when it fails.
            (unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) } != 0).then_some(Hold)
        }

        pub fn give_back(self) {
            // SAFETY: as above.
            unsafe {
                SetThreadExecutionState(ES_CONTINUOUS);
            }
        }
    }
}

/// systemd-inhibit around a program that ends with Kumi (`tail --pid`), so the hold can't outlive it; nothing where
/// systemd-inhibit isn't installed.
#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::process::{Child, Command, Stdio};

    pub struct Hold(Child);

    impl Hold {
        pub fn take(reason: &str) -> Option<Hold> {
            let pid = std::process::id().to_string();
            Command::new("systemd-inhibit")
                .args(["--what=idle:sleep", "--who=Kumi", &format!("--why={reason}"), "--mode=block"])
                .args(["tail", &format!("--pid={pid}"), "-f", "/dev/null"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
                .map(Hold)
        }

        pub fn give_back(mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub struct Hold;

    impl Hold {
        pub fn take(_reason: &str) -> Option<Hold> {
            None
        }

        pub fn give_back(self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct Fake {
        taken: Cell<u32>,
        given: Cell<u32>,
        refuse: bool,
    }

    impl SleepGuard for Fake {
        fn acquire(&self) -> bool {
            self.taken.set(self.taken.get() + 1);
            !self.refuse
        }
        fn release(&self) {
            self.given.set(self.given.get() + 1);
        }
    }

    fn state(state: TurnState) -> SessionEvent {
        SessionEvent::State { state }
    }

    #[test]
    fn held_while_an_answer_runs_and_released_when_kumi_rests() {
        let fake = Rc::new(Fake::default());
        let awake = KeepAwake::new(true, fake.clone());
        awake.follow(&state(TurnState::Running));
        awake.follow(&state(TurnState::Running));
        awake.follow(&state(TurnState::Cancelling));
        assert!(awake.is_held());
        assert_eq!(fake.taken.get(), 1, "one hold however many times it's asked for");
        awake.follow(&state(TurnState::Idle));
        assert!(!awake.is_held());
        assert_eq!(fake.given.get(), 1);
        awake.follow(&state(TurnState::Idle));
        assert_eq!(fake.given.get(), 1);
    }

    #[test]
    fn each_holder_keeps_it_until_the_last_lets_go() {
        let fake = Rc::new(Fake::default());
        let awake = KeepAwake::new(true, fake.clone());
        awake.hold("answer");
        awake.hold("jobs");
        awake.release("answer");
        assert!(awake.is_held());
        awake.release("jobs");
        assert!(!awake.is_held());
        assert_eq!((fake.taken.get(), fake.given.get()), (1, 1));
    }

    #[test]
    fn held_while_any_background_job_runs_after_the_answer_ends() {
        use kumi_agent::core::jobs::{JobKind, JobState, JobStatus, WhenDone};
        let job = |id: u32, state: JobState| {
            SessionEvent::Job(JobStatus {
                id,
                kind: JobKind::Worker,
                title: "research".into(),
                state,
                when_done: WhenDone::Tell,
                doing: None,
                tool: None,
                started_at: 0,
                ended_at: None,
                steps: 0,
                sources: 0,
                usage: None,
                line: None,
                moved: false,
            })
        };
        let fake = Rc::new(Fake::default());
        let awake = KeepAwake::new(true, fake.clone());
        awake.follow(&state(TurnState::Running));
        awake.follow(&job(1, JobState::Running));
        awake.follow(&job(2, JobState::Running));
        awake.follow(&state(TurnState::Idle));
        assert!(awake.is_held(), "the jobs keep it");
        awake.follow(&job(1, JobState::Done));
        assert!(awake.is_held(), "job 2 still runs");
        awake.follow(&job(2, JobState::Stopped));
        assert!(!awake.is_held());
        assert_eq!((fake.taken.get(), fake.given.get()), (1, 1));
    }

    #[test]
    fn off_in_settings_it_never_holds_and_nothing_outlives_it() {
        let fake = Rc::new(Fake::default());
        let awake = KeepAwake::new(false, fake.clone());
        awake.follow(&state(TurnState::Running));
        assert!(!awake.is_held());
        assert_eq!(fake.taken.get(), 0);
        let fake = Rc::new(Fake::default());
        {
            let awake = KeepAwake::new(true, fake.clone());
            awake.hold("answer");
        }
        assert_eq!(fake.given.get(), 1, "given back when Kumi lets go of it");
    }

    /// The system's own hold, as `pmset -g assertions` lists it: there while held, gone after. Opt-in (it takes a real
    /// assertion): `cargo test -p kumi --lib awake -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_system_hold_is_named_while_held_and_gone_after() {
        let listed = || {
            let output = std::process::Command::new("pmset").args(["-g", "assertions"]).output().expect("pmset");
            String::from_utf8_lossy(&output.stdout).lines().filter(|line| line.contains(REASON)).map(str::to_string).collect::<Vec<_>>()
        };
        let awake = KeepAwake::system(true);
        awake.hold("answer");
        assert!(awake.is_held());
        let held = listed();
        assert!(held.iter().any(|line| line.contains("PreventUserIdleSystemSleep")), "{held:?}");
        awake.release("answer");
        assert!(listed().is_empty(), "{:?}", listed());
    }

    #[test]
    fn a_hold_the_system_refused_is_asked_for_again_next_time() {
        let fake = Rc::new(Fake { refuse: true, ..Default::default() });
        let awake = KeepAwake::new(true, fake.clone());
        awake.hold("answer");
        assert!(!awake.is_held());
        awake.release("answer");
        assert_eq!(fake.given.get(), 0, "nothing to give back");
        awake.hold("answer");
        assert_eq!(fake.taken.get(), 2);
    }
}
