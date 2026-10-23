//! The REAPER, Renoise and Logic Pro MCP host: stdio MCP for Kumi (and any MCP client), the bridge on loopback
//! (`kumi-daw/1`, see daw-bridge): REAPER's and Renoise's inside the DAW, Logic's a process of Kumi's own that the host
//! starts. One binary serves any of them: `daw-mcp-server --daw reaper|renoise|logic`.

pub mod catchup;
pub mod changes;
pub mod client;
pub mod flow;
pub mod host;
pub mod instruments;
pub mod library_files;
pub mod listen;
pub mod listening;
pub mod mcp;
pub mod presets;
pub mod renoise_devices;
pub mod renoise_texts;
pub mod semantic;
pub mod simulator;
pub mod stdio;
pub mod tools;
pub mod xml;
pub mod zip;

use daw_bridge::Daw;
use std::path::PathBuf;

/// The command line: `--daw reaper|renoise|logic` (or KUMI_DAW), and `--kumi-home <folder>` (or KUMI_HOME). For a
/// bridge the host starts (Logic's), KUMI_LOGIC_BRIDGE names the binary to start instead of the installed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub daw: Daw,
    pub kumi_home: PathBuf,
    pub logic_bridge: Option<PathBuf>,
}

pub fn parse_args(args: &[String], env: &dyn Fn(&str) -> Option<String>, home: Option<PathBuf>) -> Result<Options, String> {
    let mut daw = env("KUMI_DAW").filter(|value| !value.is_empty());
    let mut kumi_home = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--daw" => daw = Some(rest.next().ok_or("--daw needs reaper, renoise or logic")?.clone()),
            "--kumi-home" => kumi_home = Some(PathBuf::from(rest.next().ok_or("--kumi-home needs a folder")?)),
            "--version" => return Err(format!("daw-mcp-server {}", env!("CARGO_PKG_VERSION"))),
            other => {
                return Err(format!("unknown argument {other}; usage: daw-mcp-server --daw reaper|renoise|logic [--kumi-home <folder>]"))
            }
        }
    }
    let daw = daw.ok_or("usage: daw-mcp-server --daw reaper|renoise|logic [--kumi-home <folder>]")?;
    let daw = Daw::from_id(&daw).ok_or_else(|| format!("unknown DAW {daw}: reaper, renoise or logic"))?;
    let kumi_home =
        kumi_home.or_else(|| daw_bridge::paths::kumi_home_from(env, home)).ok_or("no home folder: set KUMI_HOME or pass --kumi-home")?;
    let logic_bridge = env(host::LOGIC_BRIDGE_VAR).filter(|value| !value.is_empty()).map(PathBuf::from);
    Ok(Options { daw, kumi_home, logic_bridge })
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
            Options { daw: Daw::Renoise, kumi_home: PathBuf::from("/h/.kumi"), logic_bridge: None }
        );
        let env = |name: &str| (name == "KUMI_DAW").then(|| "reaper".to_string());
        assert_eq!(parse_args(&[], &env, home.clone()).unwrap().daw, Daw::Reaper);
        // Logic, and the bridge binary to start in place of the installed one.
        let env = |name: &str| (name == "KUMI_LOGIC_BRIDGE").then(|| "/build/logic-kumi".to_string());
        assert_eq!(
            parse_args(&strings(&["--daw", "logic"]), &env, home.clone()).unwrap(),
            Options { daw: Daw::Logic, kumi_home: PathBuf::from("/h/.kumi"), logic_bridge: Some(PathBuf::from("/build/logic-kumi")) }
        );
        assert!(parse_args(&strings(&["--daw", "logic"]), &none, home.clone()).unwrap().logic_bridge.is_none());
        assert!(parse_args(&strings(&["--what"]), &none, home.clone()).unwrap_err().contains("reaper|renoise|logic"));
        assert_eq!(
            parse_args(&strings(&["--daw", "renoise", "--kumi-home", "/k"]), &env, home.clone()).unwrap().kumi_home,
            PathBuf::from("/k")
        );
        assert!(parse_args(&[], &none, home.clone()).is_err());
        assert!(parse_args(&strings(&["--daw", "live"]), &none, home.clone()).is_err());
        assert!(parse_args(&strings(&["--daw"]), &none, home).is_err());
    }
}
