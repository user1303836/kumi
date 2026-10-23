//! Doctor's checks for one DAW's bridge: installed, opened once, the bridge's files and their hash, the secret, the
//! DAW running, the bridge answering and its version, ports, quarantine, architecture, Renoise's own settings, and
//! the permission Logic's bridge needs.
use crate::daw::Daw;
use crate::detect::{self, Env, Os, LOGIC_MIN};
use crate::files::{read_secret, scan_discovery};
use crate::install::{self, LOGIC_BINARY, LOGIC_HELPER_PREFIX, RENOISE_TOOL_ID};
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
        Daw::Logic => logic_target(inputs, &mut checks),
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
    let mut installs = detect::with_placed(&inputs.env, detect::detect_reaper(&inputs.env), receipt.as_deref());
    if installs.is_empty() {
        checks.push(check(
            "installed",
            Level::Note,
            "REAPER isn't installed (or not where Kumi looks: set KUMI_REAPER_RESOURCE_DIR to its resource folder)",
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

/// Logic's bridge is Kumi's own process in Kumi's own folder, so nothing waits on Logic having run: the app and its
/// version are checked, then the bridge's folder where [`install::logic_target`] puts it.
fn logic_target(inputs: &Inputs, checks: &mut Vec<Check>) -> Option<(Target, Vec<detect::Arch>)> {
    let installs = detect::detect_logic(&inputs.env);
    let Some(install) = installs.first() else {
        checks.push(check("installed", Level::Note, "Logic Pro isn't installed (or not where Kumi looks: set KUMI_LOGIC_APP to the app)"));
        return None;
    };
    let version = install.version.map(|version| version.to_string()).unwrap_or_else(|| "(version unknown)".into());
    if install.version.is_some_and(|version| version < LOGIC_MIN) {
        checks.push(check(
            "installed",
            Level::Fail,
            format!(
                "Logic Pro {version} at {} is older than Kumi serves: Kumi supports Logic Pro 12 on Apple Silicon",
                install.app.display()
            ),
        ));
        return None;
    }
    checks.push(check("installed", Level::Ok, format!("Logic Pro {version} at {}", install.app.display())));
    if installs.len() > 1 {
        let others: Vec<String> = installs[1..].iter().map(|other| other.app.display().to_string()).collect();
        checks.push(check("installs", Level::Note, format!("other Logic Pro apps: {}", others.join(", "))));
    }
    Some((install::logic_target(&BridgePaths::new(inputs.kumi_home, Daw::Logic)), install.archs.clone()))
}

/// The first of the bridge's files macOS marked as downloaded, if any: the extension itself, the tool's `main.lua` or
/// its folder, or Logic's binary and its helpers (Gatekeeper then stops it loading or running).
fn quarantined_in(daw: Daw, target: &Target) -> Option<std::path::PathBuf> {
    let file = target.path();
    let candidates: Vec<std::path::PathBuf> = match daw {
        Daw::Logic => {
            let mut files = vec![file.join(LOGIC_BINARY)];
            if let Ok(entries) = std::fs::read_dir(&file) {
                files.extend(
                    entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(LOGIC_HELPER_PREFIX))),
                );
            }
            files.sort();
            files
        }
        _ if target.folder => vec![file.join("main.lua"), file],
        _ => vec![file],
    };
    candidates.into_iter().find(|path| detect::quarantined(path))
}

fn renoise_target(inputs: &Inputs, checks: &mut Vec<Check>) -> Option<(Target, Vec<detect::Arch>)> {
    let found = detect::detect_renoise(&inputs.env);
    if found.apps.is_empty() && found.prefs.is_empty() {
        checks.push(check(
            "installed",
            Level::Note,
            "Renoise isn't installed (or not where Kumi looks: set KUMI_RENOISE_PREFS_DIR to its preferences folder)",
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
        if let Some(marked) = quarantined_in(daw, target) {
            let message = match daw {
                Daw::Logic => {
                    format!(
                        "macOS marked {} as downloaded, so it won't run: set up Kumi's Logic Pro bridge again to clear it",
                        marked.display()
                    )
                }
                _ => format!("macOS marked {} as downloaded, so {daw} won't load it: set up the bridge again to clear it", file.display()),
            };
            checks.push(check("quarantine", Level::Fail, message));
        }
        // What must run on the app's architecture: the extension loaded into REAPER; Logic's bridge binary, a process
        // of its own that the app's helper talks to. (Renoise's tool is Lua.)
        let binary = match daw {
            Daw::Logic => Some(file.join(LOGIC_BINARY)),
            _ if target.folder => None,
            _ => Some(file.clone()),
        };
        if let Some(binary) = binary.filter(|binary| !install::arch_fits(binary, app_archs)) {
            let message = match daw {
                Daw::Logic => format!("{} is built for another architecture than Logic Pro's ({app_archs:?})", binary.display()),
                _ => format!("the extension's architecture doesn't match {daw}'s ({app_archs:?})"),
            };
            checks.push(check("arch", Level::Fail, message));
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
        // Logic loads nothing: Kumi's host starts the bridge itself, so none running is how Logic waits.
        match daw {
            Daw::Logic => checks.push(check(
                "answering",
                Level::Note,
                "Logic Pro is running without Kumi's bridge: Kumi starts its bridge for Logic Pro when a session begins",
            )),
            _ => checks.push(check("answering", Level::Warn, format!("{daw} is running without Kumi's bridge: restart {daw} to load it"))),
        }
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
                // What the bridge can't do until the producer grants it (Logic's bridge reads through Accessibility, and
                // its status names the permission it lacks).
                if let Some(permission) = found.status.as_ref().and_then(|status| status.get("permission")).and_then(|p| p.as_str()) {
                    let message = match permission {
                        "accessibility" => {
                            "Accessibility isn't granted to Kumi's helper: System Settings > Privacy & Security > Accessibility".to_string()
                        }
                        other => format!("{daw}'s bridge lacks the {other} permission"),
                    };
                    checks.push(check("permission", Level::Fail, message));
                }
                if installed_version.as_ref().is_some_and(|installed| *installed != identity.bridge) {
                    let installed = installed_version.as_deref().unwrap_or("");
                    let message = match daw {
                        Daw::Logic => format!(
                            "bridge {} still serves Logic Pro (pid {}): Kumi starts its bridge {installed} for Logic Pro when a session begins",
                            identity.bridge, identity.pid
                        ),
                        _ => format!("{daw} (pid {}) still runs bridge {}: restart {daw} to load {installed}", identity.pid, identity.bridge),
                    };
                    checks.push(check("restart", Level::Warn, message));
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
    fn logic_from_nothing_to_answering_without_accessibility() {
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
        assert_eq!(ids(&doctor(Daw::Logic, &inputs(&no_processes))), [("installed", Level::Note)]);

        // Logic Pro 11 is older than Kumi serves: nothing else is checked.
        // Joined piece by piece: the doctor prints the path as the OS writes it, backslashes on Windows included.
        let app = root.path().join("sys").join("Applications").join("Logic Pro.app");
        let arm64 = [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 0x01];
        let plist = |version: &str| {
            format!("<plist><dict><key>CFBundleIdentifier</key><string>com.apple.logic10</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>")
        };
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), plist("11.1.2")).unwrap();
        std::fs::write(app.join("Contents/MacOS/Logic Pro"), arm64).unwrap();
        let checks = doctor(Daw::Logic, &inputs(&no_processes));
        assert_eq!(ids(&checks), [("installed", Level::Fail)]);
        assert!(checks[0].message.contains("Logic Pro 11.1.2") && checks[0].message.contains("Logic Pro 12"), "{checks:?}");

        // Logic Pro 12, with no bridge yet.
        std::fs::write(app.join("Contents/Info.plist"), plist("12.3.1")).unwrap();
        let checks = doctor(Daw::Logic, &inputs(&no_processes));
        assert_eq!(checks[0], check("installed", Level::Ok, format!("Logic Pro 12.3.1 at {}", app.display())));
        assert!(ids(&checks).contains(&("bridge", Level::Fail)) && ids(&checks).contains(&("secret", Level::Fail)), "{checks:?}");
        assert!(ids(&checks).contains(&("running", Level::Note)));

        // Installed through the lifecycle: the folder with the binary, its helper and the version file.
        let target = install::logic_target(&BridgePaths::new(&kumi, Daw::Logic));
        let source = root.path().join("src").join("logic-kumi");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("logic-kumi"), arm64).unwrap();
        std::fs::write(source.join("kumi-logic-hands-arm64"), arm64).unwrap();
        std::fs::write(source.join("version"), "0.1.0\n").unwrap();
        Lifecycle::new(&kumi, target.clone()).install(&source, "0.1.0").unwrap();
        let checks = doctor(Daw::Logic, &inputs(&no_processes));
        assert!(ids(&checks).contains(&("bridge", Level::Ok)), "{checks:?}");
        assert!(!ids(&checks).iter().any(|(id, _)| matches!(*id, "secret" | "reference" | "arch" | "quarantine")), "{checks:?}");

        // Logic running without the bridge is how Logic waits: Kumi starts the bridge itself.
        let checks = doctor(Daw::Logic, &inputs(&running));
        let answering = checks.iter().find(|check| check.id == "answering").expect("said");
        assert_eq!(answering.level, Level::Note);
        assert_eq!(
            answering.message,
            "Logic Pro is running without Kumi's bridge: Kumi starts its bridge for Logic Pro when a session begins"
        );

        // Answering, but Accessibility isn't granted: the bridge's status says so, and so does the doctor.
        let secret = read_secret(&BridgePaths::new(&kumi, Daw::Logic).secret()).unwrap();
        let identity = Identity {
            daw: "logic".into(),
            bridge: "0.1.0".into(),
            daw_version: "12.3.1".into(),
            pid: std::process::id(),
            resource: app.to_string_lossy().into_owned(),
            project: None,
        };
        let bridge = crate::probe::testing::serve_with(secret, identity.clone(), serde_json::json!({"permission": "accessibility"}));
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
        write_discovery(&BridgePaths::new(&kumi, Daw::Logic).discovery(), &discovery).unwrap();
        let checks = doctor(Daw::Logic, &inputs(&running));
        assert!(ids(&checks).contains(&("answering", Level::Ok)), "{checks:?}");
        let permission = checks.iter().find(|check| check.id == "permission").expect("the permission check");
        assert_eq!(permission.level, Level::Fail);
        assert_eq!(
            permission.message,
            "Accessibility isn't granted to Kumi's helper: System Settings > Privacy & Security > Accessibility"
        );
        assert!(!ids(&checks).iter().any(|(id, _)| *id == "restart"));

        // A bridge binary built for another architecture than Logic's is said, even inside the folder.
        std::fs::write(target.path().join("logic-kumi"), [0xcf, 0xfa, 0xed, 0xfe, 0x07, 0, 0, 0x01]).unwrap();
        let checks = doctor(Daw::Logic, &inputs(&no_processes));
        let arch = checks.iter().find(|check| check.id == "arch").expect("the arch check");
        assert!(arch.level == Level::Fail && arch.message.contains("logic-kumi") && arch.message.contains("Logic Pro's"), "{arch:?}");

        // A helper macOS marked as downloaded is named, not the folder.
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::ffi::OsStrExt;
            let helper = target.path().join("kumi-logic-hands-arm64");
            let path = std::ffi::CString::new(helper.as_os_str().as_bytes()).unwrap();
            let value = b"0083;00000000;Safari;";
            let set = unsafe { libc::setxattr(path.as_ptr(), c"com.apple.quarantine".as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
            assert_eq!(set, 0);
            let checks = doctor(Daw::Logic, &inputs(&no_processes));
            let quarantine = checks.iter().find(|check| check.id == "quarantine").expect("the quarantine check");
            assert!(quarantine.message.contains("kumi-logic-hands-arm64") && quarantine.message.contains("won't run"), "{quarantine:?}");
        }
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
