use crate::{
    command::{number, resolve, validate, value, CommandOutput},
    delivery::*,
    live::LiveError,
};
use serde_json::json;
use std::path::Path;
const VALUES: &[&str] = &["--output", "--bridge-host", "--bridge-port", "--secret-file", "--bridge-timeout", "--realtime-port"];
pub fn run(args: &[String]) -> CommandOutput {
    run_with_package_root(args, &default_package_root())
}
pub fn run_with_package_root(args: &[String], package_root: &Path) -> CommandOutput {
    let out = validate(args, "setup", VALUES, &["--force"]);
    if out.code != 0 {
        return out;
    }
    let Some(output) = value(args, "--output").filter(|v| !v.is_empty()) else {
        return CommandOutput::error("usage: ableton-mcp-setup --output <path> [--bridge-host <loopback> --bridge-port <port> --secret-file <path> --bridge-timeout <ms> --realtime-port <port>] [--force]", 2);
    };
    let result = (|| -> Result<CommandOutput, LiveError> {
        let path = resolve(output)?;
        let entrypoint = native_entrypoint(package_root);
        let config = if VALUES[1..].iter().any(|option| value(args, option).is_some()) {
            let mut bridge = json!({"host":value(args,"--bridge-host").unwrap_or("127.0.0.1"),"port":number(value(args,"--bridge-port")),"secretFile":resolve(value(args,"--secret-file").unwrap_or(""))?,"timeoutMs":number(Some(value(args,"--bridge-timeout").unwrap_or("5000")))});
            if let Some(port) = value(args, "--realtime-port") {
                bridge["realtimePort"] = json!(number(Some(port)));
            }
            let config = config_for_bridge(&entrypoint, &bridge, None, Some(&path), true)?;
            read_secret_file(&config.bridge.secret_file)?;
            AnyConfig::Bridge(config)
        } else {
            AnyConfig::Server(config_for_entrypoint(&entrypoint, None)?)
        };
        write_config(&path, &config, args.iter().any(|a| a == "--force"))?;
        let version = match config {
            AnyConfig::Bridge(c) => c.version,
            AnyConfig::Server(c) => c.version,
        };
        Ok(CommandOutput::json(&json!({"created":path,"version":version})))
    })();
    result.unwrap_or_else(|error| CommandOutput::error(error.message(), 1))
}
pub fn main() -> i32 {
    run(&std::env::args().skip(1).collect::<Vec<_>>()).emit()
}
