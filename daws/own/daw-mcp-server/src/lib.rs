//! The MCP host for Kumi's own bridges: stdio MCP for Kumi (and any MCP client), the bridge on loopback (`kumi-daw/1`,
//! see daw-bridge), inside the DAW (REAPER's extension, Renoise's tool) or beside it (SoundCraft's `soundcraft-kumi`,
//! which the host starts, or Logic's `logic-kumi`). One binary serves any of them:
//! `daw-mcp-server --daw reaper|renoise|logic|soundcraft`.

pub mod catchup;
pub mod changes;
pub mod client;
pub mod flow;
pub mod host;
pub mod listening;
pub mod logic;
pub mod mcp;
mod preset_files;
pub mod reaper;
pub mod renoise;
pub mod semantic;
pub mod simulator;
pub mod spawn;
pub mod stdio;
pub mod tools;

use daw_bridge::Daw;
use std::path::PathBuf;

/// The command line: `--daw reaper|renoise|logic|soundcraft` (or KUMI_DAW), `--kumi-home <folder>` (or KUMI_HOME), and
/// `--restricted-listening` to disable shared listening-cache and Renoise work-folder sweeps before startup.
/// `KUMI_KEEP_IDENTITY=1` in the environment, which standalone Kumi sets, asks the bridge to keep the project's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub daw: Daw,
    pub kumi_home: PathBuf,
    pub listening: host::ListeningMode,
    /// See [`host::Host::with_keep_identity`].
    pub keep_identity: bool,
}

pub fn parse_args(args: &[String], env: &dyn Fn(&str) -> Option<String>, home: Option<PathBuf>) -> Result<Options, String> {
    let ids = Daw::ALL.map(Daw::id);
    let usage = format!("usage: daw-mcp-server --daw {} [--kumi-home <folder>] [--restricted-listening]", ids.join("|"));
    let mut daw = env("KUMI_DAW").filter(|value| !value.is_empty());
    let mut kumi_home = None;
    let mut listening = host::ListeningMode::Standalone;
    let keep_identity = env(daw_bridge::wire::KEEP_IDENTITY_VARIABLE).as_deref() == Some("1");
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--daw" => daw = Some(rest.next().ok_or_else(|| format!("--daw needs one of {}", ids.join(", ")))?.clone()),
            "--kumi-home" => kumi_home = Some(PathBuf::from(rest.next().ok_or("--kumi-home needs a folder")?)),
            "--restricted-listening" => listening = host::ListeningMode::Restricted,
            "--version" => return Err(format!("daw-mcp-server {}", env!("CARGO_PKG_VERSION"))),
            other => return Err(format!("unknown argument {other}; {usage}")),
        }
    }
    let daw = daw.ok_or(usage)?;
    let daw = Daw::from_id(&daw).ok_or_else(|| format!("unknown DAW {daw}: one of {}", ids.join(", ")))?;
    let kumi_home =
        kumi_home.or_else(|| daw_bridge::paths::kumi_home_from(env, home)).ok_or("no home folder: set KUMI_HOME or pass --kumi-home")?;
    Ok(Options { daw, kumi_home, listening, keep_identity })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn the_daw_comes_from_the_flag_or_the_environment() {
        let home = Some(PathBuf::from("/h"));
        let none = |_: &str| None;
        assert_eq!(
            parse_args(&strings(&["--daw", "renoise"]), &none, home.clone()).unwrap(),
            Options {
                daw: Daw::Renoise,
                kumi_home: PathBuf::from("/h/.kumi"),
                listening: host::ListeningMode::Standalone,
                keep_identity: false,
            }
        );
        let env = |name: &str| (name == "KUMI_DAW").then(|| "reaper".to_string());
        assert_eq!(parse_args(&[], &env, home.clone()).unwrap().daw, Daw::Reaper);
        assert_eq!(
            parse_args(&strings(&["--daw", "renoise", "--kumi-home", "/k"]), &env, home.clone()).unwrap().kumi_home,
            PathBuf::from("/k")
        );
        assert!(parse_args(&[], &none, home.clone()).is_err());
        assert!(parse_args(&strings(&["--daw", "live"]), &none, home.clone()).is_err());
        assert!(parse_args(&strings(&["--daw"]), &none, home).is_err());
    }

    #[test]
    fn restricted_listening_is_an_explicit_valueless_startup_flag() {
        let home = Some(PathBuf::from("/h"));
        let none = |_: &str| None;
        for daw in Daw::ALL {
            let options = parse_args(&strings(&["--daw", daw.id(), "--restricted-listening"]), &none, home.clone()).unwrap();
            assert_eq!(options.listening, host::ListeningMode::Restricted);
            let options = parse_args(&strings(&["--daw", daw.id()]), &none, home.clone()).unwrap();
            assert_eq!(options.listening, host::ListeningMode::Standalone);
        }
        for args in [
            vec!["--daw", "reaper", "--restricted-listening=true"],
            vec!["--daw", "reaper", "--restricted-listening=false"],
            vec!["--daw", "reaper", "--restricted-listening", "true"],
            vec!["--daw", "reaper", "--restricted-listening", "false"],
            vec!["--daw", "reaper", "--restricted-listening", "--unexpected"],
        ] {
            assert!(parse_args(&strings(&args), &none, home.clone()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn the_ask_to_keep_the_projects_id_comes_from_the_environment() {
        let home = Some(PathBuf::from("/h"));
        let daw = strings(&["--daw", "reaper"]);
        let with = |value: &'static str| move |name: &str| (name == "KUMI_KEEP_IDENTITY").then(|| value.to_string());
        assert!(parse_args(&daw, &with("1"), home.clone()).unwrap().keep_identity);
        assert!(!parse_args(&daw, &with("0"), home.clone()).unwrap().keep_identity);
        assert!(!parse_args(&daw, &|_: &str| None, home).unwrap().keep_identity);
    }
}
