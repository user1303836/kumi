//! REAPER's and Renoise's providers: Kumi's bridge goes into the DAW's own folder through the shared lifecycle, is
//! found again (and chosen when it's the only bridge installed), and comes out; the doctor says nothing of a DAW this
//! user has never used.
#[path = "../support/live_words.rs"]
mod live_words;
use futures::FutureExt;
use kumi::daw::{chosen, registry, DawProvider, RemovedBridge};
use kumi_types::{daw::DawId, system::Env};
use std::rc::Rc;

#[derive(Default)]
struct Out(std::cell::RefCell<String>);
impl kumi::tui::tty::TtyOutput for Out {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        Some(80)
    }
    fn rows(&self) -> Option<i32> {
        Some(24)
    }
    fn write(&self, s: &str) {
        self.0.borrow_mut().push_str(s);
    }
}

/// A home with Renoise 3.5.4's preferences folder (as Renoise makes on its first run), and Kumi's folder in it.
fn renoise_home() -> (tempfile::TempDir, Env) {
    let dir = tempfile::tempdir().unwrap();
    let prefs = dir.path().join("Renoise");
    std::fs::create_dir_all(prefs.join("V3.5.4").join("Scripts").join("Tools")).unwrap();
    let env = Env::from([
        ("HOME".into(), dir.path().to_string_lossy().into_owned()),
        ("KUMI_HOME".into(), dir.path().join(".kumi").to_string_lossy().into_owned()),
        ("KUMI_RENOISE_PREFS_DIR".into(), prefs.to_string_lossy().into_owned()),
        ("KUMI_SETTINGS_FILE".into(), dir.path().join("settings.json").to_string_lossy().into_owned()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), dir.path().join("Remote Scripts").to_string_lossy().into_owned()),
    ]);
    (dir, env)
}

/// `kumi --daw <daw> bridge --yes`.
fn yes() -> kumi::daw::BridgeCommand {
    kumi::daw::BridgeCommand { yes: true, ..Default::default() }
}

#[tokio::test(flavor = "current_thread")]
async fn renoises_tool_goes_in_is_found_and_chosen_and_comes_out() {
    let (dir, env) = renoise_home();
    let renoise: &dyn DawProvider = &registry::RENOISE;
    assert!(!renoise.bridge_installed(&env));
    // Nothing installed: no bridge found, and a first run sets up the DAW here (Renoise, unless another is here too).
    assert_eq!(chosen(None, &env).1, None, "nothing installed");
    let renoise_alone = kumi::daw::chosen_among(None, &env, &|_| false, &|provider| provider.here(&env) && provider.id() == DawId::Renoise);
    assert_eq!(renoise_alone.0.id(), DawId::Renoise, "Renoise here alone: its setup");

    // Asked first: with nobody to answer (no --yes, no terminal), nothing goes in Renoise's folder.
    let out = Rc::new(Out::default());
    assert_eq!(renoise.setup_bridge(out.clone(), &env, Default::default()).await.unwrap(), 1);
    let said = out.0.borrow().clone();
    assert!(said.contains("Put Kumi's tool in Renoise (") && said.contains("add --yes"), "{said}");
    assert!(said.contains("Nothing was changed. To put it in later: ") && said.contains(" --daw renoise bridge\n"), "{said}");
    assert!(!renoise.bridge_installed(&env) && !dir.path().join("Renoise/V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx").exists());

    let out = Rc::new(Out::default());
    let code = renoise.setup_bridge(out.clone(), &env, yes()).await.unwrap();
    assert_eq!(code, 0, "{}", out.0.borrow());
    assert!(out.0.borrow().contains(&format!("Kumi's tool {} is in Renoise", tool_version())), "{}", out.0.borrow());
    let tool = dir.path().join("Renoise").join("V3.5.4").join("Scripts").join("Tools").join("com.kumi.Bridge.xrnx");
    assert!(tool.join("main.lua").exists() && tool.join("bridge-reference.json").exists());
    assert!(renoise.bridge_installed(&env));
    let config = renoise.bridge_config(&env).expect("the reference, once installed");
    assert!(config.ends_with("bridge-reference.json"), "{config}");
    let (provider, found) = chosen(None, &env);
    assert_eq!((provider.id(), found.as_deref()), (DawId::Renoise, Some(config.as_str())), "the only bridge installed is chosen");

    // A file of the producer's in it: the tool comes out, and the copy is kept in Kumi's folder, said with where.
    std::fs::write(tool.join("mine.lua"), "-- the producer's\n").unwrap();
    let out = Rc::new(Out::default());
    let io = kumi::install::InstalledIo::new(out.clone(), env.clone());
    let run: kumi::process::Run = Rc::new(|_, _, _| panic!("removing the tool runs no program"));
    assert_eq!(renoise.remove_bridge(&io, run).await.unwrap(), RemovedBridge::Removed);
    assert!(!tool.exists());
    assert!(!renoise.bridge_installed(&env));
    let said = out.0.borrow().clone();
    assert!(said.contains(&format!("Kumi's tool is out of Renoise ({}).", tool.display())), "{said}");
    let kept = dir.path().join(".kumi").join("bridges").join("renoise").join("changed");
    assert!(said.contains("had changed since Kumi put it there: it's kept in ") && said.contains(&kept.display().to_string()), "{said}");
    assert_eq!(std::fs::read_dir(&kept).unwrap().count(), 1);
}

/// A tool that can't come out (its folder won't let Kumi move it) stays, and uninstall says what to delete by hand.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn a_tool_that_cant_come_out_is_named_with_what_to_delete_by_hand() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, env) = renoise_home();
    let renoise: &dyn DawProvider = &registry::RENOISE;
    assert_eq!(renoise.setup_bridge(Rc::new(Out::default()), &env, yes()).await.unwrap(), 0);
    let tools = dir.path().join("Renoise/V3.5.4/Scripts/Tools");
    std::fs::set_permissions(&tools, std::fs::Permissions::from_mode(0o555)).unwrap();
    let out = Rc::new(Out::default());
    let io = kumi::install::InstalledIo::new(out.clone(), env.clone());
    let run: kumi::process::Run = Rc::new(|_, _, _| panic!("removing the tool runs no program"));
    let removed = renoise.remove_bridge(&io, run).await.unwrap();
    std::fs::set_permissions(&tools, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(removed, RemovedBridge::Kept);
    let said = out.0.borrow().clone();
    let tool = tools.join("com.kumi.Bridge.xrnx");
    assert!(said.starts_with("Kumi's tool couldn't be taken out of Renoise ("), "{said}");
    assert!(said.contains(&format!("): delete {} by hand.", tool.display())), "{said}");
}

#[tokio::test(flavor = "current_thread")]
async fn the_doctor_says_nothing_of_a_daw_this_user_never_used() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::from([
        ("HOME".into(), dir.path().to_string_lossy().into_owned()),
        ("KUMI_HOME".into(), dir.path().join(".kumi").to_string_lossy().into_owned()),
        ("KUMI_RENOISE_PREFS_DIR".into(), dir.path().join("Renoise").to_string_lossy().into_owned()),
        ("KUMI_REAPER_RESOURCE_DIR".into(), dir.path().join("REAPER").to_string_lossy().into_owned()),
    ]);
    let io = kumi::doctor::InstallationIo::default();
    for provider in [&registry::REAPER as &dyn DawProvider, &registry::RENOISE] {
        assert!(provider.installation_checks(&env, &io, "darwin").await.is_empty(), "{:?}", provider.id());
    }
    let (_renoise_dir, renoise_env) = renoise_home();
    let checks = registry::RENOISE.installation_checks(&renoise_env, &io, "darwin").await;
    assert_eq!(checks.len(), 1);
}

/// With several DAWs' bridges installed and none asked for: the one running now, else the one used last, else Live,
/// else the first. A producer with Live's bridge alone starts in Live, whatever runs.
#[tokio::test(flavor = "current_thread")]
async fn among_several_bridges_the_running_daw_then_the_last_used_is_chosen() {
    use kumi::daw::{chosen_with, remember_used};
    let (dir, mut env) = renoise_home();
    let out = Rc::new(Out::default());
    assert_eq!(registry::RENOISE.setup_bridge(out.clone(), &env, yes()).await.unwrap(), 0, "{}", out.0.borrow());
    // Kumi's extension in REAPER too, as its receipt says.
    let resource = dir.path().join("REAPER");
    env.insert("KUMI_REAPER_RESOURCE_DIR".into(), resource.to_string_lossy().into_owned());
    let paths = daw_bridge::paths::BridgePaths::new(&dir.path().join(".kumi"), daw_bridge::daw::Daw::Reaper);
    std::fs::create_dir_all(&paths.root).unwrap();
    let receipt = daw_bridge::lifecycle::Receipt {
        schema: daw_bridge::lifecycle::RECEIPT_SCHEMA.into(),
        daw: "reaper".into(),
        status: "installed".into(),
        target: resource.join("UserPlugins/reaper_kumi.dylib").to_string_lossy().into_owned(),
        reference: resource.join("UserPlugins/kumi-bridge-reference.json").to_string_lossy().into_owned(),
        generation: Some("g1".into()),
        version: Some("0.1.0".into()),
        sha256: None,
        previous: vec![],
        copies: vec![],
        updated_ms: 0,
        history: vec![],
    };
    std::fs::write(paths.receipt(), serde_json::to_string(&receipt).unwrap()).unwrap();
    assert!(registry::REAPER.bridge_installed(&env));
    let only = |id: DawId| move |provider: &dyn DawProvider| provider.id() == id;
    assert_eq!(chosen_with(None, &env, &only(DawId::Renoise)).0.id(), DawId::Renoise, "the one running");
    assert_eq!(chosen_with(None, &env, &only(DawId::Reaper)).0.id(), DawId::Reaper, "the one running");
    let none = |_: &dyn DawProvider| false;
    assert_eq!(chosen_with(None, &env, &none).0.id(), DawId::Reaper, "none running, none used: the first installed");
    remember_used(DawId::Renoise, &env);
    assert_eq!(chosen_with(None, &env, &none).0.id(), DawId::Renoise, "none running: the one used last");
    let both = |provider: &dyn DawProvider| provider.id() != DawId::Ableton;
    assert_eq!(chosen_with(None, &env, &both).0.id(), DawId::Renoise, "both running: the one used last");
    assert_eq!(chosen_with(Some(DawId::Reaper), &env, &none).0.id(), DawId::Reaper, "asked for, always");
    // Both running and none asked for: the session says which one it works with, and how to switch.
    use kumi::daw::{also_running, also_running_words};
    let others = also_running(DawId::Renoise, &env, &both);
    assert_eq!(others.iter().map(|provider| provider.id()).collect::<Vec<_>>(), vec![DawId::Reaper]);
    assert_eq!(
        also_running_words(DawId::Renoise, &others, &env).as_deref(),
        Some("REAPER is open too: Kumi works with Renoise, the one you used last. /daw reaper switches to REAPER.")
    );
    assert!(also_running(DawId::Renoise, &env, &only(DawId::Renoise)).is_empty(), "alone: nothing to say");
}

/// `/daw` says which DAW Kumi works with, and takes the DAW by the producer's name for it ("live") or Kumi's id.
#[test]
fn daw_says_which_daw_kumi_works_with_and_switches_by_name() {
    use kumi::daw::{daw_command, daw_command_among, work_with, DawCommand};
    work_with(DawId::Reaper);
    assert_eq!(
        daw_command_among("", &|_| true),
        DawCommand::Say("Kumi works with REAPER. /daw live or /daw renoise opens Kumi again with Live or Renoise.".into())
    );
    // Review K43: only the DAWs on this computer are offered.
    assert_eq!(
        daw_command_among("", &|provider| provider.id() == DawId::Ableton),
        DawCommand::Say("Kumi works with REAPER. /daw live opens Kumi again with Live.".into())
    );
    assert_eq!(
        daw_command_among("", &|_| false),
        DawCommand::Say("Kumi works with REAPER, the only DAW it works with on this computer.".into())
    );
    for typed in [" Live", "live", "ableton"] {
        assert_eq!(daw_command(typed), DawCommand::Switch(DawId::Ableton, "Kumi closes and opens again with Live.".into()), "{typed}");
    }
    assert_eq!(daw_command("reaper"), DawCommand::Say("Kumi works with REAPER already.".into()));
    assert_eq!(daw_command("bitwig"), DawCommand::Say("Kumi doesn't know bitwig: /daw live, /daw reaper, /daw renoise.".into()));
    work_with(DawId::Ableton);
}

/// What `kumi --daw reaper|renoise bridge`, uninstall and update say for REAPER and Renoise: their own words, never
/// Live's, and the command that sets up their bridge (`--daw`), not Live's.
#[tokio::test(flavor = "current_thread")]
async fn reapers_and_renoises_bridge_messages_never_speak_lives_words() {
    let (dir, mut env) = renoise_home();
    env.insert("KUMI_REAPER_RESOURCE_DIR".into(), dir.path().join("REAPER").to_string_lossy().into_owned());
    std::fs::create_dir_all(dir.path().join("REAPER/UserPlugins")).unwrap();
    for provider in [&registry::REAPER as &dyn DawProvider, &registry::RENOISE] {
        let out = Rc::new(Out::default());
        provider.setup_bridge(out.clone(), &env, yes()).await.unwrap();
        let io = kumi::install::InstalledIo::new(out.clone(), env.clone());
        let run: kumi::process::Run = Rc::new(|_, _, _| panic!("removing the bridge runs no program"));
        let _ = provider.remove_bridge(&io, run).await;
        let state = provider.bridge_state(&env, false, true, None, "kumi");
        let watch = provider.watch_words();
        let said = [
            out.0.borrow().clone(),
            provider.removal_question().to_string(),
            provider.files_stay("~/folder"),
            state.notice.clone().unwrap_or_default(),
            state.why.unwrap_or_default(),
            watch.starting.into(),
            watch.unreachable.into(),
            watch.answered.into(),
        ]
        .join("\n");
        let leaks = live_words::leaks(&said);
        assert!(leaks.is_empty(), "{:?} says Live's words:\n{}\n\nin:\n{said}", provider.id(), leaks.join("\n"));
        let command = format!("kumi --daw {} bridge", provider.id().as_str());
        assert!(state.notice.unwrap().contains(&command) && watch.unreachable.contains(&command), "{said}");
    }
}

/// Asking for Live (`--daw ableton`, `KUMI_DAW=ableton`) starts Live, though REAPER's bridge is in too and was used
/// last; asking for nothing goes by the last used. The configuration still never writes Live.
#[tokio::test(flavor = "current_thread")]
async fn asking_for_live_starts_live_whatever_was_used_last() {
    let (dir, mut env) = renoise_home();
    // Live's bridge, as its reference in the Remote Scripts says.
    let config = dir.path().join("bridge-config.json");
    std::fs::write(&config, "{}").unwrap();
    let scripts = dir.path().join("Remote Scripts/AbletonMcpBridge");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("bridge-reference.json"), serde_json::json!({"config": config}).to_string()).unwrap();
    // REAPER's, as its receipt says, and used last.
    let resource = dir.path().join("REAPER");
    env.insert("KUMI_REAPER_RESOURCE_DIR".into(), resource.to_string_lossy().into_owned());
    let paths = daw_bridge::paths::BridgePaths::new(&dir.path().join(".kumi"), daw_bridge::daw::Daw::Reaper);
    std::fs::create_dir_all(&paths.root).unwrap();
    let receipt = daw_bridge::lifecycle::Receipt {
        schema: daw_bridge::lifecycle::RECEIPT_SCHEMA.into(),
        daw: "reaper".into(),
        status: "installed".into(),
        target: resource.join("UserPlugins/reaper_kumi.dylib").to_string_lossy().into_owned(),
        reference: resource.join("UserPlugins/kumi-bridge-reference.json").to_string_lossy().into_owned(),
        generation: Some("g1".into()),
        version: Some("0.1.0".into()),
        sha256: None,
        previous: vec![],
        copies: vec![],
        updated_ms: 0,
        history: vec![],
    };
    std::fs::write(paths.receipt(), serde_json::to_string(&receipt).unwrap()).unwrap();
    kumi::daw::remember_used(DawId::Reaper, &env);
    let config_of = |args: &[&str], env: &Env| {
        serde_json::to_value(kumi::config::load_config(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), env).unwrap()).unwrap()
    };
    let asked = config_of(&["--daw", "ableton"], &env);
    assert_eq!(asked["bridgeConfig"], serde_json::json!(config.to_string_lossy()), "Live's bridge: {asked}");
    assert!(asked.get("daw").is_none(), "Live is never written: {asked}");
    let mut by_env = env.clone();
    by_env.insert("KUMI_DAW".into(), "ableton".into());
    assert_eq!(config_of(&[], &by_env), asked);
    // Nothing asked and nothing running: the one used last (what runs on this computer is left out).
    let none = |_: &dyn DawProvider| false;
    assert_eq!(kumi::daw::chosen_with(None, &env, &none).0.id(), DawId::Reaper, "nothing asked: the one used last");
    assert_eq!(kumi::daw::chosen_with(Some(DawId::Ableton), &env, &none).0.id(), DawId::Ableton, "Live asked: Live");
}

/// Copies a folder, as a release's bundle holds one.
fn copy_folder(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            copy_folder(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

/// The version of Renoise's tool this checkout carries (its version.lua), which a release that changes the tool moves on.
fn tool_version() -> String {
    let lua = std::fs::read_to_string(kumi_testkit::workspace_path("daws/renoise/com.kumi.Bridge.xrnx/payload/version.lua")).unwrap();
    lua.split('"').nth(1).expect("version.lua returns the tool's version").to_owned()
}

/// A Kumi's app folder carrying REAPER's extension (these bytes, this version) and Renoise's tool (this checkout's,
/// saying `tool`), laid out as a release lays them out.
fn app_carrying(root: &std::path::Path, name: &str, extension: &[u8], version: &str, tool: &str) -> std::path::PathBuf {
    let app = root.join(name);
    let reaper = app.join("daws").join("reaper");
    std::fs::create_dir_all(&reaper).unwrap();
    let file = daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current());
    std::fs::write(reaper.join(file), extension).unwrap();
    std::fs::write(reaper.join("reaper_kumi.version"), format!("{version}\n")).unwrap();
    let folder = app.join("daws").join("renoise").join("com.kumi.Bridge.xrnx");
    copy_folder(&kumi_testkit::workspace_path("daws/renoise/com.kumi.Bridge.xrnx"), &folder);
    let lua = folder.join("payload").join("version.lua");
    let text = std::fs::read_to_string(&lua).unwrap();
    std::fs::write(&lua, text.replace(&format!("\"{}\"", tool_version()), &format!("\"{tool}\""))).unwrap();
    app
}

/// An update follows Kumi's bridge where it is: Renoise's tool stays in the version folder chosen at setup, and the copy
/// a newer Renoise made of it is brought along; a bridge the producer took out isn't put back, and an extension whose
/// REAPER folder is gone never moves into another REAPER. Each is said.
#[tokio::test(flavor = "current_thread")]
async fn an_update_follows_each_copy_and_never_puts_back_a_bridge_taken_out_or_moves_it_to_another_reaper() {
    use daw_bridge::daw::Daw;
    let (dir, mut env) = renoise_home();
    let resource = dir.path().join("REAPER");
    std::fs::create_dir_all(resource.join("UserPlugins")).unwrap();
    std::fs::write(resource.join("reaper.ini"), "[REAPER]\n").unwrap();
    env.insert("KUMI_REAPER_RESOURCE_DIR".into(), resource.to_string_lossy().into_owned());
    let now = app_carrying(dir.path(), "app", b"extension 0.1.0", "0.1.0", "1.0.0");
    let later = app_carrying(dir.path(), "app.new", b"extension 0.1.1", "0.1.1", "1.0.1");
    let home = env["KUMI_HOME"].clone();
    let out = Rc::new(Out::default());
    let io = kumi::install::InstalledIo::new(out.clone(), env.clone());
    assert_eq!(registry::RENOISE.setup_bridge(out.clone(), &env, yes()).await.unwrap(), 0, "{}", out.0.borrow());
    for provider in [&registry::REAPER as &dyn DawProvider, &registry::RENOISE] {
        assert_eq!(provider.after_update(&io, &home, &now.to_string_lossy(), None).await, 0);
    }
    let extension = now.join("daws/reaper").join(daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current()));
    let target = daw_bridge::install::reaper_target(
        &daw_bridge::detect::ReaperInstall {
            app: None,
            version: None,
            archs: vec![],
            resource: resource.clone(),
            resource_from: daw_bridge::detect::ResourceFrom::Override,
            opened: true,
        },
        daw_bridge::detect::Os::current(),
    )
    .unwrap();
    daw_bridge::lifecycle::Lifecycle::new(std::path::Path::new(&home), target).install(&extension, "0.1.0").unwrap();

    // Renoise 3.5.5's first run copied 3.5.4's folder, Kumi's tool in it: an update brings both along, from 3.5.4.
    let tool = |version: &str| dir.path().join("Renoise").join(version).join("Scripts").join("Tools").join("com.kumi.Bridge.xrnx");
    copy_folder(&tool("V3.5.4"), &tool("V3.5.5"));
    out.0.borrow_mut().clear();
    assert_eq!(registry::RENOISE.after_update(&io, &home, &later.to_string_lossy(), None).await, 0);
    assert!(out.0.borrow().contains("Kumi's tool 1.0.1 is in Renoise now"), "{}", out.0.borrow());
    for version in ["V3.5.4", "V3.5.5"] {
        let lua = std::fs::read_to_string(tool(version).join("payload/version.lua")).unwrap();
        assert!(lua.contains("\"1.0.1\""), "{version}: {lua}");
    }
    let receipt = receipt_of(&env, Daw::Renoise);
    assert_eq!(receipt.target, tool("V3.5.4").to_string_lossy(), "setup chose 3.5.4");
    assert_eq!(
        receipt.copies.iter().map(|copy| copy.target.clone()).collect::<Vec<_>>(),
        vec![tool("V3.5.5").to_string_lossy().into_owned()]
    );

    // The producer took the tool out of both: the next update, even one bringing the same tool, leaves it out.
    for version in ["V3.5.4", "V3.5.5"] {
        std::fs::remove_dir_all(tool(version)).unwrap();
    }
    out.0.borrow_mut().clear();
    assert_eq!(registry::RENOISE.after_update(&io, &home, &later.to_string_lossy(), None).await, 0);
    let said = out.0.borrow().clone();
    assert!(said.starts_with("Kumi's tool isn't in Renoise any more (") && said.contains(" --daw renoise bridge puts it back."), "{said}");
    assert!(!tool("V3.5.4").exists() && !tool("V3.5.5").exists());

    // REAPER's folder moved (an unplugged drive, say) while another REAPER is here: nothing goes into that one.
    std::fs::rename(&resource, dir.path().join("REAPER moved")).unwrap();
    for default in ["Library/Application Support/REAPER", "AppData/Roaming/REAPER", ".config/REAPER"] {
        std::fs::create_dir_all(dir.path().join(default)).unwrap();
        std::fs::write(dir.path().join(default).join("reaper.ini"), "[REAPER]\n").unwrap();
    }
    let mut unset = env.clone();
    unset.remove("KUMI_REAPER_RESOURCE_DIR");
    let io = kumi::install::InstalledIo::new(out.clone(), unset);
    out.0.borrow_mut().clear();
    assert_eq!(registry::REAPER.after_update(&io, &home, &later.to_string_lossy(), None).await, 0);
    let said = out.0.borrow().clone();
    assert!(
        said.starts_with("Kumi's extension isn't in REAPER any more (") && said.contains(" --daw reaper bridge puts it back."),
        "{said}"
    );
    for default in ["Library/Application Support/REAPER", "AppData/Roaming/REAPER", ".config/REAPER"] {
        assert!(!dir.path().join(default).join("UserPlugins").exists(), "{default}");
    }
}

/// A Kumi an earlier updater put in place (1.19's knows no DAW bridge) brings the bridge it carries as a session starts;
/// once that's in, starting again changes nothing, and a tool the producer took out stays out.
#[tokio::test(flavor = "current_thread")]
async fn a_kumi_an_earlier_updater_put_in_place_brings_its_bridge_as_it_starts() {
    use daw_bridge::daw::Daw;
    let (dir, env) = renoise_home();
    let tools = dir.path().join("Renoise/V3.5.4/Scripts/Tools");
    let folder = daw_bridge::install::RENOISE_TOOL_FOLDER;
    let target = daw_bridge::lifecycle::Target {
        daw: Daw::Renoise,
        dir: tools.clone(),
        name: folder.into(),
        folder: true,
        reference: tools.join(folder).join(daw_bridge::install::RENOISE_REFERENCE),
    };
    // Renoise's tool 0.9.0, as the Kumi before put it in; this checkout carries its own (tool_version).
    let earlier = app_carrying(dir.path(), "app.earlier", b"x", "0.0.9", "0.9.0").join("daws/renoise").join(folder);
    let home = std::path::PathBuf::from(&env["KUMI_HOME"]);
    let put_earlier = || daw_bridge::lifecycle::Lifecycle::new(&home, target.clone()).install(&earlier, "0.9.0").unwrap();
    put_earlier();
    let out = Rc::new(Out::default());
    let io = kumi::install::InstalledIo::new(out.clone(), env.clone());
    registry::RENOISE.finish_transition(&io).await;
    let tool = tool_version();
    assert!(out.0.borrow().contains(&format!("Kumi's tool {tool} is in Renoise now: restart Renoise if it's open.")), "{}", out.0.borrow());
    assert_eq!(receipt_of(&env, Daw::Renoise).version.as_deref(), Some(tool.as_str()));
    out.0.borrow_mut().clear();
    registry::RENOISE.finish_transition(&io).await;
    assert_eq!(out.0.borrow().as_str(), "", "in already: nothing to do");
    // Taken out by the producer: starting doesn't put it back.
    put_earlier();
    std::fs::remove_dir_all(tools.join(folder)).unwrap();
    registry::RENOISE.finish_transition(&io).await;
    assert!(!tools.join(folder).exists());
    assert_eq!(out.0.borrow().as_str(), "");
}

/// In a checkout, `kumi update` builds REAPER's extension before putting it in (the workspace's build leaves it out), and
/// says when that fails rather than put in whatever an earlier build left.
#[tokio::test(flavor = "current_thread")]
async fn a_checkout_update_builds_reapers_extension_and_says_when_that_fails() {
    use daw_bridge::daw::Daw;
    let (dir, mut env) = renoise_home();
    let resource = dir.path().join("REAPER");
    std::fs::create_dir_all(resource.join("UserPlugins")).unwrap();
    std::fs::write(resource.join("reaper.ini"), "[REAPER]\n").unwrap();
    env.insert("KUMI_REAPER_RESOURCE_DIR".into(), resource.to_string_lossy().into_owned());
    let plugins = resource.join("UserPlugins");
    let target = daw_bridge::lifecycle::Target {
        daw: Daw::Reaper,
        dir: plugins.clone(),
        name: daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current()).into(),
        folder: false,
        reference: plugins.join("kumi-bridge-reference.json"),
    };
    let earlier = dir.path().join("earlier-build");
    std::fs::write(&earlier, b"an earlier build").unwrap();
    daw_bridge::lifecycle::Lifecycle::new(std::path::Path::new(&env["KUMI_HOME"]), target.clone()).install(&earlier, "0.1.0").unwrap();
    let asked: Rc<std::cell::RefCell<Vec<(String, Vec<String>, Option<String>)>>> = Rc::default();
    let run: kumi::process::Run = Rc::new({
        let asked = asked.clone();
        move |command, args, cwd| {
            asked.borrow_mut().push((command, args, cwd));
            async { kumi::process::Ran { code: 101, stdout: String::new(), stderr: "Compiling reaper-kumi\nerror: linking failed".into() } }
                .boxed_local()
        }
    });
    let out = Rc::new(Out::default());
    let io = kumi::update::UpdateIo::new(out.clone(), env.clone());
    assert_eq!(registry::REAPER.after_checkout_update(&io, run, "/repo").await, 0);
    let (command, args, cwd) = asked.borrow()[0].clone();
    assert_eq!((command.as_str(), cwd.as_deref()), ("cargo", Some("/repo")));
    assert!(args.windows(2).any(|pair| pair == ["-p", "reaper-kumi"]) && args[0] == "build", "{args:?}");
    let said = out.0.borrow().clone();
    assert!(said.contains("Kumi's extension wasn't brought up to date: building it failed (error: linking failed)."), "{said}");
    assert_eq!(std::fs::read(target.path()).unwrap(), b"an earlier build", "nothing else went in");
}

fn receipt_of(env: &Env, daw: daw_bridge::daw::Daw) -> daw_bridge::lifecycle::Receipt {
    let paths = daw_bridge::paths::BridgePaths::new(std::path::Path::new(&env["KUMI_HOME"]), daw);
    serde_json::from_str(&std::fs::read_to_string(paths.receipt()).unwrap()).unwrap()
}

/// An update that brings the bridges in place changes nothing (so the one to roll back to stays); Kumi's rollback takes
/// each to the copy the Kumi it goes back to carries, and the next rollback back again, from the generations kept; an
/// earlier Kumi that knows neither DAW leaves both where they are. An update finds REAPER's extension where Kumi put it.
#[tokio::test(flavor = "current_thread")]
async fn an_update_keeps_the_bridges_in_place_and_a_rollback_takes_each_to_the_copy_its_kumi_carries() {
    use daw_bridge::daw::Daw;
    let (dir, mut env) = renoise_home();
    let resource = dir.path().join("REAPER");
    std::fs::create_dir_all(resource.join("UserPlugins")).unwrap();
    std::fs::write(resource.join("reaper.ini"), "[REAPER]\n").unwrap();
    env.insert("KUMI_REAPER_RESOURCE_DIR".into(), resource.to_string_lossy().into_owned());
    // (`now` carries the tool setup puts in, this checkout's)
    let tool = tool_version();
    let now = app_carrying(dir.path(), "app", b"extension 0.1.0", "0.1.0", &tool);
    let earlier = app_carrying(dir.path(), "app.previous", b"extension 0.0.9", "0.0.9", "0.9.0");
    let unknowing = dir.path().join("app.older");
    std::fs::create_dir_all(&unknowing).unwrap();
    let home = env["KUMI_HOME"].clone();
    let out = Rc::new(Out::default());
    let io = kumi::install::InstalledIo::new(out.clone(), env.clone());

    // Both in, as `kumi --daw renoise bridge` and `kumi --daw reaper bridge` put them (REAPER's from `now`).
    assert_eq!(registry::RENOISE.setup_bridge(out.clone(), &env, yes()).await.unwrap(), 0, "{}", out.0.borrow());
    let extension = now.join("daws/reaper").join(daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current()));
    let target = daw_bridge::install::reaper_target(
        &daw_bridge::detect::ReaperInstall {
            app: None,
            version: None,
            archs: vec![],
            resource: resource.clone(),
            resource_from: daw_bridge::detect::ResourceFrom::Override,
            opened: true,
        },
        daw_bridge::detect::Os::current(),
    )
    .unwrap();
    daw_bridge::lifecycle::Lifecycle::new(std::path::Path::new(&home), target).install(&extension, "0.1.0").unwrap();
    let installed = [receipt_of(&env, Daw::Reaper), receipt_of(&env, Daw::Renoise)];

    // `kumi update` to a Kumi bringing the same bridges, twice: nothing changes, nothing is said.
    out.0.borrow_mut().clear();
    for _ in 0..2 {
        for provider in [&registry::REAPER as &dyn DawProvider, &registry::RENOISE] {
            assert_eq!(provider.after_update(&io, &home, &now.to_string_lossy(), None).await, 0);
        }
    }
    assert_eq!([receipt_of(&env, Daw::Reaper), receipt_of(&env, Daw::Renoise)], installed);
    assert_eq!(out.0.borrow().as_str(), "");

    // Rolled back to a Kumi carrying 0.0.9 and 0.9.0, then forward again.
    for (app, kumi_version, reaper, renoise) in [(&earlier, "1.19.2", "0.0.9", "0.9.0"), (&now, "1.19.3", "0.1.0", tool.as_str())] {
        out.0.borrow_mut().clear();
        for provider in [&registry::REAPER as &dyn DawProvider, &registry::RENOISE] {
            let step = provider.prepare_rollback(&io, &home, &app.to_string_lossy(), kumi_version, false).await.unwrap();
            assert_eq!(step.after_swap(&io, &home, &app.to_string_lossy(), kumi_version, true).await, 0);
        }
        let said = out.0.borrow().clone();
        assert!(
            said.contains(&format!(
                "Kumi's extension {reaper} is back in REAPER, as Kumi {kumi_version} had it: restart REAPER if it's open."
            )),
            "{said}"
        );
        assert!(said.contains(&format!("Kumi's tool {renoise} is back in Renoise, as Kumi {kumi_version} had it")), "{said}");
        let (reaper_receipt, renoise_receipt) = (receipt_of(&env, Daw::Reaper), receipt_of(&env, Daw::Renoise));
        assert_eq!((reaper_receipt.version.as_deref(), renoise_receipt.version.as_deref()), (Some(reaper), Some(renoise)));
        assert_eq!((reaper_receipt.history[0].action.as_str(), renoise_receipt.history[0].action.as_str()), ("rollback", "rollback"));
        let placed = std::fs::read(resource.join("UserPlugins").join(extension.file_name().unwrap())).unwrap();
        assert_eq!(placed, format!("extension {reaper}").into_bytes());
    }
    // Forward again is the generation kept from the first install, not a third copy.
    assert_eq!(receipt_of(&env, Daw::Reaper).generation, installed[0].generation);
    assert_eq!(receipt_of(&env, Daw::Renoise).generation, installed[1].generation);

    // An earlier Kumi that knows neither DAW: both stay, said; and as KUMI_DAW names Renoise, which that Kumi refuses to
    // start with, that's said too.
    out.0.borrow_mut().clear();
    let mut asking = env.clone();
    asking.insert("KUMI_DAW".into(), "renoise".into());
    let asking = kumi::install::InstalledIo::new(out.clone(), asking);
    for provider in [&registry::REAPER as &dyn DawProvider, &registry::RENOISE] {
        let step = provider.prepare_rollback(&asking, &home, &unknowing.to_string_lossy(), "1.19.1", false).await.unwrap();
        assert_eq!(step.after_swap(&asking, &home, &unknowing.to_string_lossy(), "1.19.1", true).await, 0);
    }
    let said = out.0.borrow().clone();
    assert!(said.contains("Kumi's extension stays in REAPER (") && said.contains("Kumi 1.19.1 doesn't know REAPER"), "{said}");
    assert!(said.contains("Kumi's tool stays in Renoise (") && said.contains("Kumi 1.19.1 doesn't know Renoise"), "{said}");
    assert!(
        said.contains(
            "KUMI_DAW is set to renoise where you run Kumi, and Kumi 1.19.1 doesn't know Renoise: it won't start while it's set."
        ),
        "{said}"
    );
    assert!(!said.contains("KUMI_DAW is set to reaper"), "{said}");
    assert_eq!(receipt_of(&env, Daw::Reaper).version.as_deref(), Some("0.1.0"));

    // An update with KUMI_REAPER_RESOURCE_DIR no longer set, and a REAPER with its default resource folder too:
    // REAPER's extension is updated where Kumi put it, not moved into the other REAPER.
    for default in ["Library/Application Support/REAPER", "AppData/Roaming/REAPER", ".config/REAPER"] {
        std::fs::create_dir_all(dir.path().join(default)).unwrap();
        std::fs::write(dir.path().join(default).join("reaper.ini"), "[REAPER]\n").unwrap();
    }
    let mut unset = env.clone();
    unset.remove("KUMI_REAPER_RESOURCE_DIR");
    let io = kumi::install::InstalledIo::new(out.clone(), unset);
    out.0.borrow_mut().clear();
    assert_eq!(registry::REAPER.after_update(&io, &home, &earlier.to_string_lossy(), None).await, 0);
    assert!(out.0.borrow().contains("Kumi's extension 0.0.9 is in REAPER now: restart REAPER if it's open."), "{}", out.0.borrow());
    assert_eq!(receipt_of(&env, Daw::Reaper).target, installed[0].target);
    for default in ["Library/Application Support/REAPER", "AppData/Roaming/REAPER", ".config/REAPER"] {
        assert!(!dir.path().join(default).join("UserPlugins").exists(), "{default}");
    }
}

/// Review K19 and U6: setup opens the app it found when the DAW hasn't been opened yet (never an empty name, which
/// opened nothing), and names a REAPER older than 7 by its version rather than put the extension in it.
#[tokio::test(flavor = "current_thread")]
async fn setup_opens_the_app_it_found_and_names_an_older_reaper() {
    use kumi::daw::{older_than, Installed};
    let dir = tempfile::tempdir().unwrap();
    let app = if cfg!(target_os = "macos") {
        dir.path().join("Applications/REAPER.app")
    } else if cfg!(windows) {
        dir.path().join("Program Files/REAPER/reaper.exe")
    } else {
        dir.path().join("opt/REAPER/reaper")
    };
    let plist = |version: &str| format!("<plist><dict><key>CFBundleShortVersionString</key>\n<string>{version}</string></dict></plist>");
    if cfg!(target_os = "macos") {
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/REAPER"), "").unwrap();
        std::fs::write(app.join("Contents/Info.plist"), plist("7.22")).unwrap();
    } else {
        std::fs::create_dir_all(app.parent().unwrap()).unwrap();
        std::fs::write(&app, "").unwrap();
    }
    let env = |resource: &std::path::Path| {
        Env::from([
            ("HOME".into(), dir.path().to_string_lossy().into_owned()),
            ("KUMI_HOME".into(), dir.path().join(".kumi").to_string_lossy().into_owned()),
            ("KUMI_REAPER_RESOURCE_DIR".into(), resource.to_string_lossy().into_owned()),
            ("ProgramFiles".into(), dir.path().join("Program Files").to_string_lossy().into_owned()),
        ])
    };
    let setup = |env: &Env| registry::REAPER.setup(env, Rc::default(), false, Rc::default());
    // Installed, never opened (no resource folder yet): setup opens this app.
    let never = setup(&env(&dir.path().join("REAPER"))).installed().await;
    let Installed::NeverOpened(found) = never else { panic!("{never:?}") };
    assert!(found.ends_with(app.file_name().unwrap().to_str().unwrap()), "{found}");
    // A portable REAPER 6 (its reaper.ini beside the app; macOS apps say their version): named, and older than Kumi needs.
    if cfg!(target_os = "macos") {
        std::fs::write(app.join("Contents/Info.plist"), plist("6.83")).unwrap();
        std::fs::write(dir.path().join("Applications/reaper.ini"), "").unwrap();
        let older = setup(&env(&dir.path().join("Applications")));
        assert_eq!(older.installed().await, Installed::Older("6.83".into()));
        let version = older.version(Some(app.to_string_lossy().into_owned())).await.unwrap();
        assert!(older_than(&version, DawProvider::min_version(&registry::REAPER)), "{version}");
        assert!(
            !older_than("3.5.4", DawProvider::min_version(&registry::RENOISE))
                && older_than("3.4.4", DawProvider::min_version(&registry::RENOISE))
        );
        assert!(older_than("11.3.21", "12") && !older_than("12.0b4", "12") && !older_than("unknown", "12"));
    }
}

/// Review K20: a portable REAPER Kumi doesn't find by itself is found once its receipt says the extension went in it;
/// with that folder gone (the drive is out), the doctor names the variable that shows Kumi where REAPER is.
#[tokio::test(flavor = "current_thread")]
async fn a_portable_reaper_is_found_by_its_receipt_and_otherwise_named_by_the_variable() {
    use kumi::daw::Installed;
    let dir = tempfile::tempdir().unwrap();
    let env = Env::from([
        ("HOME".into(), dir.path().to_string_lossy().into_owned()),
        ("KUMI_HOME".into(), dir.path().join(".kumi").to_string_lossy().into_owned()),
        ("ProgramFiles".into(), dir.path().join("Program Files").to_string_lossy().into_owned()),
    ]);
    let reaper: &dyn DawProvider = &registry::REAPER;
    // A REAPER installed on this computer itself would be found anyway.
    let alone = !reaper.here(&env);
    let portable = dir.path().join("Stick/REAPER");
    std::fs::create_dir_all(portable.join("UserPlugins")).unwrap();
    std::fs::write(portable.join("reaper.ini"), "").unwrap();
    let target = portable.join("UserPlugins").join(daw_bridge::install::reaper_extension_name(daw_bridge::detect::Os::current()));
    let paths = daw_bridge::paths::BridgePaths::new(&dir.path().join(".kumi"), daw_bridge::daw::Daw::Reaper);
    std::fs::create_dir_all(&paths.root).unwrap();
    let receipt = serde_json::json!({
        "schema": daw_bridge::lifecycle::RECEIPT_SCHEMA, "daw": "reaper", "status": "installed",
        "target": target, "reference": portable.join("UserPlugins/kumi-bridge-reference.json"),
        "previous": [], "updatedMs": 0, "history": [],
    });
    std::fs::write(paths.receipt(), receipt.to_string()).unwrap();
    assert!(reaper.here(&env));
    assert_eq!(reaper.setup(&env, Rc::default(), false, Rc::default()).installed().await, Installed::Ready);
    std::fs::remove_dir_all(dir.path().join("Stick")).unwrap();
    if alone {
        let checks = reaper.installation_checks(&env, &kumi::doctor::InstallationIo::default(), "darwin").await;
        let said = format!("{checks:?}");
        assert!(said.contains("set KUMI_REAPER_RESOURCE_DIR to its resource folder"), "{said}");
    }
}

/// Review U13 and K42: each of the doctor's REAPER and Renoise failures names what puts it right (the bridge command,
/// or opening the DAW once), and a host that won't start is a failure with the repair to run.
#[tokio::test(flavor = "current_thread")]
async fn the_doctor_names_each_fix_and_says_when_kumis_host_wont_start() {
    use kumi::doctor::{Check, CheckStatus, DoctorIo, LiveProbe};
    let (dir, env) = renoise_home();
    assert_eq!(registry::RENOISE.setup_bridge(Rc::new(Out::default()), &env, yes()).await.unwrap(), 0);
    // The producer deleted Kumi's tool from Renoise.
    std::fs::remove_dir_all(dir.path().join("Renoise/V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx")).unwrap();
    let mut io = DoctorIo::new(Rc::new(Out::default()), env.clone());
    io.probe_own = Some(Rc::new(|_| {
        async { LiveProbe { started: false, said: Some("daw-mcp-server wasn't found".into()), ..Default::default() } }.boxed_local()
    }));
    let checks = registry::RENOISE.doctor_checks(&io, &Check::ok("Kumi")).await;
    let again = format!("{} --daw renoise bridge", *kumi_types::command::KUMI);
    let removed = checks.iter().find(|check| check.text.contains("was removed from")).unwrap_or_else(|| panic!("{checks:?}"));
    assert_eq!((removed.status.clone(), removed.next.as_deref()), (CheckStatus::Fix, Some(again.as_str())), "{checks:?}");
    let host = checks.iter().find(|check| check.text == "Kumi's host for Renoise didn't start: daw-mcp-server wasn't found");
    let host = host.unwrap_or_else(|| panic!("{checks:?}"));
    assert_eq!(host.status, CheckStatus::Fix);
    assert!(host.next.as_deref().is_some_and(|next| next.contains(kumi_types::command::KUMI_REPAIR.as_str())), "{host:?}");
}

/// Review K43: the report reads the bridge's logs oldest first, whatever order their folder lists them in.
#[tokio::test(flavor = "current_thread")]
async fn the_report_reads_the_bridge_logs_oldest_first() {
    let (dir, env) = renoise_home();
    let logs = dir.path().join(".kumi/bridges/renoise/logs");
    std::fs::create_dir_all(&logs).unwrap();
    let now = std::time::SystemTime::now();
    for (name, hours_ago) in [("a.log", 1), ("b.log", 3), ("c.log", 2)] {
        std::fs::write(logs.join(name), format!("{name} line\n")).unwrap();
        let file = std::fs::File::options().write(true).open(logs.join(name)).unwrap();
        file.set_modified(now - std::time::Duration::from_secs(hours_ago * 3600)).unwrap();
    }
    let (heading, lines) = registry::RENOISE.report_log(&env, "", None).await;
    assert_eq!(heading, "Renoise's bridge log");
    assert_eq!(lines, "b.log line\nc.log line\na.log line");
}
