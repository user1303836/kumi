//! Doctor's checks for one DAW's bridge: installed, opened once, the bridge's files and their hash, the secret, the
//! DAW running, the bridge answering and its version, ports, quarantine, architecture, and Renoise's own settings.
use crate::daw::Daw;
use crate::detect::{self, Env, Os};
use crate::files::{read_secret, scan_discovery};
use crate::install::{self, RENOISE_TOOL_ID};
use crate::lifecycle::{Lifecycle, Target};
use crate::owner;
use crate::paths::BridgePaths;
use crate::probe::{listening, probe};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    /// Worth knowing, nothing wrong.
    Note,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub level: Level,
    pub message: String,
}

fn check(id: &'static str, level: Level, message: impl Into<String>) -> Check {
    Check { id, level, message: message.into() }
}

/// What this Kumi ships for the DAW, to compare with what's installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundled {
    pub version: String,
    pub sha256: String,
}

/// What doctor reads besides the file system: the running DAW's pids (one process listing), injectable for tests.
pub struct Inputs<'a> {
    pub env: Env<'a>,
    pub kumi_home: &'a Path,
    pub bundled: Option<Bundled>,
    pub running: &'a dyn Fn(Daw) -> Vec<u32>,
    pub probe_timeout: Duration,
}

pub fn doctor(daw: Daw, inputs: &Inputs) -> Vec<Check> {
    let mut checks = vec![];
    let target = match daw {
        Daw::Reaper => reaper_target(inputs, &mut checks),
        Daw::Renoise => renoise_target(inputs, &mut checks),
    };
    let Some((target, app_archs)) = target else { return checks };
    bridge_checks(daw, &target, &app_archs, inputs, &mut checks);
    checks
}

/// Where the bridge's receipt says it was placed, if it was.
fn receipt_target(inputs: &Inputs, any_target: &Target) -> Option<std::path::PathBuf> {
    let receipt = Lifecycle::new(inputs.kumi_home, any_target.clone()).receipt().ok()??;
    (receipt.status == "installed").then(|| receipt.target.into())
}

fn reaper_target(inputs: &Inputs, checks: &mut Vec<Check>) -> Option<(Target, Vec<detect::Arch>)> {
    // A portable REAPER Kumi doesn't find by itself counts once its receipt says the extension went in it.
    let receipt = std::fs::read_to_string(BridgePaths::new(inputs.kumi_home, Daw::Reaper).receipt())
        .ok()
        .and_then(|text| serde_json::from_str::<crate::lifecycle::Receipt>(&text).ok())
        .filter(|receipt| receipt.status == "installed")
        .map(|receipt| std::path::PathBuf::from(receipt.target));
    let installs = detect::with_placed(&inputs.env, detect::detect_reaper(&inputs.env), receipt.as_deref());
    // A REAPER running from anywhere (a portable one on another drive) counts too.
    let mut installs = detect::with_running(&inputs.env, installs, &detect::running_programs(Daw::Reaper, &inputs.env));
    if installs.is_empty() {
        checks.push(check(
            "installed",
            Level::Note,
            "REAPER isn't where Kumi looks (open it, and Kumi finds it; or set KUMI_REAPER_RESOURCE_DIR to its resource folder)",
        ));
        return None;
    }
    // The REAPER the bridge was installed into comes first, not always the default one: a portable install has a
    // resource folder of its own, and the bridge's files, reference and architecture are checked in that one.
    let placed = installs
        .iter()
        .filter_map(|install| install::reaper_target(install, inputs.env.os).ok())
        .find_map(|any| receipt_target(inputs, &any))
        .and_then(|receipt| {
            installs.iter().position(|install| install::reaper_target(install, inputs.env.os).is_ok_and(|target| target.path() == receipt))
        });
    if let Some(index) = placed {
        let install = installs.remove(index);
        installs.insert(0, install);
    }
    let install = &installs[0];
    let app = match (&install.app, &install.version) {
        (Some(app), Some(version)) => format!("REAPER {version} at {}", app.display()),
        (Some(app), None) => format!("REAPER at {}", app.display()),
        (None, _) => format!("REAPER's resource folder {} (the app wasn't found)", install.resource.display()),
    };
    checks.push(check("installed", Level::Ok, app));
    if installs.len() > 1 {
        let others: Vec<String> = installs[1..].iter().map(|other| other.resource.display().to_string()).collect();
        checks.push(check("installs", Level::Note, format!("other REAPER resource folders: {}", others.join(", "))));
    }
    if !install.opened {
        checks.push(check("opened", Level::Warn, format!("REAPER hasn't run with {} yet: open it once", install.resource.display())));
    }
    match install::reaper_target(install, inputs.env.os) {
        Ok(target) => Some((target, install.archs.clone())),
        Err(why) => {
            checks.push(check("opened", Level::Fail, why.to_string()));
            None
        }
    }
}

fn renoise_target(inputs: &Inputs, checks: &mut Vec<Check>) -> Option<(Target, Vec<detect::Arch>)> {
    let found = detect::detect_renoise(&inputs.env);
    if found.apps.is_empty() && found.prefs.is_empty() {
        checks.push(check(
            "installed",
            Level::Note,
            "Renoise isn't where Kumi looks (open it once, so it makes its preferences folder; or set KUMI_RENOISE_PREFS_DIR to that folder)",
        ));
        return None;
    }
    for app in &found.apps {
        let version = app.version.as_deref().unwrap_or("(version unknown)");
        checks.push(check("installed", Level::Ok, format!("Renoise {version} at {}", app.path.display())));
        if app.demo == Some(true) {
            checks.push(check("demo", Level::Note, "this is Renoise's demo: it can't render, so Kumi can't listen to songs offline"));
        }
    }
    let target = match install::renoise_target(&found) {
        Ok(target) => target,
        Err(why) => {
            checks.push(check("opened", Level::Fail, why.to_string()));
            return None;
        }
    };
    if let Some(prefs) = &found.target {
        if let Some(config) = detect::renoise_config(&prefs.dir, RENOISE_TOOL_ID) {
            if config.osc_server {
                checks.push(check(
                    "osc",
                    Level::Warn,
                    "Renoise's OSC server is on: it takes commands from the network without a password (Kumi doesn't need it; Preferences > OSC turns it off)",
                ));
            }
            if config.tool_disabled {
                checks.push(check("disabled", Level::Fail, "Kumi's tool is disabled in Renoise: turn it on in Tools > Tool Browser"));
            }
        }
    }
    let archs = found.apps.first().map(|app| app.archs.clone()).unwrap_or_default();
    Some((target, archs))
}

fn bridge_checks(daw: Daw, target: &Target, app_archs: &[detect::Arch], inputs: &Inputs, checks: &mut Vec<Check>) {
    let lifecycle = Lifecycle::new(inputs.kumi_home, target.clone());
    let paths = BridgePaths::new(inputs.kumi_home, daw);
    let installed_version = match lifecycle.status() {
        Err(error) => {
            checks.push(check("bridge", Level::Fail, format!("Kumi's {daw} bridge receipt can't be read: {error}")));
            None
        }
        Ok(status) => match &status.receipt {
            None => {
                let level = if target.path().exists() { Level::Warn } else { Level::Fail };
                let message = if target.path().exists() {
                    format!(
                        "{} is there but Kumi has no record of installing it: set up Kumi's {daw} bridge again",
                        target.path().display()
                    )
                } else {
                    format!("Kumi's {daw} bridge isn't installed")
                };
                checks.push(check("bridge", level, message));
                None
            }
            Some(receipt) if !status.present => {
                checks.push(check(
                    "bridge",
                    Level::Fail,
                    format!("Kumi's {daw} bridge was removed from {}: set it up again", receipt.target),
                ));
                None
            }
            Some(receipt) => {
                let version = receipt.version.clone().unwrap_or_default();
                let sha = receipt.sha256.clone().unwrap_or_default();
                if status.matches {
                    checks.push(check("bridge", Level::Ok, format!("bridge {version} installed ({})", &sha[..sha.len().min(12)])));
                } else {
                    checks.push(check(
                        "bridge",
                        Level::Warn,
                        format!("bridge {version} was changed since Kumi installed it: set it up again"),
                    ));
                }
                if let Some(bundled) = &inputs.bundled {
                    if bundled.sha256 != sha {
                        checks.push(check(
                            "update",
                            Level::Warn,
                            format!(
                                "this Kumi ships bridge {} and {daw} has {version}: kumi --daw {} bridge brings it up to date",
                                bundled.version,
                                daw.id()
                            ),
                        ));
                    }
                }
                if !status.reference {
                    checks.push(check(
                        "reference",
                        Level::Fail,
                        format!("the bridge's reference file is missing: {}", target.reference.display()),
                    ));
                }
                Some(version)
            }
        },
    };
    match read_secret(&paths.secret()) {
        Ok(_) if inputs.env.os == Os::Windows && !owner::is_owner_only(&paths.secret()) => {
            checks.push(check("secret", Level::Fail, "the bridge's secret is readable by other accounts"));
        }
        Ok(_) => {}
        Err(error) => checks.push(check("secret", Level::Fail, format!("the bridge's secret: {error}"))),
    }
    let file = target.path();
    if file.exists() {
        let quarantined = if target.folder {
            detect::quarantined(&file.join("main.lua")) || detect::quarantined(&file)
        } else {
            detect::quarantined(&file)
        };
        if quarantined {
            checks.push(check(
                "quarantine",
                Level::Fail,
                format!("macOS marked {} as downloaded, so {daw} won't load it: set up the bridge again to clear it", file.display()),
            ));
        }
        if !target.folder && !install::arch_fits(&file, app_archs) {
            checks.push(check("arch", Level::Fail, format!("the extension's architecture doesn't match {daw}'s ({app_archs:?})")));
        }
    }

    let pids = (inputs.running)(daw);
    let discovered = scan_discovery(&paths.discovery(), daw);
    // A discovery file with no pid (Renoise on Windows writes none) is alive only while its port answers: one whose port
    // nobody listens on is left from a run that ended (a crash, a folder the tool couldn't delete it from), so it goes, as
    // the host sweeps it.
    let alive: Vec<_> = discovered
        .iter()
        .filter(|found| found.alive)
        .filter_map(|found| found.discovery.as_ref().ok().map(|discovery| (found, discovery)))
        .filter(|(found, discovery)| {
            let stale = discovery.pid == 0 && !listening(discovery.port, inputs.probe_timeout.min(Duration::from_millis(500)));
            if stale {
                let _ = std::fs::remove_file(&found.path);
            }
            !stale
        })
        .map(|(_, discovery)| discovery)
        .collect();
    if pids.is_empty() && alive.is_empty() {
        checks.push(check("running", Level::Note, format!("{daw} isn't running")));
    } else if !pids.is_empty() {
        checks.push(check(
            "running",
            Level::Ok,
            format!("{daw} is running (pid {})", pids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")),
        ));
    }
    if !pids.is_empty() && alive.is_empty() && installed_version.is_some() {
        checks.push(check("answering", Level::Warn, format!("{daw} is running without Kumi's bridge: restart {daw} to load it")));
    }
    let secret = read_secret(&paths.secret()).ok();
    for discovery in &alive {
        let Some(secret) = &secret else { break };
        let address = SocketAddr::from(([127, 0, 0, 1], discovery.port));
        match probe(address, secret, "kumi-doctor", inputs.probe_timeout) {
            Ok(found) => {
                let identity = &found.welcome.identity;
                let project = identity.project.as_ref().and_then(|project| project.name.clone()).unwrap_or_else(|| "no project".into());
                checks.push(check(
                    "answering",
                    Level::Ok,
                    format!("bridge {} answering on port {} ({daw} {}, {project})", identity.bridge, discovery.port, identity.daw_version),
                ));
                // What this version of the DAW lacks that the bridge reads with (REAPER's status names it): those reads come
                // back empty, and nothing else says why.
                let missing = found.status.as_ref().and_then(|status| status.get("missing")).and_then(|missing| missing.as_array());
                for missing in missing.into_iter().flatten() {
                    let (Some(feature), Some(needs)) =
                        (missing.get("feature").and_then(|v| v.as_str()), missing.get("needs").and_then(|v| v.as_str()))
                    else {
                        continue;
                    };
                    checks.push(check(
                        "missing",
                        Level::Warn,
                        format!("{daw} {} lacks what Kumi reads {feature} with: update to {daw} {needs} or newer", identity.daw_version),
                    ));
                }
                if installed_version.as_ref().is_some_and(|installed| *installed != identity.bridge) {
                    checks.push(check(
                        "restart",
                        Level::Warn,
                        format!(
                            "{daw} (pid {}) still runs bridge {}: restart {daw} to load {}",
                            identity.pid,
                            identity.bridge,
                            installed_version.as_deref().unwrap_or("")
                        ),
                    ));
                }
            }
            Err(error) => {
                checks.push(check("answering", Level::Fail, format!("{daw}'s bridge on port {} didn't answer: {error}", discovery.port)))
            }
        }
    }
    let ours: Vec<u16> = alive.iter().map(|discovery| discovery.port).collect();
    let taken: Vec<String> = daw
        .ports()
        .filter(|port| !ours.contains(port) && listening(*port, Duration::from_millis(200)))
        .map(|port| port.to_string())
        .collect();
    if !taken.is_empty() {
        checks.push(check(
            "ports",
            Level::Note,
            format!("another program listens on port {} of {daw}'s range: the bridge takes the next free one", taken.join(", ")),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::{write_discovery, Discovery, DISCOVERY_SCHEMA};
    use crate::wire::{Identity, PROTOCOL};

    fn no_processes(_: Daw) -> Vec<u32> {
        vec![]
    }

    fn running(_: Daw) -> Vec<u32> {
        vec![4242]
    }

    fn ids(checks: &[Check]) -> Vec<(&'static str, Level)> {
        checks.iter().map(|check| (check.id, check.level)).collect()
    }

    #[test]
    fn renoise_from_nothing_to_answering() {
        let root = tempfile::tempdir().unwrap();
        let kumi = root.path().join("kumi");
        let none = |_: &str| None;
        let inputs = |running: &'static dyn Fn(Daw) -> Vec<u32>| Inputs {
            env: Env { os: Os::Mac, home: root.path().join("home"), var: &none, root: Some(root.path().join("sys")) },
            kumi_home: &kumi,
            bundled: None,
            running,
            probe_timeout: Duration::from_secs(5),
        };
        assert_eq!(ids(&doctor(Daw::Renoise, &inputs(&no_processes))), [("installed", Level::Note)]);

        let prefs = root.path().join("home/Library/Preferences/Renoise/V3.5.4");
        std::fs::create_dir_all(&prefs).unwrap();
        std::fs::write(prefs.join("Config.xml"), "<OscServerEnabled>true</OscServerEnabled>").unwrap();
        let checks = doctor(Daw::Renoise, &inputs(&no_processes));
        assert!(checks.contains(&check("osc", Level::Warn, checks.iter().find(|c| c.id == "osc").unwrap().message.clone())));
        assert!(ids(&checks).contains(&("bridge", Level::Fail)));
        assert!(ids(&checks).contains(&("secret", Level::Fail)));

        // Installed through the lifecycle.
        let target = install::renoise_target(&detect::detect_renoise(&inputs(&no_processes).env)).unwrap();
        let source = root.path().join("src").join("com.kumi.Bridge.xrnx");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("main.lua"), "-- tool\n").unwrap();
        let receipt = Lifecycle::new(&kumi, target).install(&source, "0.1.0").unwrap();
        let checks = doctor(Daw::Renoise, &inputs(&no_processes));
        assert!(ids(&checks).contains(&("bridge", Level::Ok)), "{checks:?}");
        assert!(!ids(&checks).iter().any(|(id, _)| *id == "secret" || *id == "reference"));
        assert!(ids(&checks).contains(&("running", Level::Note)));

        // Running, but started before the bridge was installed.
        assert!(ids(&doctor(Daw::Renoise, &inputs(&running))).contains(&("answering", Level::Warn)));

        // Answering, with an older bridge than the one installed.
        let secret = read_secret(&BridgePaths::new(&kumi, Daw::Renoise).secret()).unwrap();
        let identity = Identity {
            preset_files: Vec::new(),
            daw: "renoise".into(),
            bridge: "0.0.9".into(),
            daw_version: "3.5.4".into(),
            pid: std::process::id(),
            resource: prefs.to_string_lossy().into_owned(),
            project: None,
        };
        // A DAW too old for some of what the bridge reads says which (REAPER's status does).
        let bridge = crate::probe::testing::serve_with(
            secret,
            identity.clone(),
            serde_json::json!({"missing": [{"feature": "markers and regions", "needs": "3.6"}]}),
        );
        let discovery = Discovery {
            schema: DISCOVERY_SCHEMA.into(),
            daw: identity.daw.clone(),
            pid: identity.pid,
            host: "127.0.0.1".into(),
            port: bridge.address.port(),
            protocol: PROTOCOL.into(),
            version: identity.bridge.clone(),
            daw_version: identity.daw_version.clone(),
            resource: identity.resource.clone(),
            project: None,
            started_ms: 1,
        };
        write_discovery(&BridgePaths::new(&kumi, Daw::Renoise).discovery(), &discovery).unwrap();
        let mut with_bundle = inputs(&running);
        with_bundle.bundled = Some(Bundled { version: "0.2.0".into(), sha256: "other".into() });
        let checks = doctor(Daw::Renoise, &with_bundle);
        assert!(ids(&checks).contains(&("answering", Level::Ok)), "{checks:?}");
        assert!(ids(&checks).contains(&("restart", Level::Warn)));
        assert!(ids(&checks).contains(&("update", Level::Warn)));
        let missing = checks.iter().find(|check| check.id == "missing").expect("what this Renoise lacks");
        assert_eq!(missing.message, "Renoise 3.5.4 lacks what Kumi reads markers and regions with: update to Renoise 3.6 or newer");
        assert_eq!(receipt.version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn a_discovery_file_with_no_pid_whose_port_refuses_is_stale_and_goes() {
        let root = tempfile::tempdir().unwrap();
        let kumi = root.path().join("kumi");
        let none = |_: &str| None;
        let inputs = Inputs {
            env: Env { os: Os::Mac, home: root.path().join("home"), var: &none, root: Some(root.path().join("sys")) },
            kumi_home: &kumi,
            bundled: None,
            running: &no_processes,
            probe_timeout: Duration::from_secs(5),
        };
        // Renoise with Kumi's bridge set up.
        std::fs::create_dir_all(root.path().join("home/Library/Preferences/Renoise/V3.5.4")).unwrap();
        let target = install::renoise_target(&detect::detect_renoise(&inputs.env)).unwrap();
        let source = root.path().join("src").join("com.kumi.Bridge.xrnx");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("main.lua"), "-- tool\n").unwrap();
        Lifecycle::new(&kumi, target).install(&source, "0.1.0").unwrap();
        // A file with no pid (Renoise on Windows writes none) for a port nobody listens on: one just let go.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let discovery = Discovery {
            schema: DISCOVERY_SCHEMA.into(),
            daw: "renoise".into(),
            pid: 0,
            host: "127.0.0.1".into(),
            port,
            protocol: PROTOCOL.into(),
            version: "0.1.0".into(),
            daw_version: "3.5.4".into(),
            resource: root.path().join("home/Library/Preferences/Renoise/V3.5.4").to_string_lossy().into_owned(),
            project: None,
            started_ms: 1,
        };
        let dir = BridgePaths::new(&kumi, Daw::Renoise).discovery();
        write_discovery(&dir, &discovery).unwrap();
        let checks = doctor(Daw::Renoise, &inputs);
        assert!(ids(&checks).contains(&("running", Level::Note)), "Renoise isn't running: {checks:?}");
        assert!(!ids(&checks).contains(&("answering", Level::Fail)), "{checks:?}");
        assert!(!dir.join(discovery.file_name()).exists(), "the stale file went");
    }

    #[test]
    fn reaper_wrong_architecture_and_unrecorded_files() {
        let root = tempfile::tempdir().unwrap();
        let kumi = root.path().join("kumi");
        let none = |_: &str| None;
        let app = root.path().join("sys/Applications/REAPER.app/Contents/MacOS");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("REAPER"), [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 0x01]).unwrap();
        let resource = root.path().join("home/Library/Application Support/REAPER");
        std::fs::create_dir_all(resource.join("UserPlugins")).unwrap();
        std::fs::write(resource.join("reaper.ini"), "").unwrap();
        let inputs = Inputs {
            env: Env { os: Os::Mac, home: root.path().join("home"), var: &none, root: Some(root.path().join("sys")) },
            kumi_home: &kumi,
            bundled: None,
            running: &no_processes,
            probe_timeout: Duration::from_secs(1),
        };
        std::fs::write(resource.join("UserPlugins/reaper_kumi.dylib"), [0xcf, 0xfa, 0xed, 0xfe, 0x07, 0, 0, 0x01]).unwrap();
        assert!(ids(&doctor(Daw::Reaper, &inputs)).contains(&("bridge", Level::Warn)));
        let source = root.path().join("reaper_kumi.dylib");
        std::fs::write(&source, [0xcf, 0xfa, 0xed, 0xfe, 0x07, 0, 0, 0x01]).unwrap();
        let target = install::reaper_target(&detect::detect_reaper(&inputs.env)[0], Os::Mac).unwrap();
        Lifecycle::new(&kumi, target).install(&source, "0.1.0").unwrap();
        let checks = doctor(Daw::Reaper, &inputs);
        assert!(ids(&checks).contains(&("arch", Level::Fail)), "{checks:?}");
    }

    #[test]
    fn reaper_is_checked_where_the_bridge_was_installed_not_in_the_default_folder() {
        let root = tempfile::tempdir().unwrap();
        let kumi = root.path().join("kumi");
        let none = |_: &str| None;
        let binary = [0xcf, 0xfa, 0xed, 0xfe, 0x07, 0, 0, 0x01];
        // The default REAPER, with its resource folder, and a portable one beside its app in ~/Applications.
        let default_app = root.path().join("sys/Applications/REAPER.app/Contents/MacOS");
        std::fs::create_dir_all(&default_app).unwrap();
        std::fs::write(default_app.join("REAPER"), binary).unwrap();
        let default_resource = root.path().join("home/Library/Application Support/REAPER");
        std::fs::create_dir_all(default_resource.join("UserPlugins")).unwrap();
        std::fs::write(default_resource.join("reaper.ini"), "").unwrap();
        let portable_app = root.path().join("home/Applications/REAPER.app/Contents/MacOS");
        std::fs::create_dir_all(&portable_app).unwrap();
        std::fs::write(portable_app.join("REAPER"), binary).unwrap();
        let portable = root.path().join("home/Applications");
        std::fs::create_dir_all(portable.join("UserPlugins")).unwrap();
        std::fs::write(portable.join("reaper.ini"), "").unwrap();
        let inputs = Inputs {
            env: Env { os: Os::Mac, home: root.path().join("home"), var: &none, root: Some(root.path().join("sys")) },
            kumi_home: &kumi,
            bundled: None,
            running: &no_processes,
            probe_timeout: Duration::from_secs(1),
        };
        let installs = detect::detect_reaper(&inputs.env);
        assert_eq!(installs.len(), 2, "{installs:?}");
        let source = root.path().join("reaper_kumi.dylib");
        std::fs::write(&source, binary).unwrap();
        let portable_install = installs.iter().find(|install| install.resource == portable).unwrap();
        let target = install::reaper_target(portable_install, Os::Mac).unwrap();
        Lifecycle::new(&kumi, target).install(&source, "0.1.0").unwrap();

        let checks = doctor(Daw::Reaper, &inputs);
        assert!(ids(&checks).contains(&("bridge", Level::Ok)), "{checks:?}");
        assert!(!ids(&checks).iter().any(|(id, level)| *id == "reference" && *level == Level::Fail), "{checks:?}");
        let installed = checks.iter().find(|c| c.id == "installed").unwrap();
        assert!(installed.message.replace('\\', "/").contains("home/Applications/REAPER.app"), "{installed:?}");
        let others = checks.iter().find(|c| c.id == "installs").unwrap();
        assert!(others.message.contains("Application Support"), "the default folder is the other one: {others:?}");
    }

    /// Review K20: a portable REAPER outside the folders Kumi looks in (named once with KUMI_REAPER_RESOURCE_DIR) is
    /// checked where its receipt says the extension went, not taken for the default REAPER with a missing reference.
    #[test]
    fn a_portable_reaper_kumi_doesnt_find_is_checked_where_its_receipt_says() {
        for os in [Os::Mac, Os::Windows, Os::Linux] {
            let root = tempfile::tempdir().unwrap();
            let kumi = root.path().join("kumi");
            let none = |_: &str| None;
            let default_resource = match os {
                Os::Mac => root.path().join("home/Library/Application Support/REAPER"),
                Os::Windows => root.path().join("home/AppData/Roaming/REAPER"),
                Os::Linux => root.path().join("home/.config/REAPER"),
            };
            std::fs::create_dir_all(default_resource.join("UserPlugins")).unwrap();
            std::fs::write(default_resource.join("reaper.ini"), "").unwrap();
            let portable = root.path().join("Volumes/Stick/REAPER");
            std::fs::create_dir_all(portable.join("UserPlugins")).unwrap();
            std::fs::write(portable.join("reaper.ini"), "").unwrap();
            let app = portable.join(match os {
                Os::Mac => "REAPER.app",
                Os::Windows => "reaper.exe",
                Os::Linux => "reaper",
            });
            std::fs::create_dir_all(&app).unwrap();
            let env = Env { os, home: root.path().join("home"), var: &none, root: Some(root.path().join("sys")) };
            let installs = detect::detect_reaper(&env);
            assert_eq!(installs.len(), 1, "only the default REAPER is found by itself: {installs:?}");
            let target = install::reaper_target(&detect::with_placed(&env, vec![], Some(&portable.join("UserPlugins/x"))).remove(0), os);
            let source = root.path().join("reaper_kumi");
            std::fs::write(&source, "extension").unwrap();
            Lifecycle::new(&kumi, target.unwrap()).install(&source, "0.1.0").unwrap();
            let inputs = Inputs { env, kumi_home: &kumi, bundled: None, running: &no_processes, probe_timeout: Duration::from_secs(1) };
            let checks = doctor(Daw::Reaper, &inputs);
            assert!(ids(&checks).contains(&("bridge", Level::Ok)), "{os:?}: {checks:?}");
            assert!(!ids(&checks).contains(&("reference", Level::Fail)), "{os:?}: {checks:?}");
            let installed = checks.iter().find(|c| c.id == "installed").unwrap();
            assert!(installed.message.contains(&app.display().to_string()), "{os:?}: {installed:?}");
            // Named by the variable, that folder is the only one: nothing is added beside it.
            let named = |name: &str| (name == "KUMI_REAPER_RESOURCE_DIR").then(|| default_resource.display().to_string());
            let env = Env { os, home: root.path().join("home"), var: &named, root: Some(root.path().join("sys")) };
            let receipt = portable.join("UserPlugins").join(install::reaper_extension_name(os));
            assert_eq!(detect::with_placed(&env, detect::detect_reaper(&env), Some(&receipt)).len(), 1);
        }
    }
}
