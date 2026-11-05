//! `kumi serve --stdio`: Kumi as a headless runtime for another agent's host (Hermes's `kumi` plugin), over MCP on
//! stdio. The host's model plans; this runs Kumi's typed, bounded operations on the DAW and keeps their HISTORY.
//!
//! It starts in `main`, before anything of the app's runs: Kumi's settings, model keys, sign-in and update check are
//! never read, and nothing of the producer's own Kumi is written. What it keeps goes under `--state-dir` (the host's
//! data for this profile); the DAW bridge's transport files (its secret, discovery and receipts) under `--kumi-home`.
//!
//! The operations it offers are in [`registry`]; [`ctx`] is what each is given; [`mcp`] is the wire.

pub mod change;
pub mod connection;
pub mod ctx;
pub mod history;
pub mod inflight;
pub mod jobs;
pub mod lease;
pub mod mcp;
pub mod read;
pub mod registry;
pub mod setup;
pub mod snapshots;

mod core;

use kumi_types::daw::DawId;
use kumi_types::system::Env;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// The runtime's own protocol, which `status` names: the host refuses another major version before any write.
pub const PROTOCOL: &str = "kumi-serve/1";
/// The MCP protocol version it speaks, as Kumi's own host does.
pub const MCP_PROTOCOL: &str = "2025-11-25";
/// The DAWs a host may ask it to work with.
pub const DAWS: [DawId; 3] = [DawId::Ableton, DawId::Reaper, DawId::Renoise];

const USAGE: &str = "usage: kumi serve --stdio --state-dir <folder> --kumi-home <folder> --daw <ableton|reaper|renoise>
                  [--library-root <folder>]... [--media-root <folder>]... [--tool <name>=<path>]...";

/// What `kumi serve` is started with. Every folder and program is an absolute path: the host resolves them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// Where the runtime keeps what it writes: snapshots, HISTORY, jobs, artifacts, scratch.
    pub state_dir: PathBuf,
    /// The Kumi home the DAW bridge's transport files are in.
    pub kumi_home: PathBuf,
    pub daw: DawId,
    /// The sample folders the producer approved: the only ones the library reads.
    pub library_roots: Vec<PathBuf>,
    /// The host's own media folders (its tools' generated files): only for placing such a file in the DAW.
    pub media_roots: Vec<PathBuf>,
    /// Programs setup selected for optional features (`ffmpeg`, say), by name. Nothing else is looked for or fetched.
    pub tools: BTreeMap<String, PathBuf>,
}

/// The arguments after `serve`.
pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut stdio = false;
    let (mut state_dir, mut kumi_home, mut daw) = (None, None, None);
    let mut parsed = Args {
        state_dir: PathBuf::new(),
        kumi_home: PathBuf::new(),
        daw: DawId::Ableton,
        library_roots: vec![],
        media_roots: vec![],
        tools: BTreeMap::new(),
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |flag: &str| rest.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--stdio" => stdio = true,
            "--state-dir" => state_dir = Some(absolute("--state-dir", &value("--state-dir")?)?),
            "--kumi-home" => kumi_home = Some(absolute("--kumi-home", &value("--kumi-home")?)?),
            "--daw" => {
                let name = value("--daw")?;
                daw = Some(
                    DawId::parse(&name)
                        .filter(|id| DAWS.contains(id))
                        .ok_or_else(|| format!("--daw is ableton, reaper or renoise, not \"{name}\""))?,
                );
            }
            "--library-root" => add_once(&mut parsed.library_roots, absolute("--library-root", &value("--library-root")?)?),
            "--media-root" => add_once(&mut parsed.media_roots, absolute("--media-root", &value("--media-root")?)?),
            "--tool" => {
                let given = value("--tool")?;
                let (name, path) = given.split_once('=').ok_or_else(|| format!("--tool is <name>=<path>, not \"{given}\""))?;
                let named = !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
                if !named {
                    return Err(format!("--tool's name is lowercase letters, digits and -, not \"{name}\""));
                }
                parsed.tools.insert(name.to_string(), absolute("--tool", path)?);
            }
            other => return Err(format!("unknown argument \"{other}\"")),
        }
    }
    if !stdio {
        return Err("it serves on stdio only: --stdio".into());
    }
    parsed.state_dir = state_dir.ok_or("--state-dir is needed")?;
    parsed.kumi_home = kumi_home.ok_or("--kumi-home is needed")?;
    parsed.daw = daw.ok_or("--daw is needed")?;
    Ok(parsed)
}

fn absolute(flag: &str, path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("{flag} is a whole path, not \"{}\"", path.display()))
    }
}

fn add_once(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

/// The environment the runtime and the DAW code it drives read: this process's, with the Kumi home it was given, the
/// update check off (the host moves versions; a runtime never updates itself), and no model key (the host's model
/// plans; the runtime calls none).
pub fn environment(args: &Args, mut env: Env) -> Env {
    env.insert("KUMI_HOME".into(), args.kumi_home.display().to_string());
    env.insert("KUMI_NO_UPDATE_CHECK".into(), "1".into());
    for name in crate::report::KEY_VARIABLES {
        env.remove(name);
    }
    env
}

/// [`environment`] made the process's own, for the parts of Kumi that read it (and the programs it starts). Called
/// before any thread starts.
fn set_process_environment(args: &Args) {
    std::env::set_var("KUMI_HOME", &args.kumi_home);
    std::env::set_var("KUMI_NO_UPDATE_CHECK", "1");
    for name in crate::report::KEY_VARIABLES {
        std::env::remove_var(name);
    }
}

/// Every descriptor the runtime was started with past stdin, stdout and stderr (the host's lock on the state folder,
/// CC-6) is marked close-on-exec: the runtime keeps it open, but no program it starts (a DAW's host) inherits it, so
/// the lock lasts exactly as long as the runtime. The descriptors are the ones `/dev/fd` lists (Linux and macOS).
#[cfg(unix)]
pub fn close_inherited_on_exec() {
    let open: Vec<libc::c_int> = match std::fs::read_dir("/dev/fd") {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str().and_then(|name| name.parse().ok()))
            .filter(|fd| *fd > 2)
            .collect(),
        // No listing: every descriptor a process is likely to have been given.
        Err(_) => (3..1024).collect(),
    };
    for fd in open {
        // SAFETY: fcntl on a descriptor number reads or sets its flags only; one that isn't open fails, untouched.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }
}

/// `kumi serve` with the arguments after `serve`: serves until stdin ends or it's told to stop; the exit code.
pub fn main(args: &[String]) -> i32 {
    // Before anything runs: a panic's message may hold what a call was given (the host's private key among it), so
    // the log says only where it was.
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map_or_else(|| "an unknown place".to_string(), |at| format!("{}:{}", at.file(), at.line()));
        say(&format!("kumi serve: a call failed unexpectedly at {at}"));
    }));
    // Before anything starts a program.
    #[cfg(unix)]
    close_inherited_on_exec();
    let args = match parse(args) {
        Ok(args) => args,
        Err(why) => {
            say(&format!("kumi serve: {why}\n{USAGE}"));
            return 2;
        }
    };
    // While this is the process's only thread: the runtime's blocking threads start with it.
    set_process_environment(&args);
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            say(&format!("kumi serve: no runtime: {error}"));
            return 1;
        }
    };
    let env = environment(&args, kumi_types::system::process_env());
    let local = tokio::task::LocalSet::new();
    let code = local.block_on(&runtime, async move {
        let ctx = match ctx::Ctx::new(args, env, kumi_daw::io::Answers::default()) {
            Ok(ctx) => ctx,
            Err(why) => {
                say(&format!("kumi serve: {why}"));
                return 1;
            }
        };
        match mcp::serve(ctx, tokio::io::stdin(), tokio::io::stdout()).await {
            Ok(()) => 0,
            Err(error) => {
                say(&format!("kumi serve: {error}"));
                1
            }
        }
    });
    // stdin's reader may still wait in its own thread: the runtime doesn't wait for it.
    drop(local);
    runtime.shutdown_timeout(Duration::from_millis(200));
    code
}

/// A line on stderr, for the host's log. Never fails: the host may have closed it.
pub(crate) fn say(text: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn home() -> String {
        std::env::temp_dir().join("kumi-serve-args").display().to_string()
    }

    #[test]
    fn serve_takes_its_folders_daw_roots_and_programs() {
        let (state, kumi_home, root, media, ffmpeg) =
            (home() + "/state", home() + "/home", home() + "/Samples", home() + "/media", home() + "/ffmpeg");
        let parsed = parse(&args(&[
            "--stdio",
            "--state-dir",
            &state,
            "--kumi-home",
            &kumi_home,
            "--daw",
            "renoise",
            "--library-root",
            &root,
            "--library-root",
            &root,
            "--media-root",
            &media,
            "--tool",
            &format!("ffmpeg={ffmpeg}"),
        ]))
        .unwrap();
        assert_eq!((parsed.state_dir, parsed.kumi_home, parsed.daw), (PathBuf::from(state), PathBuf::from(kumi_home), DawId::Renoise));
        assert_eq!(parsed.library_roots, [PathBuf::from(root)], "a root given twice is one root");
        assert_eq!(parsed.media_roots, [PathBuf::from(media)]);
        assert_eq!(parsed.tools.get("ffmpeg"), Some(&PathBuf::from(ffmpeg)));
    }

    #[test]
    fn serve_refuses_what_it_cant_take() {
        let (state, kumi_home) = (home() + "/state", home() + "/home");
        let refused = |words: &[&str]| parse(&args(words)).unwrap_err();
        assert!(refused(&["--state-dir", &state, "--kumi-home", &kumi_home, "--daw", "reaper"]).contains("--stdio"));
        assert!(refused(&["--stdio", "--state-dir", "state", "--kumi-home", &kumi_home, "--daw", "reaper"]).contains("whole path"));
        assert!(refused(&["--stdio", "--state-dir", &state, "--kumi-home", &kumi_home, "--daw", "soundcraft"])
            .contains("ableton, reaper or renoise"));
        assert!(refused(&["--stdio", "--state-dir", &state, "--kumi-home", &kumi_home]).contains("--daw"));
        assert!(refused(&["--stdio", "--state-dir", &state, "--kumi-home", &kumi_home, "--daw", "live"]).contains("\"live\""));
        assert!(refused(&["--stdio", "--state-dir", &state, "--kumi-home", &kumi_home, "--daw", "reaper", "--tool", "ffmpeg"])
            .contains("<name>=<path>"));
        assert!(refused(&["--stdio", "--state-dir", &state, "--kumi-home", &kumi_home, "--daw", "reaper", "--yes"]).contains("unknown"));
        assert!(refused(&["--stdio", "--state-dir"]).contains("needs a value"));
    }

    #[cfg(unix)]
    #[test]
    fn a_descriptor_the_runtime_was_started_with_stays_open_but_isnt_passed_on() {
        use std::os::fd::AsRawFd;
        let lock = tempfile::tempfile().unwrap();
        let fd = lock.as_raw_fd();
        let close_on_exec = || unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC != 0;
        // As a host passes it (Python's pass_fds): inheritable.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::fcntl(fd, libc::F_GETFD) & !libc::FD_CLOEXEC) };
        assert!(!close_on_exec());
        close_inherited_on_exec();
        assert!(close_on_exec(), "a program the runtime starts doesn't get it");
        assert!(lock.metadata().is_ok(), "the runtime keeps it open");
    }

    #[test]
    fn the_runtime_reads_its_own_kumi_home_never_checks_for_updates_and_holds_no_model_key() {
        let parsed =
            parse(&args(&["--stdio", "--state-dir", &(home() + "/state"), "--kumi-home", &(home() + "/home"), "--daw", "ableton"]))
                .unwrap();
        let given = Env::from([
            ("KUMI_HOME".into(), "/elsewhere".into()),
            ("OPENAI_API_KEY".into(), "sk-test".into()),
            ("ANTHROPIC_API_KEY".into(), "sk-test".into()),
            ("PATH".into(), "/bin".into()),
        ]);
        let env = environment(&parsed, given);
        assert_eq!(env.get("KUMI_HOME"), Some(&(home() + "/home")));
        assert_eq!(env.get("KUMI_NO_UPDATE_CHECK").map(String::as_str), Some("1"));
        assert!(!env.contains_key("OPENAI_API_KEY") && !env.contains_key("ANTHROPIC_API_KEY"));
        assert_eq!(env.get("PATH").map(String::as_str), Some("/bin"), "the rest is as given");
    }
}
