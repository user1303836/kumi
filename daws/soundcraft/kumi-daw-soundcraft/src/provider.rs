//! SoundCraft as Kumi's app works with it: its bridge is a program beside Kumi's (`soundcraft-kumi`), which opens SoundCraft
//! with its control channel on, so nothing goes into SoundCraft (kumi-daw-own's `Beside`). Experimental: only
//! `--daw soundcraft`, `KUMI_DAW` and `/daw soundcraft` reach it.
use daw_bridge::{daw::Daw, detect};
use kumi_daw::provider::WatchWords;
use kumi_daw_own::{beside::Beside, provider::detected};
use kumi_types::daw::{DawId, DawProfile};
use kumi_types::system::Env;
use std::path::PathBuf;

/// SoundCraft, through the program beside Kumi.
pub static PROVIDER: Beside = Beside {
    id: DawId::SoundCraft,
    daw: Daw::SoundCraft,
    profile: &DawProfile::SOUNDCRAFT,
    presentation: &crate::PRESENTATION,
    view: &crate::VIEW,
    library: &crate::library::LIBRARY,
    site: "github.com/storytold/soundcraft",
    experimental: true,
    // (as its change set says: `tests::kumi_reads_soundcraft_and_has_the_producer_make_a_fix`)
    reads_only: true,
    program: "soundcraft-kumi",
    app_variable: detect::SOUNDCRAFT_APP,
    bridge_folder: "bridges/soundcraft",
    create: crate::create_soundcraft_integration,
    instructions: "SoundCraft isn't connected, so Kumi can't see your session: answer from what you know, and say so.",
    not_found: "SoundCraft isn't where Kumi looks (set KUMI_SOUNDCRAFT_APP to the app)",
    restart_why: "Kumi reaches the SoundCraft it opens itself, with its control channel on.",
    watch_words: WatchWords {
        starting: "SoundCraft is opening; Kumi connects once its bridge answers.",
        unreachable: "Kumi couldn't reach SoundCraft, so this is chat without it. Kumi reaches only the SoundCraft it opens: run kumi --daw soundcraft bridge (save and quit a SoundCraft you opened yourself first), and Kumi connects once it answers.",
        answered: "SoundCraft answered, so Kumi is connected to it now.",
    },
    used: |env| last_session(env).is_some(),
    here: |env| detected(env, detect::detect_soundcraft).is_some(),
    app,
    app_version: |env, is| detected(env, detect::detect_soundcraft).filter(|found| is(&found.path)).and_then(|found| found.version),
    last_session,
};

fn app(env: &Env) -> Option<PathBuf> {
    detected(env, detect::detect_soundcraft).map(|found| found.path)
}

/// The session SoundCraft opened or saved last, by the settings folder of the app Kumi opens.
fn last_session(env: &Env) -> Option<String> {
    let found = app(env);
    let settings = detected(env, |detect| detect::soundcraft_settings(detect, found.as_deref()))?;
    detect::soundcraft_last_session(&settings).map(|session| session.to_string_lossy().into_owned())
}
