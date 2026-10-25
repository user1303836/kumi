//! Live's part of `kumi doctor`: its bridge (the configuration, the server other MCP apps start, whether Kumi
//! reaches Live through it, Kumi's Live Extension), what Live needs from this computer (Live installed and opened,
//! its User Library, the bridge's ports, Developer Mode), and Kumi's hands in Live's menus.
use super::folders::{find_bridge_config, live_user_library, read_bridge_server, remote_scripts_dir, BridgeServer};
use super::{
    answers::{ExtensionHost, Hands, LiveRunning, Lives, NodeVersions, PortOwners},
    bridge_setup::is_live_running,
    live_app::{installed_lives, installed_with, runs_on, InstalledLive, MIN_LIVE_MAJOR},
    live_extension::*,
};
use crate::hands::{can_build_hands, open_hands, HandsErrorKind, OpenHandsOptions};
use futures::{future::join_all, FutureExt};
use kumi_daw::check::{access, major, newer, node_version, tilde};
use kumi_daw::{
    check::{Bundled, Check, CheckStatus, Probe, Probes},
    io::DawIo,
    process::{default_run, executable_dir},
    provider::Installed as LiveInstalled,
};
use kumi_library::sources::{dirname, join};
use kumi_types::{
    command::{CONNECT_LIVE, INSTALLED, KUMI, KUMI_REPAIR, KUMI_START},
    daw::DawId,
    errors::RuntimeError,
    system::{self, Env},
};
use serde_json::Value;
use std::{path::Path, time::Duration};

/// Who listens on one of this computer's ports: nobody (None), or a program, named when the system says.
pub type PortOwner = Option<Option<String>>;

/// Live's bridge and whether Kumi reaches Live through it; `node` is the doctor's check of Kumi itself.
pub(crate) async fn bridge_checks(io: &DawIo, node: &Check) -> Vec<Check> {
    let env = &io.env;
    let bundled = io.answers.get::<Bundled>().and_then(|bundled| (bundled.0)(DawId::Ableton));
    let mut checks = vec![];
    if let Some(config) = find_bridge_config(env) {
        let server = read_bridge_server(&config).unwrap_or_default();
        let version = server.version.as_ref().map(|s| format!(" {s}")).unwrap_or_default();
        checks.push(Check::ok(format!("Ableton bridge{version} ({})", tilde(&config))));
        if let (Some(installed), Some(bundled)) = (&server.version, &bundled) {
            if newer(bundled, installed) {
                checks.push(older_bridge(system::platform(), installed, bundled))
            }
        }
        let later = if server.native() {
            format!("Run {} bridge to record Kumi's bridge executable", *KUMI)
        } else if *INSTALLED {
            format!("Kumi isn't affected. The next {} bridge records Kumi's own Node", *KUMI)
        } else {
            format!("Kumi isn't affected. Install Node 24 LTS (nodejs.org); the next {} bridge records it", *KUMI)
        };
        if let Some(command) = &server.command {
            if !access(command, true) {
                checks.push(Check::note(
                    if server.native() {
                        format!("Other MCP apps would start the bridge with an executable that's missing ({})", tilde(command))
                    } else {
                        format!("Other MCP apps would start the bridge with a Node that's missing ({})", tilde(command))
                    },
                    Some(later),
                ))
            } else {
                if !server.native() {
                    let bridge_node = match io.answers.get::<NodeVersions>() {
                        Some(version) => (version.0)(command.clone()).await,
                        None => node_version(command).await,
                    };
                    if let Some(version) = bridge_node {
                        if ![22., 24.].contains(&major(&version)) {
                            checks.push(Check::note(
                                format!(
                                    "Other MCP apps would start the bridge with Node.js {}, which it doesn't support",
                                    version.strip_prefix('v').unwrap_or(&version)
                                ),
                                Some(later.clone()),
                            ))
                        }
                    }
                }
                if regex::Regex::new(r"(?i)[\\/](_npx|\.npm[\\/]_npx|tmp|Temp)[\\/]").unwrap().is_match(command) {
                    checks.push(Check::note(
                        if server.native() {
                            "Other MCP apps would start the bridge with an executable from a temporary folder, which can disappear"
                        } else {
                            "Other MCP apps would start the bridge with a Node from a temporary folder, which can disappear"
                        },
                        Some(later),
                    ))
                }
            }
        } else {
            checks.push(Check::note("The bridge configuration names no Node for other MCP apps", Some(later)))
        }
        let live = match io.answers.get::<Probes>() {
            Some(probe) => (probe.0)(DawId::Ableton, config.clone()).await.unwrap_or_default(),
            None => Probe::default(),
        };
        if !live.started && live.said.as_deref().is_some_and(|said| said.contains(kumi_common::bridge::ANOTHER_BRIDGE)) {
            checks.push(Check::fix(
                "Live is running another version of Kumi's bridge",
                Some(format!(
                    "Restart Live: it loads its bridge when it starts. If it still says this, quit Live and run {} bridge. Then: {} doctor",
                    *KUMI, *KUMI
                )),
            ))
        } else if !live.started {
            let current = server.version.as_ref().zip(bundled.as_ref()).is_some_and(|(installed, bundled)| !newer(bundled, installed));
            checks.push(if node.status==CheckStatus::Fix{Check::note("The bridge didn't start; it needs Node 22 or 24 too",None)}else if current{Check::fix("Kumi's bridge couldn't reach Live",Some(format!("Open Live and choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI); if Live is showing a dialog, answer it first. Then: {} doctor",*KUMI)))}else{Check::fix("The bridge didn't start",Some(format!("{}, and bring Live's part up to date: quit Live, then run {} bridge. Then: {} doctor",if *INSTALLED{format!("Run {} to repair Kumi",*KUMI_REPAIR)}else{format!("Build it ({})",*KUMI_REPAIR)},*KUMI,*KUMI)))})
        } else if live.connected != Some(true) {
            checks.push(Check::fix(
                "Live isn't connected",
                Some("Open Live and choose AbletonMcpBridge as a Control Surface (Settings → Link, Tempo & MIDI)".into()),
            ))
        } else {
            let location = format!(
                "{} connected{}",
                live.live_version.as_ref().filter(|s| !s.is_empty()).map(|s| format!("Live {s}")).unwrap_or("Live".into()),
                live.set.as_ref().filter(|s| !s.is_empty()).map(|s| format!(" · {s}")).unwrap_or_default()
            );
            checks.push(if live.real_live == Some(false) {
                Check::note(format!("{location} (a simulator, not real Live)"), None)
            } else {
                Check::ok(location)
            })
        }
        if live.real_live != Some(false) {
            if let Some(extension) = extension_check(env, &config, &server, &live).await {
                checks.push(extension)
            }
        }
    } else {
        let platform = system::platform();
        checks.push(missing_bridge(platform, || super::live_app::installed(env, platform)))
    }
    checks
}

/// No bridge in Live: something to fix where a Live 12 can take it. No Live to put one in (Linux, where Live's folders
/// aren't read at all, or no Live 12 here) is a note.
pub fn missing_bridge(platform: &str, installed: impl FnOnce() -> LiveInstalled) -> Check {
    let live_12 = || Some(format!("Kumi works with Live {MIN_LIVE_MAJOR}: once it's installed and opened once, run: {}", *KUMI_START));
    match super::live_app::runs_on(platform).then(installed) {
        None => Check::note("There's no Live for this system, so Kumi chats without it", None),
        Some(LiveInstalled::Missing) => {
            Check::note(format!("Live {MIN_LIVE_MAJOR} isn't on this computer, so Kumi chats without Live"), live_12())
        }
        Some(LiveInstalled::Older(version)) => {
            Check::note(format!("Only Live {version} is on this computer, so Kumi chats without Live"), live_12())
        }
        Some(_) => Check::fix("The Ableton bridge isn't installed, so Kumi can't see Live", Some(CONNECT_LIVE.to_string())),
    }
}
/// A bridge older than this Kumi's: to bring up to date where Live runs, the one way. Where it doesn't (Linux) there's
/// no Live to load it, so it's a note, with what this system can do: bring the bridge's files up to date.
pub fn older_bridge(platform: &str, installed: &str, bundled: &str) -> Check {
    let text = format!("The installed bridge ({installed}) is older than this Kumi's ({bundled})");
    if super::live_app::runs_on(platform) {
        Check::fix(text, Some(CONNECT_LIVE.to_string()))
    } else {
        Check::note(text, Some(format!("{} bridge --yes brings its files up to date", *KUMI)))
    }
}
async fn extension_check(env: &Env, config: &str, server: &BridgeServer, live: &Probe) -> Option<Check> {
    let folder = live_extensions_dir(env, system::platform())?;
    let version = live.live_version.as_deref().unwrap_or("");
    let parts: Vec<_> = version
        .split('.')
        .map(|p| p.bytes().take_while(u8::is_ascii_digit).collect::<Vec<_>>())
        .map(|p| String::from_utf8_lossy(&p).parse::<f64>().unwrap_or(0.))
        .collect();
    let m = parts.first().copied().unwrap_or(0.);
    let minor = parts.get(1).copied().unwrap_or(0.);
    if !version.is_empty() && (m < 12. || (m == 12. && minor < 4.)) {
        return Some(Check::note(
            format!("Live {version} runs no extensions (12.4 and later do), so Kumi can't render tracks without playing them"),
            None,
        ));
    }
    let carried = server.package_root().and_then(|p| extension_source(&p)).and_then(|p| read_extension(&p));
    let installed = installed_extension(&folder);
    let again = Some(format!("Run: {} bridge, then restart Live", *KUMI));
    let Some(installed) = installed else {
        return carried.map(|_| {
            Check::fix(
                "Kumi's extension isn't in Live (it renders tracks without playing them and writes MIDI clips in the Arrangement)",
                again,
            )
        });
    };
    if carried.as_ref().is_some_and(|c| c.digest != installed.digest) {
        return Some(Check::fix("Kumi's extension in Live is from another bridge", again));
    }
    let in_live = extension_data_dir(&folder);
    let running = running_extension(&in_live).or_else(|| running_extension(&join(&dirname(config), "live-extension")));
    if let Some(running) = running {
        return Some(if !extension_answers(running.port, 2000).await {
            Check::fix("Kumi's extension is running but doesn't answer", Some("Restart Live".into()))
        } else {
            Check::ok(if running.folder == in_live {
                "Kumi's extension is running in Live"
            } else {
                "Kumi's extension is running (Kumi started it: Live's Developer Mode is on)"
            })
        });
    }
    Some(if live.connected == Some(true) {
        Check::note("Live hasn't started Kumi's extension",Some("Restart Live: it starts extensions when it opens. With Developer Mode on (Settings → Extensions), Kumi starts it itself while Kumi runs".into()))
    } else {
        Check::ok(format!("Kumi's extension {} is in Live; it starts with Live", installed.version))
    })
}
/// Kumi's hands in Live's menus: a test's answer, or this computer's.
pub(crate) async fn extra_checks(io: &DawIo) -> Result<Vec<Check>, RuntimeError> {
    let hands = if let Some(hands) = io.answers.get::<Hands>() { (hands.0)().await? } else { hands_check().await };
    Ok(hands.into_iter().collect())
}
async fn hands_check() -> Option<Check> {
    if system::platform() == "win32" {
        // Read Live's menu bar as live_command does: it said "ok" while Live's menus were out of reach (#192).
        let hands = open_hands(OpenHandsOptions { timeout_ms: Some(15_000), ..Default::default() }).await.ok().flatten()?;
        let menus = hands.menus(None).await;
        hands.close();
        return Some(match menus {
            Ok(items)
                if ["File", "Edit", "Create"].iter().all(|top| items.iter().any(|item| item.path.first().is_some_and(|t| t == top))) =>
            {
                Check::ok("Uses Live's own menus for what Live's scripting can't do (grouping, freezing, bouncing, saving)")
            }
            Err(error) if error.kind == HandsErrorKind::NoLive => {
                Check::ok("Uses Live's own menus for what Live's scripting can't do (checked once Live is open)")
            }
            Ok(_) => Check::fix(
                "Kumi can't read Live's menus (grouping, freezing, bouncing, saving won't work)",
                Some("Run kumi report and send it: Live's menu bar didn't have File, Edit and Create".into()),
            ),
            Err(error) => Check::fix(format!("Kumi can't use Live's menus: {error}"), Some("Run kumi report and send it".into())),
        });
    }
    if system::platform() != "darwin" {
        return None;
    }
    let hands = open_hands(OpenHandsOptions { build: Some(false), timeout_ms: Some(3000), ..Default::default() }).await.ok().flatten();
    let Some(hands) = hands else {
        return Some(if can_build_hands() {
            Check::ok("Uses Live's own menus (Kumi builds its helper the first time it needs it)")
        } else {
            Check::note(
                "Kumi can't use Live's own menus here yet (grouping, freezing, bouncing, saving)",
                Some("Install Xcode's command line tools (xcode-select --install), or update Kumi".into()),
            )
        });
    };
    let result = hands.trusted(false).await.ok().map(|trusted| {
        if trusted {
            Check::ok("Uses Live's own menus (Accessibility is on for this terminal)")
        } else {
            Check::fix(
                "Kumi can't use Live's own menus until Accessibility is on for this terminal",
                Some("System Settings › Privacy & Security › Accessibility: turn on the app Kumi runs in".into()),
            )
        }
    });
    hands.close();
    result
}

/// What Live needs from this computer, for the Installation section: Live installed and opened, its User Library,
/// the bridge's ports, Developer Mode.
pub(crate) async fn installation_checks(env: &Env, io: &DawIo, platform: &str) -> Vec<Check> {
    let mut checks = vec![];
    // Where Live doesn't run (Linux), nothing below applies; the bridge's check says Kumi chats without Live.
    if !runs_on(platform) {
        return checks;
    }
    let lives = match io.answers.get::<Lives>() {
        Some(lives) => (lives.0)(),
        None => installed_lives(env, platform),
    };
    let ports = bridge_ports(env);
    let (held, live_running) = futures::join!(
        join_all(ports.iter().map(|port| {
            let port = *port;
            let platform = platform.to_string();
            match io.answers.get::<PortOwners>() {
                Some(owner) => (owner.0)(port),
                None => async move { port_owner(port, &platform).await }.boxed_local(),
            }
            .map(move |owner| (port, owner))
        })),
        async {
            match io.answers.get::<LiveRunning>() {
                Some(running) => (running.0)().await,
                None => is_live_running(default_run()).await,
            }
        }
    );
    checks.extend(live_checks(env, platform, &lives, live_running));
    checks.push(ports_check(&held, live_running));
    checks.extend(developer_mode(env, platform, io, live_running).await);
    checks.extend(bundled_bridge_check(io).await);
    checks
}

/// Kumi's own copy of the bridge, the program a Live session starts (in the app's bridge/package since 1.24, beside
/// kumi before): that it is where Kumi looks, and that it starts. For an installed app only: a checkout's build has
/// its bridge beside it, or not built yet.
async fn bundled_bridge_check(io: &DawIo) -> Option<Check> {
    if !Path::new(&join(&executable_dir(), "package.json")).exists() {
        return None;
    }
    let Some(binary) = super::bridge_setup::bundled_bridge_binary() else {
        return Some(Check::fix("Kumi's copy of the bridge is missing", Some(format!("Run {}.", *KUMI_REPAIR))));
    };
    let ran = (io.run)(binary.clone(), vec!["--version".into()], None).await;
    Some(if ran.code == 0 {
        let version = ran.stdout.trim().trim_start_matches("ableton-mcp-server").trim();
        Check::ok(format!("Kumi's bridge {version} starts ({})", tilde(&binary)))
    } else {
        Check::fix(format!("Kumi's copy of the bridge didn't start ({})", tilde(&binary)), Some(format!("Run {}.", *KUMI_REPAIR)))
    })
}

/// The Lives installed here and, once one Kumi works with can take the bridge (setup's own verdict,
/// `live_app::installed`), its User Library. No such Live (none, an older one, one never opened) is said with its
/// next step by the bridge's check below, so here it's information, never a fix: Kumi chats without Live, and a
/// computer without it (a build machine, say) isn't broken.
fn live_checks(env: &Env, platform: &str, lives: &[InstalledLive], running: bool) -> Vec<Check> {
    if !runs_on(platform) {
        return vec![];
    }
    let ready = installed_with(env, platform, lives) == LiveInstalled::Ready || running;
    let mut checks = vec![if !lives.is_empty() {
        let named: Vec<_> =
            lives.iter().map(|live| format!("{} ({})", live.version.as_deref().unwrap_or("version unknown"), live.name)).collect();
        Check::ok(format!("Live installed: {}", named.join(", ")))
    } else if ready {
        Check::ok("Live has been opened on this account (its app isn't in the usual folder)")
    } else {
        Check::note("Live isn't installed here", None)
    }];
    if !ready {
        return checks;
    }
    if let Some(folder) = env.get("KUMI_REMOTE_SCRIPTS_DIR").filter(|s| !s.is_empty()) {
        checks.push(if Path::new(folder).is_dir() {
            Check::ok(format!("Remote Scripts folder: {} (KUMI_REMOTE_SCRIPTS_DIR)", tilde(folder)))
        } else {
            Check::fix(
                format!("KUMI_REMOTE_SCRIPTS_DIR names {}, which isn't there", tilde(folder)),
                Some("Make that folder, or unset KUMI_REMOTE_SCRIPTS_DIR".into()),
            )
        });
        return checks;
    }
    let library = live_user_library(env);
    let found = library.clone().unwrap_or_else(|| dirname(&remote_scripts_dir(env)));
    if !Path::new(&found).is_dir() {
        checks.push(match library {
            Some(library) => Check::fix(
                format!("Live's User Library is set to {}, which isn't there", tilde(&library)),
                Some("Connect the drive it's on, or choose it again in Live's Settings → Library".into()),
            ),
            None => Check::note(
                "Kumi can't find Live's User Library",
                Some("Live's Settings → Library shows where it is: set KUMI_REMOTE_SCRIPTS_DIR to the Remote Scripts folder in it".into()),
            ),
        });
        return checks;
    }
    checks.push(Check::ok(format!("User Library: {}", tilde(&found))));
    if platform == "win32" && found.to_ascii_lowercase().contains("onedrive") {
        checks.push(Check::note(
            "Live's User Library is in OneDrive, which can keep files online only",
            Some("In File Explorer, right-click Documents\\Ableton and choose Always keep on this device".into()),
        ));
    }
    checks
}

/// The bridge's ports: its configuration's, or the ones it installs with.
fn bridge_ports(env: &Env) -> Vec<u16> {
    let config = find_bridge_config(env).and_then(|file| serde_json::from_slice::<Value>(&std::fs::read(file).ok()?).ok());
    let port = |key: &str, default: u16| {
        config.as_ref().and_then(|c| c["bridge"][key].as_u64()).and_then(|p| u16::try_from(p).ok()).unwrap_or(default)
    };
    let mut ports = vec![port("port", 9765), port("realtimePort", 9766)];
    ports.dedup();
    ports
}

/// Whether something listens on `port` of this computer, by connecting to it (never by listening), and if so,
/// which program, when the system says.
pub async fn port_owner(port: u16, platform: &str) -> PortOwner {
    let connected = tokio::time::timeout(Duration::from_millis(400), tokio::net::TcpStream::connect(("127.0.0.1", port))).await;
    if !connected.is_ok_and(|c| c.is_ok()) {
        return None;
    }
    let run = default_run();
    // A lookup that hangs (a stuck lsof, a slow PowerShell) leaves the owner unnamed rather than the doctor waiting.
    let run = move |program: String, args: Vec<String>| {
        let run = run.clone();
        async move { tokio::time::timeout(Duration::from_secs(2), run(program, args, None)).await.unwrap_or_default() }
    };
    let named = match platform {
        "darwin" | "linux" => {
            let lsof = ["/usr/sbin/lsof", "/usr/bin/lsof"].into_iter().find(|p| Path::new(p).exists()).unwrap_or("lsof");
            let args = ["+c", "0", "-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fc"];
            let ran = run(lsof.into(), args.map(str::to_string).into()).await;
            ran.stdout.lines().find_map(|line| line.strip_prefix('c')).map(str::to_string)
        }
        "win32" => {
            let script = format!(
                "Get-NetTCPConnection -State Listen -LocalPort {port} -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty OwningProcess | ForEach-Object {{ (Get-Process -Id $_ -ErrorAction SilentlyContinue).ProcessName }}"
            );
            let powershell = system::system_program_default(system::SystemProgram::Powershell);
            let ran = run(powershell, vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), script]).await;
            Some(ran.stdout.trim().to_string()).filter(|s| !s.is_empty())
        }
        _ => None,
    };
    Some(named)
}

fn is_live(program: &str) -> bool {
    program == "Live" || program.starts_with("Ableton Live")
}

/// The bridge's ports: free, or held by Live's bridge while Live is open, are both fine; another program
/// holding one keeps the bridge from starting.
pub(crate) fn ports_check(held: &[(u16, PortOwner)], live_running: bool) -> Check {
    let ports = |list: &[u16]| match list {
        [one] => format!("port {one}"),
        many => format!("ports {}", many.iter().map(u16::to_string).collect::<Vec<_>>().join(" and ")),
    };
    let taken: Vec<_> = held.iter().filter_map(|(port, owner)| owner.as_ref().map(|name| (*port, name.clone()))).collect();
    if taken.is_empty() {
        let all: Vec<_> = held.iter().map(|(port, _)| *port).collect();
        return Check::ok(format!("{} free for Live's bridge", capitalized(&ports(&all))));
    }
    let other = taken.iter().find(|(_, name)| name.as_deref().is_some_and(|name| !is_live(name)) || (name.is_none() && !live_running));
    match other {
        Some((port, Some(name))) => {
            Check::fix(format!("{name} holds port {port}, which Live's bridge needs"), Some(format!("Quit {name}, then restart Live")))
        }
        Some((port, None)) => Check::fix(
            format!("Another program holds port {port}, which Live's bridge needs"),
            Some(format!(
                "Quit it, then restart Live ({} names it)",
                if cfg!(windows) { format!("netstat -ano | findstr :{port}") } else { format!("lsof -nP -iTCP:{port} -sTCP:LISTEN") }
            )),
        ),
        None => Check::ok(format!("Live's bridge has {} (Live is open)", ports(&taken.iter().map(|(port, _)| *port).collect::<Vec<_>>()))),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Live's Developer Mode, when it shows: who started Kumi's extension, or, with Live open, whether Live runs
/// its own Extension Host (it doesn't with Developer Mode on). None while it can't be told.
async fn developer_mode(env: &Env, platform: &str, io: &DawIo, live_running: bool) -> Option<Check> {
    let extensions = live_extensions_dir(env, platform)?;
    let off = || Check::ok("Live's Developer Mode is off: Live starts Kumi's extension when it opens");
    let on = || Check::ok("Live's Developer Mode is on: Kumi's bridge starts Kumi's extension while Kumi runs");
    if running_extension(&extension_data_dir(&extensions)).is_some() {
        return Some(off());
    }
    let config = find_bridge_config(env);
    if config.as_ref().is_some_and(|config| running_extension(&join(&dirname(config), "live-extension")).is_some()) {
        return Some(on());
    }
    if !live_running || installed_extension(&extensions).is_none() {
        return None;
    }
    let own_host = match io.answers.get::<ExtensionHost>() {
        Some(host) => (host.0)().await,
        None => ableton_mcp_server::bridge::extension_launcher::running_extension_hosts().await.live,
    };
    Some(if own_host { off() } else { on() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_held_by_live_are_fine_and_by_another_program_need_fixing() {
        assert_eq!(ports_check(&[(9765, None), (9766, None)], false).text, "Ports 9765 and 9766 free for Live's bridge");
        assert_eq!(ports_check(&[(9765, None), (9766, None)], true).status, kumi_daw::check::CheckStatus::Ok);
        let live = ports_check(&[(9765, Some(Some("Live".into()))), (9766, None)], true);
        assert_eq!(live, Check::ok("Live's bridge has port 9765 (Live is open)"));
        let windows =
            ports_check(&[(9765, Some(Some("Ableton Live 12 Suite".into()))), (9766, Some(Some("Ableton Live 12 Suite".into())))], true);
        assert_eq!(windows.text, "Live's bridge has ports 9765 and 9766 (Live is open)");
        // Unnamed while Live runs: Live's. Unnamed with Live closed: another program's.
        assert_eq!(ports_check(&[(9765, Some(None))], true).status, kumi_daw::check::CheckStatus::Ok);
        let unnamed = ports_check(&[(9765, Some(None))], false);
        assert_eq!(unnamed.status, kumi_daw::check::CheckStatus::Fix);
        assert!(unnamed.next.unwrap().starts_with("Quit it, then restart Live"));
        let other = ports_check(&[(9765, Some(Some("Live".into()))), (9766, Some(Some("node".into())))], true);
        assert_eq!(other, Check::fix("node holds port 9766, which Live's bridge needs", Some("Quit node, then restart Live".into())));
    }
}
