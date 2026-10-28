//! `--daw <id>` and KUMI_DAW choose the DAW a session works with, among the DAWs this Kumi integrates (Live, REAPER and
//! Renoise). Choosing Live is what Kumi does anyway: the configuration reads, and is written, exactly as without it.
use kumi::config::*;
use kumi_types::system::Env;

fn env() -> Env {
    Env::from([
        ("KUMI_SETTINGS_FILE".into(), "/nonexistent-kumi-test/settings.json".into()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), "/nonexistent-kumi-test/Remote Scripts".into()),
    ])
}
fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}
fn config(args_: &[&str], env: &Env) -> Result<String, String> {
    load_config(&args(args_), env).map(|config| serde_json::to_string(&config).unwrap()).map_err(|error| error.message())
}

#[test]
fn choosing_live_reads_as_choosing_nothing() {
    let env = env();
    for (asked, plain) in [
        (vec!["--daw", "ableton"], vec![]),
        (vec!["--daw=ableton"], vec![]),
        (vec!["--daw", "ableton", "--inference-only"], vec!["--inference-only"]),
        (vec!["--inference-only", "--daw", "ableton"], vec!["--inference-only"]),
        (vec!["-c", "--daw", "ableton"], vec!["-c"]),
    ] {
        assert_eq!(config(&asked, &env), config(&plain, &env), "{asked:?}");
    }
    let mut chosen = env.clone();
    chosen.insert("KUMI_DAW".into(), "ableton".into());
    assert_eq!(config(&[], &chosen), config(&[], &env));
    chosen.insert("KUMI_DAW".into(), String::new());
    assert_eq!(config(&[], &chosen), config(&[], &env), "an empty KUMI_DAW is no choice");
    assert!(!config(&["--daw", "ableton"], &env).unwrap().contains("daw"), "never written for Live");
}

#[test]
fn only_a_daw_this_kumi_integrates_can_be_chosen() {
    let env = env();
    for asked in [vec!["--daw"], vec!["--daw", "bitwig"]] {
        assert_eq!(config(&asked, &env), Err("Use: --daw <daw>, with daw one of ableton, reaper, renoise.".into()), "{asked:?}");
    }
    // A name in any case, as /daw takes it.
    for asked in [vec!["--daw", "Live"], vec!["--daw", "ABLETON"]] {
        assert_eq!(config(&asked, &env), config(&[], &env), "{asked:?}");
    }
    let mut chosen = env.clone();
    chosen.insert("KUMI_DAW".into(), "bitwig".into());
    assert_eq!(config(&[], &chosen), Err("KUMI_DAW must be one of: ableton, reaper, renoise.".into()));
    // --daw beats KUMI_DAW.
    assert_eq!(config(&["--daw", "ableton"], &chosen), config(&[], &env));
}

#[test]
fn reaper_or_renoise_chosen_is_written_and_read_back() {
    let env = env();
    for (asked, id) in [(vec!["--daw", "reaper"], "reaper"), (vec!["--daw=renoise"], "renoise")] {
        let config: serde_json::Value = serde_json::from_str(&config(&asked, &env).unwrap()).unwrap();
        assert_eq!(config["daw"], id, "{asked:?}");
    }
    let mut chosen = env.clone();
    chosen.insert("KUMI_DAW".into(), "renoise".into());
    let read: serde_json::Value = serde_json::from_str(&config(&[], &chosen).unwrap()).unwrap();
    assert_eq!(read["daw"], "renoise");
    // Review K43: KUMI_DAW in capitals, as environment variables often are.
    chosen.insert("KUMI_DAW".into(), "REAPER".into());
    let read: serde_json::Value = serde_json::from_str(&config(&[], &chosen).unwrap()).unwrap();
    assert_eq!(read["daw"], "reaper");
}

/// With no DAW's bridge installed and none asked for, Kumi chats without a DAW in Live's words: Live stays the DAW Kumi
/// starts with. REAPER and Renoise are chosen with --daw or KUMI_DAW, or by their bridge being installed.
#[test]
fn without_any_bridge_kumi_chats_in_lives_words() {
    use kumi::daw::{chosen, providers};
    use kumi_types::daw::{DawId, DawProfile};
    let env = env();
    assert_eq!(
        providers().map(|provider| provider.id()).collect::<Vec<_>>(),
        [DawId::Ableton, DawId::Reaper, DawId::Renoise],
        "Live first"
    );
    let (provider, found) = chosen(None, &env);
    assert_eq!((provider.id(), found), (DawId::Ableton, None));
    let without = provider.without_daw(std::rc::Rc::new(|_| {}));
    // Live's words and vocabulary, with none of Live's capabilities while there's no Live to use them on.
    assert_eq!((without.profile().id, without.profile().words), (Some(DawId::Ableton), DawProfile::ABLETON.words));
    assert!(*without.profile() == DawProfile::ABLETON.without_caps());
    let config = serde_json::to_value(load_config(&[], &env).unwrap()).unwrap();
    assert_eq!((config["mode"].as_str(), config["bridgeMissing"].as_bool(), config.get("daw")), (Some("inference-only"), Some(true), None));
}

#[test]
fn bridge_sets_up_the_bridge_of_the_daw_asked_for_and_lives_otherwise() {
    let env = env();
    assert_eq!(config(&["bridge"], &env).unwrap(), r#"{"mode":"bridge","yes":false,"allowDirty":false}"#, "Live's, as always");
    assert_eq!(config(&["--daw", "ableton", "bridge"], &env), config(&["bridge"], &env));
    assert_eq!(
        config(&["--daw", "reaper", "bridge", "--yes"], &env).unwrap(),
        r#"{"mode":"bridge","yes":true,"allowDirty":false,"daw":"reaper"}"#
    );
    assert_eq!(config(&["bridge", "--daw=renoise"], &env), config(&["--daw", "renoise", "bridge"], &env));
    // KUMI_DAW chooses the DAW sessions work with, never whose bridge `kumi bridge` sets up: Live's update ends with
    // `kumi bridge`, and every fix Kumi gives for Live says it (review L1). Nor does one Kumi doesn't know stop it (L10).
    for chosen in ["renoise", "reaper", "logic"] {
        let mut chosen_env = env.clone();
        chosen_env.insert("KUMI_DAW".into(), chosen.into());
        assert_eq!(config(&["bridge", "--yes"], &chosen_env), config(&["bridge", "--yes"], &env), "KUMI_DAW={chosen}");
        assert_eq!(config(&["--daw", "reaper", "bridge"], &chosen_env), config(&["--daw", "reaper", "bridge"], &env));
    }
    assert!(config(&["--daw", "logic", "bridge"], &env).unwrap_err().starts_with("Use: --daw <daw>"));
    assert_eq!(config(&["--daw", "reaper", "bridge", "--force"], &env).unwrap_err(), "Use: bridge [--yes] [--allow-dirty].");
}

/// `kumi --daw reaper doctor` (or KUMI_DAW) asks the doctor and the report to speak of that DAW; without it they read as
/// they always did.
#[test]
fn doctor_and_report_take_the_daw_asked_for() {
    let env = env();
    assert_eq!(config(&["doctor"], &env).unwrap(), r#"{"mode":"doctor","json":false}"#);
    assert_eq!(config(&["doctor", "--json"], &env).unwrap(), r#"{"mode":"doctor","json":true}"#);
    assert_eq!(config(&["report"], &env).unwrap(), r#"{"mode":"report"}"#);
    assert_eq!(config(&["--daw", "reaper", "doctor"], &env).unwrap(), r#"{"mode":"doctor","json":false,"daw":"reaper"}"#);
    assert_eq!(config(&["doctor", "--json", "--daw=renoise"], &env).unwrap(), r#"{"mode":"doctor","json":true,"daw":"renoise"}"#);
    assert_eq!(config(&["--daw", "ableton", "report"], &env).unwrap(), r#"{"mode":"report","daw":"ableton"}"#, "Live asked for decides");
    let mut chosen = env.clone();
    chosen.insert("KUMI_DAW".into(), "renoise".into());
    assert_eq!(config(&["report"], &chosen).unwrap(), r#"{"mode":"report","daw":"renoise"}"#);
    assert!(config(&["--daw", "logic", "doctor"], &env).unwrap_err().starts_with("Use: --daw <daw>"));
    // A KUMI_DAW Kumi doesn't know never stops the doctor or the report (the doctor says so).
    let mut wrong = env.clone();
    wrong.insert("KUMI_DAW".into(), "bitwig".into());
    assert_eq!(config(&["doctor"], &wrong).unwrap(), r#"{"mode":"doctor","json":false}"#);
    assert_eq!(config(&["report"], &wrong).unwrap(), r#"{"mode":"report"}"#);
    assert!(config(&["doctor", "--verbose"], &env).is_err());
}

/// "live" is another name for Live's DAW, as /daw live takes it: `--daw live` and KUMI_DAW=live choose Live, and nothing
/// writes it (review L10).
#[test]
fn live_is_another_name_for_ableton() {
    let env = env();
    for asked in [vec!["--daw", "live"], vec!["--daw=live"], vec!["--daw", "ableton"]] {
        assert_eq!(config(&asked, &env), config(&[], &env), "{asked:?}");
    }
    let mut live = env.clone();
    live.insert("KUMI_DAW".into(), "live".into());
    assert_eq!(config(&[], &live), config(&[], &env));
    assert_eq!(config(&["bridge"], &live), config(&["bridge"], &env));
    assert_eq!(config(&["--daw", "live", "doctor"], &env).unwrap(), r#"{"mode":"doctor","json":false,"daw":"ableton"}"#);
}

/// A bridge configuration given (`--bridge-config <Live's file>`) starts Live whatever KUMI_DAW says; --daw still says
/// whose it is (review L10).
#[test]
fn a_bridge_configuration_given_is_lives_unless_daw_says_whose() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bridge.json");
    std::fs::write(&file, "{}").unwrap();
    let path = file.to_string_lossy().into_owned();
    let mut chosen = env();
    chosen.insert("KUMI_DAW".into(), "reaper".into());
    let started = config(&["--bridge-config", &path], &chosen).unwrap();
    assert!(started.contains(r#""mode":"live""#) && !started.contains(r#""daw""#), "{started}");
    let asked = config(&["--daw", "reaper", "--bridge-config", &path], &env()).unwrap();
    assert!(asked.contains(r#""daw":"reaper""#), "{asked}");
}

/// A start with several bridges asks every DAW whether it runs (choosing, then saying which others are open) from one
/// process listing, not one per DAW (review L11).
#[test]
fn every_daw_is_asked_whether_it_runs_from_one_process_listing() {
    let before = kumi::daw::process_listings();
    for provider in kumi::daw::providers() {
        provider.running_now(&kumi::daw::processes());
    }
    let env = env();
    let chosen = kumi::daw::chosen_with(None, &env, &|provider| provider.running_now(&kumi::daw::processes())).0.id();
    kumi::daw::also_running(chosen, &|provider| provider.running_now(&kumi::daw::processes()));
    for provider in kumi::daw::providers() {
        provider.running_now(&kumi::daw::processes());
    }
    assert_eq!(kumi::daw::process_listings() - before, 1, "one listing for every DAW");
}

/// With no bridge installed, a first run sets up the DAW on this computer: the only one here, else the one the producer
/// chose before, else Live, else the first; setup asks among several none of which was chosen (review L2).
#[test]
fn without_a_bridge_the_daw_here_is_the_one_a_first_run_sets_up() {
    use kumi_types::daw::DawId;
    let home = tempfile::tempdir().unwrap();
    let mut env = env();
    env.insert("KUMI_HOME".into(), home.path().to_string_lossy().into_owned());
    let not_running = |_: &dyn kumi_daw::provider::DawProvider| false;
    let first = |here: &[DawId], env: &Env| {
        let here = here.to_vec();
        kumi::daw::chosen_among(None, env, &not_running, &move |provider| here.contains(&provider.id())).0.id()
    };
    let asked = |here: &[DawId], env: &Env| {
        let here = here.to_vec();
        kumi::daw::to_ask(env, &move |provider| here.contains(&provider.id()), &not_running)
    };
    assert_eq!(first(&[DawId::Reaper], &env), DawId::Reaper, "REAPER alone");
    assert_eq!(first(&[DawId::Renoise], &env), DawId::Renoise, "Renoise alone");
    assert_eq!(first(&[DawId::Ableton], &env), DawId::Ableton, "Live alone, as always");
    assert_eq!(first(&[], &env), DawId::Ableton, "none: Live's words, as always");
    assert!(asked(&[DawId::Reaper], &env).is_empty() && asked(&[], &env).is_empty(), "nothing to ask with one or none");
    assert_eq!(first(&[DawId::Ableton, DawId::Reaper], &env), DawId::Ableton);
    assert_eq!(asked(&[DawId::Ableton, DawId::Reaper], &env), [DawId::Ableton, DawId::Reaper]);
    assert_eq!(first(&[DawId::Reaper, DawId::Renoise], &env), DawId::Reaper);
    // One open is the answer: nothing to ask.
    assert!(kumi::daw::to_ask(&env, &|_| true, &|provider| provider.id() == DawId::Renoise).is_empty(), "Renoise open");
    // Chosen once (setup remembers it): that one, and nothing asked again.
    kumi::daw::remember_used(DawId::Renoise, &env);
    assert_eq!(first(&[DawId::Ableton, DawId::Renoise], &env), DawId::Renoise);
    assert!(asked(&[DawId::Ableton, DawId::Renoise], &env).is_empty());
    // Asked for, it's that one whatever is here.
    assert_eq!(kumi::daw::chosen_among(Some(DawId::Ableton), &env, &not_running, &|_| true).0.id(), DawId::Ableton);
}

/// A home where Kumi's bridge is in Live (its reference and configuration, as `kumi bridge` leaves them) and Renoise has
/// run (its preferences folder), with no tool of Kumi's in it yet.
fn live_and_renoise_home() -> (tempfile::TempDir, Env) {
    let dir = tempfile::tempdir().unwrap();
    let scripts = dir.path().join("Remote Scripts");
    std::fs::create_dir_all(scripts.join("AbletonMcpBridge")).unwrap();
    let config = dir.path().join("bridge-config.json");
    std::fs::write(&config, "{}").unwrap();
    std::fs::write(scripts.join("AbletonMcpBridge/bridge-reference.json"), serde_json::json!({ "config": config }).to_string()).unwrap();
    let prefs = dir.path().join("Renoise");
    std::fs::create_dir_all(prefs.join("V3.5.4/Scripts/Tools")).unwrap();
    let path = |path: std::path::PathBuf| path.to_string_lossy().into_owned();
    let env = Env::from([
        ("HOME".into(), path(dir.path().to_path_buf())),
        ("KUMI_HOME".into(), path(dir.path().join(".kumi"))),
        ("KUMI_SETTINGS_FILE".into(), path(dir.path().join("settings.json"))),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), path(scripts)),
        ("KUMI_RENOISE_PREFS_DIR".into(), path(prefs)),
    ]);
    (dir, env)
}

fn open(ids: &'static [kumi_types::daw::DawId]) -> impl Fn(&dyn kumi_daw::provider::DawProvider) -> bool {
    move |provider| ids.contains(&provider.id())
}

/// The DAW open is the one Kumi works with, its bridge in or not: a producer with Kumi in Live who opens Renoise gets
/// Renoise (first-run setup puts Kumi's tool in), and Live again once nothing else is open.
#[test]
fn the_daw_open_is_the_one_kumi_works_with() {
    use kumi::daw::chosen_with;
    use kumi_types::daw::DawId;
    let (_dir, env) = live_and_renoise_home();
    let chosen = |ids: &'static [DawId]| {
        let (provider, config) = chosen_with(None, &env, &open(ids));
        (provider.id(), config.is_some())
    };
    assert_eq!(chosen(&[DawId::Renoise]), (DawId::Renoise, false), "Renoise open: Renoise, without its tool yet");
    assert_eq!(chosen(&[DawId::Reaper]), (DawId::Reaper, false), "REAPER open: REAPER");
    assert_eq!(chosen(&[]), (DawId::Ableton, true), "none open: Live, whose bridge is in");
    assert_eq!(chosen(&[DawId::Ableton]), (DawId::Ableton, true));
    assert_eq!(chosen(&[DawId::Ableton, DawId::Renoise]), (DawId::Ableton, true), "both open: the one Kumi is in");
    assert_eq!(chosen_with(Some(DawId::Ableton), &env, &open(&[DawId::Renoise])).0.id(), DawId::Ableton, "asked for, always");
    // Which others are open, for the session to say.
    let others = kumi::daw::also_running(DawId::Ableton, &open(&[DawId::Ableton, DawId::Renoise]));
    assert_eq!(others.iter().map(|provider| provider.id()).collect::<Vec<_>>(), [DawId::Renoise], "with its tool in or not");
}

/// `kumi` alone, as a producer runs it, with Renoise open: Kumi works with Renoise, and setup puts its tool in. With
/// nothing open it's Live, whose bridge is in, as always.
#[test]
fn kumi_alone_works_with_the_daw_open() {
    let (_dir, mut env) = live_and_renoise_home();
    env.insert("KUMI_PROCESSES".into(), "7 /Applications/Renoise.app/Contents/MacOS/Renoise".into());
    let config = serde_json::to_value(load_config(&[], &env).unwrap()).unwrap();
    assert_eq!(
        (config["mode"].as_str(), config["daw"].as_str(), config["bridgeMissing"].as_bool()),
        (Some("inference-only"), Some("renoise"), Some(true)),
        "{config}"
    );
    env.insert("KUMI_PROCESSES".into(), "none".into());
    let config = serde_json::to_value(load_config(&[], &env).unwrap()).unwrap();
    assert_eq!((config["mode"].as_str(), config.get("daw")), (Some("live"), None), "{config}");
}

/// Kumi follows the producer to the DAW they open while it isn't connected: the one open instead of its own, or one
/// whose bridge is in while its own has none. It's chosen as a start chooses, so Kumi opened again starts with it, and
/// never follows back and forth.
#[test]
fn kumi_follows_to_the_daw_opened_as_a_start_would_choose_it() {
    use kumi::daw::{chosen_with, to_follow};
    use kumi_types::daw::DawId;
    let (_dir, env) = live_and_renoise_home();
    let all = [DawId::Ableton, DawId::Reaper, DawId::Renoise];
    for mask in 0..8u8 {
        let running: Vec<DawId> = all.iter().enumerate().filter(|(at, _)| mask & (1 << at) != 0).map(|(_, id)| *id).collect();
        let open = |provider: &dyn kumi_daw::provider::DawProvider| running.contains(&provider.id());
        for current in all {
            if let Some(next) = to_follow(current, &env, &open) {
                assert_ne!(next.id(), current, "{running:?}");
                assert_eq!(chosen_with(None, &env, &open).0.id(), next.id(), "from {current:?} with {running:?} open");
            }
        }
    }
    let follows = |current: DawId, ids: &'static [DawId]| to_follow(current, &env, &open(ids)).map(|provider| provider.id());
    assert_eq!(follows(DawId::Ableton, &[DawId::Renoise]), Some(DawId::Renoise), "Renoise opened, Live not");
    assert_eq!(follows(DawId::Ableton, &[]), None, "nothing open");
    assert_eq!(follows(DawId::Ableton, &[DawId::Ableton, DawId::Renoise]), None, "Live open: Kumi waits for it to answer");
    assert_eq!(
        follows(DawId::Renoise, &[DawId::Renoise, DawId::Ableton]),
        Some(DawId::Ableton),
        "Renoise without the tool, Live with the bridge"
    );
    assert_eq!(follows(DawId::Renoise, &[DawId::Renoise]), None, "Renoise alone: setup puts its tool in");
    assert_eq!(follows(DawId::Reaper, &[DawId::Renoise]), Some(DawId::Renoise));
}

/// Kumi's bridge in REAPER or Renoise, as its receipt says.
fn own_bridge_in(env: &Env, daw: daw_bridge::Daw) {
    let paths = daw_bridge::paths::BridgePaths::new(std::path::Path::new(&env["KUMI_HOME"]), daw);
    std::fs::create_dir_all(&paths.root).unwrap();
    let receipt = daw_bridge::lifecycle::Receipt {
        schema: daw_bridge::lifecycle::RECEIPT_SCHEMA.into(),
        daw: daw.id().into(),
        status: "installed".into(),
        target: paths.root.join("bridge").to_string_lossy().into_owned(),
        reference: paths.root.join("bridge-reference.json").to_string_lossy().into_owned(),
        generation: Some("g1".into()),
        version: Some("1.0.0".into()),
        sha256: None,
        previous: vec![],
        copies: vec![],
        updated_ms: 0,
        history: vec![],
    };
    std::fs::write(paths.receipt(), serde_json::to_string(&receipt).unwrap()).unwrap();
}

/// With no DAW open, a producer who works with Kumi in several DAWs hears which to open, not how to make one answer;
/// with Kumi in one, its own words say it (another DAW only being installed changes nothing).
#[test]
fn with_no_daw_open_kumi_says_to_open_one_of_theirs() {
    use kumi::daw::open_one_words;
    let (_dir, env) = live_and_renoise_home();
    assert_eq!(open_one_words(&env), None, "Kumi in Live alone, Renoise here");
    own_bridge_in(&env, daw_bridge::Daw::Renoise);
    assert_eq!(open_one_words(&env).as_deref(), Some("No DAW is open yet: open Live or Renoise, and Kumi connects to the one you open."));
    own_bridge_in(&env, daw_bridge::Daw::Reaper);
    assert_eq!(
        open_one_words(&env).as_deref(),
        Some("No DAW is open yet: open Live, REAPER or Renoise, and Kumi connects to the one you open.")
    );
}
