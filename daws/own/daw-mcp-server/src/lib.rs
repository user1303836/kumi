//! The REAPER and Renoise MCP host: stdio MCP for Kumi (and any MCP client), the bridge inside the DAW on loopback
//! (`kumi-daw/1`, see daw-bridge). One binary serves either DAW: `daw-mcp-server --daw reaper|renoise`.

pub mod catchup;
pub mod changes;
pub mod client;
pub mod flow;
pub mod host;
pub mod listening;
pub mod mcp;
pub mod reaper;
pub mod renoise;
pub mod semantic;
pub mod simulator;
pub mod stdio;
pub mod tools;

use daw_bridge::Daw;
use std::path::PathBuf;

/// The command line: `--daw reaper|renoise` (or KUMI_DAW), and `--kumi-home <folder>` (or KUMI_HOME).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub daw: Daw,
    pub kumi_home: PathBuf,
}

pub fn parse_args(args: &[String], env: &dyn Fn(&str) -> Option<String>, home: Option<PathBuf>) -> Result<Options, String> {
    let mut daw = env("KUMI_DAW").filter(|value| !value.is_empty());
    let mut kumi_home = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--daw" => daw = Some(rest.next().ok_or("--daw needs reaper or renoise")?.clone()),
            "--kumi-home" => kumi_home = Some(PathBuf::from(rest.next().ok_or("--kumi-home needs a folder")?)),
            "--version" => return Err(format!("daw-mcp-server {}", env!("CARGO_PKG_VERSION"))),
            other => return Err(format!("unknown argument {other}; usage: daw-mcp-server --daw reaper|renoise [--kumi-home <folder>]")),
        }
    }
    let daw = daw.ok_or("usage: daw-mcp-server --daw reaper|renoise [--kumi-home <folder>]")?;
    let daw = Daw::from_id(&daw).ok_or_else(|| format!("unknown DAW {daw}: reaper or renoise"))?;
    let kumi_home =
        kumi_home.or_else(|| daw_bridge::paths::kumi_home_from(env, home)).ok_or("no home folder: set KUMI_HOME or pass --kumi-home")?;
    Ok(Options { daw, kumi_home })
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
            Options { daw: Daw::Renoise, kumi_home: PathBuf::from("/h/.kumi") }
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
}
