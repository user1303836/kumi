use crate::{
    command::{resolve, validate, value, CommandOutput},
    delivery::*,
    live::LiveError,
};
use std::path::Path;
pub fn run(args: &[String]) -> CommandOutput {
    run_with_package_root(args, &default_package_root())
}
pub fn run_with_package_root(args: &[String], package_root: &Path) -> CommandOutput {
    let out = validate(args, "installer", &["--destination", "--config"], &["--dry-run", "--force"]);
    if out.code != 0 {
        return out;
    }
    let Some(destination) = value(args, "--destination").filter(|v| !v.is_empty()) else {
        return CommandOutput::error("usage: ableton-mcp-install-remote-script --destination <explicit-directory> [--config <absolute-host-config>] [--dry-run] [--force]",2);
    };
    let result = (|| -> Result<CommandOutput, LiveError> {
        let options = InstallOptions {
            dry_run: args.iter().any(|a| a == "--dry-run"),
            force: args.iter().any(|a| a == "--force"),
            config_path: value(args, "--config").map(resolve).transpose()?,
            producer_files_from: None,
        };
        let source = package_root.join("remote-script").join(REMOTE_SCRIPT_PACKAGE).join(REMOTE_SCRIPT_ASSET);
        Ok(CommandOutput::json(&install_remote_script(&source, &resolve(destination)?, &options)?))
    })();
    result.unwrap_or_else(|error| CommandOutput::error(error.message(), 1))
}
pub fn main() -> i32 {
    run(&std::env::args().skip(1).collect::<Vec<_>>()).emit()
}
