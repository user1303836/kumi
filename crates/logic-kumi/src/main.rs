//! `logic-kumi`: the arguments, the files it finds (the helper, the reference), its log and the signals that stop it;
//! the run itself is [`logic_kumi::bridge::run`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::atomic::Ordering;
use std::sync::Arc;

use daw_bridge::files::Reference;
use daw_bridge::paths::{self, BridgePaths};
use daw_bridge::Daw;
use logic_kumi::bridge::{self, Config, Outcome};
use logic_kumi::helper::{ChildLauncher, Supervised};
use logic_kumi::{log, BRIDGE_VERSION};

/// The reference file beside the binary, as the Renoise tool's is beside it.
const REFERENCE: &str = "bridge-reference.json";
/// The helper's name, with the architecture it was built for after a dash.
const HELPER_PREFIX: &str = "kumi-logic-hands";
const HELPER_ENV: &str = "KUMI_LOGIC_HANDS";

const USAGE: &str = "\
usage: logic-kumi [--kumi-home <dir>] [--helper <path>] [--reference <path>]
       logic-kumi --version | --help

Kumi's Logic Pro bridge: finds the running Logic Pro through its helper, serves Kumi's bridge on 127.0.0.1
(ports 9790-9799) and leaves when Logic does.

  --kumi-home <dir>    Kumi's folder (default: KUMI_HOME, else ~/.kumi)
  --helper <path>      the kumi-logic-hands helper (default: KUMI_LOGIC_HANDS, else a kumi-logic-hands-* file
                       beside this binary, else target/logic-hands/ in the checkout)
  --reference <path>   the reference file naming the secret, the discovery and log folders and the first port
                       (default: bridge-reference.json beside this binary; without one, a development run uses
                       Kumi's folder directly)
";

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    kumi_home: Option<PathBuf>,
    helper: Option<PathBuf>,
    reference: Option<PathBuf>,
    version: bool,
    help: bool,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().filter(|value| !value.is_empty()).map(PathBuf::from).ok_or(format!("{name} needs a path"));
        match arg.as_str() {
            "--kumi-home" => parsed.kumi_home = Some(value("--kumi-home")?),
            "--helper" => parsed.helper = Some(value("--helper")?),
            "--reference" => parsed.reference = Some(value("--reference")?),
            "--version" | "-V" => parsed.version = true,
            "--help" | "-h" => parsed.help = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(parsed)
}

/// The architecture a helper built for this machine names itself by.
const ARCHES: &[&str] = if cfg!(target_arch = "aarch64") { &["arm64", "aarch64"] } else { &["x86_64", "amd64"] };

/// A `kumi-logic-hands` or `kumi-logic-hands-*` file in `dir`: the one named for this architecture when there are
/// several, else the first by name.
fn hands_in(dir: &Path) -> Option<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return None };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| !kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            name == HELPER_PREFIX || name.strip_prefix(HELPER_PREFIX).is_some_and(|rest| rest.starts_with('-') && !rest.contains('.'))
        })
        .collect();
    found.sort();
    found
        .iter()
        .find(|path| {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            ARCHES.iter().any(|arch| name.ends_with(&format!("-{arch}")))
        })
        .or(found.first())
        .cloned()
}

/// The helper: named, or from the environment, or found by its name beside the binary or in the checkout's build
/// folder.
fn find_helper(named: Option<PathBuf>, env: Option<String>, beside: Option<&Path>, checkout: &Path) -> Option<PathBuf> {
    if let Some(path) = named {
        return Some(path);
    }
    if let Some(path) = env.filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    beside.and_then(hands_in).or_else(|| hands_in(&checkout.join("target").join("logic-hands")))
}

/// SIGINT, SIGTERM and SIGHUP set `stop`: the run says goodbye to its hosts and removes its discovery file.
#[cfg(unix)]
fn stop_on_signals(stop: Arc<AtomicBool>) {
    static SIGNALLED: AtomicBool = AtomicBool::new(false);
    extern "C" fn on_signal(_signal: libc::c_int) {
        SIGNALLED.store(true, Ordering::Release);
    }
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // A handler that only sets a flag: nothing else is safe in one.
        unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
    }
    std::thread::spawn(move || loop {
        if SIGNALLED.load(Ordering::Acquire) {
            stop.store(true, Ordering::Release);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    });
}

#[cfg(not(unix))]
fn stop_on_signals(_stop: Arc<AtomicBool>) {}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(why) => {
            eprintln!("logic-kumi: {why}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.version {
        println!("logic-kumi {BRIDGE_VERSION}");
        return ExitCode::SUCCESS;
    }
    if args.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let Some(kumi_home) = args.kumi_home.or_else(paths::kumi_home) else {
        eprintln!("logic-kumi: no home folder: name Kumi's folder with --kumi-home");
        return ExitCode::from(2);
    };
    let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    let bridge_paths = BridgePaths::new(&kumi_home, Daw::Logic);
    // A reference named on the command line must be there; the one beside the binary is used when it is.
    let reference = match &args.reference {
        Some(path) => Some(path.clone()),
        None => exe_dir.as_ref().map(|dir| dir.join(REFERENCE)).filter(|path| path.is_file()),
    };
    let config = match reference {
        Some(path) => match Reference::read(&path) {
            Ok(reference) if reference.daw == Daw::Logic.id() => Config::from_reference(&reference),
            Ok(reference) => {
                eprintln!("logic-kumi: {} is a reference for {}, not Logic Pro", path.display(), reference.daw);
                return ExitCode::from(2);
            }
            Err(error) => {
                eprintln!("logic-kumi: {} isn't a reference file: {error}", path.display());
                return ExitCode::from(2);
            }
        },
        None => match Config::development(&kumi_home) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("logic-kumi: couldn't set up a development run in {}: {error}", kumi_home.display());
                return ExitCode::from(2);
            }
        },
    };
    log::set_dir(Some(config.server.log_dir.clone().unwrap_or_else(|| bridge_paths.logs())));
    // The checkout this bridge was built in, where a development run finds its helper. Cargo gives `<checkout>/.`
    // (.cargo/config.toml): its components drop the `.`.
    let checkout: PathBuf = Path::new(env!("KUMI_WORKSPACE")).components().collect();
    let Some(helper) = find_helper(args.helper, std::env::var(HELPER_ENV).ok(), exe_dir.as_deref(), &checkout) else {
        log::line("no helper: build kumi-logic-hands (python3 scripts/build-logic-hands.py), or name it with --helper or KUMI_LOGIC_HANDS");
        return ExitCode::from(2);
    };
    if !helper.is_file() {
        log::line(&format!("the helper {} isn't a file", helper.display()));
        return ExitCode::from(2);
    }
    log::line(&format!(
        "logic-kumi {BRIDGE_VERSION}: helper {}, secret {}, discovery {}, log {}",
        helper.display(),
        config.server.secret_file.display(),
        config.server.discovery_dir.display(),
        log::file().map(|file| file.display().to_string()).unwrap_or_else(|| "stderr".into())
    ));
    let stop = Arc::new(AtomicBool::new(false));
    stop_on_signals(stop.clone());
    let supervised = Supervised::new(Box::new(ChildLauncher { path: helper }));
    match bridge::run(config, Box::new(supervised), stop) {
        Outcome::LogicQuit => log::line("Logic Pro quit; the bridge is done"),
        Outcome::Stopped => log::line("stopped"),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn arguments_name_the_home_the_helper_and_the_reference() {
        let args = parse_args(strings(&["--kumi-home", "/k", "--helper", "/h/hands", "--reference", "/r.json"])).unwrap();
        assert_eq!(
            args,
            Args {
                kumi_home: Some("/k".into()),
                helper: Some("/h/hands".into()),
                reference: Some("/r.json".into()),
                version: false,
                help: false
            }
        );
        assert!(parse_args(strings(&["--version"])).unwrap().version);
        assert!(parse_args(strings(&["-h"])).unwrap().help);
        assert!(parse_args(strings(&["--helper"])).is_err());
        assert!(parse_args(strings(&["--port", "1"])).is_err());
    }

    #[test]
    fn the_helper_is_named_or_found_by_its_name_for_this_architecture() {
        let dir = tempfile::tempdir().unwrap();
        let beside = dir.path().join("bin");
        let checkout = dir.path().join("checkout");
        std::fs::create_dir_all(&beside).unwrap();
        std::fs::create_dir_all(checkout.join("target/logic-hands")).unwrap();
        assert_eq!(find_helper(Some("/named".into()), Some("/env".into()), Some(&beside), &checkout), Some(PathBuf::from("/named")));
        assert_eq!(find_helper(None, Some("/env".into()), Some(&beside), &checkout), Some(PathBuf::from("/env")));
        assert_eq!(find_helper(None, Some(String::new()), Some(&beside), &checkout), None);
        // Beside the binary first, then the checkout's build folder; a file for another architecture only when
        // there's none for this one; the helper's build products with extensions are not helpers.
        let other = if cfg!(target_arch = "aarch64") { "x86_64" } else { "arm64" };
        std::fs::write(checkout.join("target/logic-hands").join(format!("kumi-logic-hands-{other}")), "").unwrap();
        std::fs::write(checkout.join("target/logic-hands/kumi-logic-hands-arm64.dSYM"), "").unwrap();
        let found = find_helper(None, None, Some(&beside), &checkout).unwrap();
        assert_eq!(found.file_name().unwrap().to_string_lossy(), format!("kumi-logic-hands-{other}"));
        std::fs::write(beside.join(format!("kumi-logic-hands-{other}")), "").unwrap();
        std::fs::write(beside.join(format!("kumi-logic-hands-{}", ARCHES[0])), "").unwrap();
        std::fs::write(beside.join("kumi-logic-hands.txt"), "").unwrap();
        let found = find_helper(None, None, Some(&beside), &checkout).unwrap();
        assert_eq!(found, beside.join(format!("kumi-logic-hands-{}", ARCHES[0])));
        assert_eq!(find_helper(None, None, None, &dir.path().join("nowhere")), None);
    }
}
