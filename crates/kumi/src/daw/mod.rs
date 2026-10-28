//! The DAWs this Kumi works with (the DAW table, in `registry`), and the one this run works with: what the app says of
//! the DAW and shows of its tools, its focus and its devices comes from here, and each DAW's provider finds its bridge
//! and makes the session's integration.
pub mod registry;

use kumi_agent::core::{presentation::ToolPresentation, view::ViewPresentation};
use kumi_daw::provider::DawProvider;
use kumi_types::{
    daw::{DawId, DawProfile},
    system::Env,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Each registered DAW's provider, in the DAW table's order: Live's first, then the DAWs whose bridges are Kumi's own.
pub fn providers() -> impl Iterator<Item = &'static dyn DawProvider> + Clone {
    registry::DAWS.iter().map(|daw| *daw as &'static dyn DawProvider)
}

/// A registered DAW's provider; None for one this Kumi doesn't integrate.
pub fn provider(id: DawId) -> Option<&'static dyn DawProvider> {
    providers().into_iter().find(|provider| provider.id() == id)
}

/// The DAW a session works with, and where Kumi's bridge for it says how to reach it:
/// 1. the one asked for (with --daw or KUMI_DAW);
/// 2. the DAW open now, whether or not its bridge is in yet (first-run setup puts it in): with several open, as
///    [`open_among`] chooses;
/// 3. with none open, the only DAW whose bridge is installed (a producer who uses Live alone starts in Live, as always);
/// 4. with several installed: the one used last, else Live, else the first;
/// 5. with none installed, the DAW on this computer, for first-run setup to set up: the only one here, else the one
///    the producer picked last, else Live, else the first (setup asks which, `to_ask`); with none here, Live's words.
pub fn chosen(asked: Option<DawId>, env: &Env) -> (&'static dyn DawProvider, Option<String>) {
    chosen_with(asked, env, &|provider| provider.running_now(&processes_in(env)))
}

/// `chosen`, with whether each DAW runs now given (a test's answer, or one process listing each).
pub fn chosen_with(
    asked: Option<DawId>,
    env: &Env,
    running: &dyn Fn(&dyn DawProvider) -> bool,
) -> (&'static dyn DawProvider, Option<String>) {
    chosen_among(asked, env, running, &|provider| provider.here(env))
}

/// The DAWs on this computer whose bridge isn't installed yet, as `here` says.
fn found_here(env: &Env, here: &dyn Fn(&dyn DawProvider) -> bool) -> Vec<&'static dyn DawProvider> {
    if providers().into_iter().any(|provider| provider.bridge_config(env).is_some()) {
        return vec![];
    }
    providers().into_iter().filter(|provider| here(*provider)).collect()
}

/// The DAWs a first run asks the producer to choose among ("Which DAW do you work in?"): with no bridge installed,
/// several DAWs here, none of them picked before, and none open (the one open is the answer).
pub fn to_ask(env: &Env, here: &dyn Fn(&dyn DawProvider) -> bool, running: &dyn Fn(&dyn DawProvider) -> bool) -> Vec<DawId> {
    if providers().into_iter().any(running) {
        return vec![];
    }
    let found = found_here(env, here);
    let picked = last_used(env);
    if found.len() < 2 || found.iter().any(|provider| Some(provider.id()) == picked) {
        return vec![];
    }
    found.into_iter().map(|provider| provider.id()).collect()
}

/// `chosen_with`, with whether each DAW is on this computer given too.
pub fn chosen_among(
    asked: Option<DawId>,
    env: &Env,
    running: &dyn Fn(&dyn DawProvider) -> bool,
    here: &dyn Fn(&dyn DawProvider) -> bool,
) -> (&'static dyn DawProvider, Option<String>) {
    if let Some(provider) = asked.and_then(provider) {
        return (provider, provider.bridge_config(env));
    }
    let open: Vec<&'static dyn DawProvider> = providers().into_iter().filter(|provider| running(*provider)).collect();
    if let Some(provider) = open_among(&open, env) {
        return (provider, provider.bridge_config(env));
    }
    let installed: Vec<(&'static dyn DawProvider, String)> =
        providers().into_iter().filter_map(|provider| provider.bridge_config(env).map(|config| (provider, config))).collect();
    let pick = |provider: &'static dyn DawProvider, config: &str| (provider, Some(config.to_string()));
    match installed.as_slice() {
        [] => {
            let found = found_here(env, here);
            let picked = last_used(env);
            let first = match found.as_slice() {
                [] => provider(DawId::Ableton).expect("Live is registered"),
                [one] => *one,
                several => several
                    .iter()
                    .find(|provider| Some(provider.id()) == picked)
                    .or_else(|| several.iter().find(|provider| provider.id() == DawId::Ableton))
                    .copied()
                    .unwrap_or(several[0]),
            };
            (first, None)
        }
        [(provider, config)] => pick(*provider, config),
        several => {
            let last = last_used(env);
            let found = several
                .iter()
                .find(|(provider, _)| Some(provider.id()) == last)
                .or_else(|| several.iter().find(|(provider, _)| provider.id() == DawId::Ableton))
                .unwrap_or(&several[0]);
            pick(found.0, &found.1)
        }
    }
}

/// The other DAWs open beside `chosen`: a session that chose among them says so, and how to switch.
pub fn also_running(chosen: DawId, running: &dyn Fn(&dyn DawProvider) -> bool) -> Vec<&'static dyn DawProvider> {
    providers().into_iter().filter(|provider| provider.id() != chosen && running(*provider)).collect()
}

/// Among the DAWs open now, the one Kumi works with: the only one; else, of those whose bridge is in (all of them when
/// none's is), the one used last, else Live, else the first.
pub fn open_among(open: &[&'static dyn DawProvider], env: &Env) -> Option<&'static dyn DawProvider> {
    match open {
        [] => None,
        [one] => Some(*one),
        several => {
            let bridged: Vec<&'static dyn DawProvider> =
                several.iter().copied().filter(|provider| provider.bridge_config(env).is_some()).collect();
            let among = if bridged.is_empty() { several.to_vec() } else { bridged };
            let last = last_used(env);
            among
                .iter()
                .find(|provider| Some(provider.id()) == last)
                .or_else(|| among.iter().find(|provider| provider.id() == DawId::Ableton))
                .or(among.first())
                .copied()
        }
    }
}

/// The DAW Kumi follows the producer to while its session isn't connected (none asked for): the one open instead of the
/// session's own, when that one isn't running and another is, or has no bridge in while a DAW whose bridge is in runs.
/// Chosen as a start chooses among the DAWs open ([`open_among`]), so Kumi, opened again, starts with it.
pub fn to_follow(current: DawId, env: &Env, running: &dyn Fn(&dyn DawProvider) -> bool) -> Option<&'static dyn DawProvider> {
    let open: Vec<&'static dyn DawProvider> = providers().into_iter().filter(|provider| running(*provider)).collect();
    let follows = if open.iter().any(|provider| provider.id() == current) {
        provider(current).is_some_and(|provider| provider.bridge_config(env).is_none())
            && open.iter().any(|provider| provider.id() != current && provider.bridge_config(env).is_some())
    } else {
        !open.is_empty()
    };
    open_among(&open, env).filter(|provider| follows && provider.id() != current)
}

/// A test's stand-in for which DAWs run at each look of the session's watch for a DAW the producer opened, in place of
/// listing this computer's processes.
#[derive(Clone)]
pub struct RunningDaws(pub Rc<dyn Fn() -> Vec<DawId>>);

/// What Kumi says while no DAW is open and the producer works with it in several (their bridges are in): which to open,
/// rather than how to make one answer. None with fewer than two (each DAW's own words say it).
pub fn open_one_words(env: &Env) -> Option<String> {
    let names: Vec<&str> =
        providers().into_iter().filter(|provider| provider.bridge_installed(env)).map(|provider| provider.profile().words.app).collect();
    let (last, rest) = names.split_last()?;
    if rest.is_empty() {
        return None;
    }
    Some(format!("No DAW is open yet: open {} or {last}, and Kumi connects to the one you open.", rest.join(", ")))
}

/// What a session says when it chose `chosen` while `others` run too.
pub fn also_running_words(chosen: DawId, others: &[&'static dyn DawProvider], env: &Env) -> Option<String> {
    let first = others.first()?;
    let names: Vec<&str> = others.iter().map(|provider| provider.profile().words.app).collect();
    let chosen_name = provider(chosen)?.profile().words.app;
    let why = if last_used(env) == Some(chosen) { ", the one you used last" } else { "" };
    let (open, switch) = match names.as_slice() {
        [one] => (format!("{one} is open too"), format!("/daw {} switches to {one}", command_name(first.id()))),
        many => (format!("{} are open too", many.join(" and ")), "/daw <name> switches".into()),
    };
    Some(format!("{open}: Kumi works with {chosen_name}{why}. {switch}."))
}

/// The processes running now (pid and name), from one listing (`ps`, `tasklist` on Windows) that every DAW's
/// `running_now` matches against; a listing is reused for a moment, then made again. KUMI_PROCESSES stands in for it
/// (the isolated test runner sets it to `none`: no test sees a DAW running on the computer it runs on).
pub fn processes() -> Rc<Vec<(u32, String)>> {
    daw_bridge::detect::processes(&|name| std::env::var(name).ok())
}

/// [`processes`], with KUMI_PROCESSES read from a session's environment first.
pub fn processes_in(env: &Env) -> Rc<Vec<(u32, String)>> {
    daw_bridge::detect::processes(&|name| env.get(name).cloned())
}

/// How many process listings this Kumi has made (`processes`).
pub fn process_listings() -> u64 {
    daw_bridge::detect::listings()
}

/// A DAW as `/daw` takes it: "live", "reaper", "renoise".
pub fn command_name(id: DawId) -> String {
    provider(id).map(|provider| provider.profile().words.app.to_lowercase()).unwrap_or_else(|| id.as_str().into())
}

/// What `/daw` does: says which DAW Kumi works with, or opens Kumi again with another.
#[derive(Debug, PartialEq, Eq)]
pub enum DawCommand {
    Say(String),
    Switch(DawId, String),
}

/// `/daw` with `typed` after it ("", "renoise", "live").
pub fn daw_command(typed: &str) -> DawCommand {
    match HERE.with(|here| here.borrow().clone()) {
        Some(here) => daw_command_among(typed, &|provider| here(provider.id())),
        None => {
            let env = kumi_types::system::process_env();
            daw_command_among(typed, &|provider| provider.here(&env) || provider.bridge_installed(&env))
        }
    }
}

/// Whether a DAW is on this computer.
pub type Here = Rc<dyn Fn(DawId) -> bool>;

thread_local! {
    /// Which DAWs `/daw` offers this run, as the session says ([`offer_here`]).
    static HERE: RefCell<Option<Here>> = const { RefCell::new(None) };
}

/// Which DAWs are on this computer for `/daw` to offer, this run: by the session's environment, or a test's answer.
pub fn offer_here(here: Here) {
    HERE.with(|offered| *offered.borrow_mut() = Some(here));
}

/// [`daw_command`], offering the other DAWs `here` says are on this computer (any can still be asked for by name).
pub fn daw_command_among(typed: &str, here: &dyn Fn(&dyn DawProvider) -> bool) -> DawCommand {
    let current = current().id();
    let app = |id: DawId| provider(id).map(|provider| provider.profile().words.app).unwrap_or_default();
    let ids: Vec<DawId> = providers().map(|provider| provider.id()).collect();
    let word = typed.trim().to_lowercase();
    if word.is_empty() {
        let others: Vec<DawId> =
            providers().into_iter().filter(|provider| provider.id() != current && here(*provider)).map(|provider| provider.id()).collect();
        if others.is_empty() {
            return DawCommand::Say(format!("Kumi works with {}, the only DAW it works with on this computer.", app(current)));
        }
        let commands: Vec<String> = others.iter().map(|id| format!("/daw {}", command_name(*id))).collect();
        let names: Vec<&str> = others.iter().map(|id| app(*id)).collect();
        return DawCommand::Say(format!(
            "Kumi works with {}. {} opens Kumi again with {}.",
            app(current),
            commands.join(" or "),
            names.join(" or ")
        ));
    }
    let Some(asked) = ids.iter().copied().find(|id| command_name(*id) == word || id.as_str() == word) else {
        let commands: Vec<String> = ids.iter().map(|id| format!("/daw {}", command_name(*id))).collect();
        return DawCommand::Say(format!("Kumi doesn't know {}: {}.", typed.trim(), commands.join(", ")));
    };
    if asked == current {
        return DawCommand::Say(format!("Kumi works with {} already.", app(current)));
    }
    DawCommand::Switch(asked, format!("Kumi closes and opens again with {}.", app(asked)))
}

/// Where Kumi keeps the DAW a session last worked with.
fn last_used_file(env: &Env) -> std::path::PathBuf {
    std::path::Path::new(&crate::config::kumi_dir(env)).join("last-daw")
}

/// The DAW a session last worked with, when Kumi knows.
pub fn last_used(env: &Env) -> Option<DawId> {
    DawId::parse(std::fs::read_to_string(last_used_file(env)).ok()?.trim())
}

/// Keeps the DAW a session works with, for choosing among several next time. Best effort, and written only when it
/// changes.
pub fn remember_used(id: DawId, env: &Env) {
    if last_used(env) != Some(id) {
        let _ = std::fs::write(last_used_file(env), format!("{}\n", id.as_str()));
    }
}

thread_local! {
    static CURRENT: Cell<DawId> = const { Cell::new(DawId::Ableton) };
}

/// Sets the DAW this run works with (Live until a session says otherwise).
pub fn work_with(id: DawId) {
    CURRENT.with(|current| current.set(id));
}

/// The DAW this run works with.
pub fn current() -> &'static dyn DawProvider {
    provider(CURRENT.with(Cell::get)).expect("the DAW worked with is registered")
}

/// The DAW the app works with.
pub fn profile() -> &'static DawProfile {
    current().profile()
}

/// The DAW's words for what the app says ("Live", "Set", "Sets"): Live's for Live, so its screens keep their bytes.
pub fn words() -> &'static kumi_types::daw::DawWords {
    &profile().words
}

/// Whether the doctor speaks of `provider` on this computer: Live's checks always, unless the producer uses other DAWs
/// alone (Live's bridge isn't in, and another DAW's is); another DAW's once its bridge is in, or when the doctor is about
/// that DAW (`--daw renoise doctor`, or the DAW a first run would set up): a Live producer who only opened REAPER or
/// Renoise once sees the doctor as before (review L7).
pub fn in_use(provider: &dyn DawProvider, env: &Env) -> bool {
    if provider.id() != DawId::Ableton {
        return provider.bridge_installed(env) || current().id() == provider.id();
    }
    provider.bridge_installed(env) || !providers().into_iter().any(|other| other.id() != DawId::Ableton && other.bridge_installed(env))
}

/// The DAWs the producer works with here, by name, for the doctor's heading: those whose bridge is in (Live when none
/// is).
pub fn used_names(env: &Env) -> String {
    let names: Vec<&str> = providers()
        .into_iter()
        .filter(|provider| provider.bridge_installed(env) || (provider.id() == DawId::Ableton && in_use(*provider, env)))
        .map(|provider| provider.profile().words.app)
        .collect();
    if names.is_empty() {
        "Live".into()
    } else {
        names.join(", ")
    }
}

/// Where the DAW is downloaded from, as setup names it.
pub fn site() -> &'static str {
    current().site()
}

/// A word as a sentence starts it: "Set", "Project", "Song".
pub fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// How the app shows the DAW's own tools.
pub fn presentation() -> &'static ToolPresentation {
    current().presentation()
}

/// How the app reads the DAW's focus and devices.
pub fn view() -> &'static ViewPresentation {
    current().view()
}
