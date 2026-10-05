use crate::{command::CommandOutput, delivery::diagnostics_async};
use std::path::Path;
pub async fn run(args: &[String]) -> CommandOutput {
    run_with_package_root(args, None).await
}
pub async fn run_with_package_root(args: &[String], package_root: Option<&Path>) -> CommandOutput {
    if !(args.is_empty() || (args.len() == 2 && args[0] == "--config" && !args[1].starts_with('-'))) {
        return CommandOutput::error("diagnostics: expected at most one --config PATH", 2);
    }
    let config = args.get(1);
    if config.is_some_and(String::is_empty) {
        return CommandOutput::error("diagnostics: --config requires a path", 2);
    }
    let report = diagnostics_async(package_root, config.map(Path::new)).await;
    CommandOutput {
        stdout: format!("{}\n", serde_json::to_string_pretty(&report).expect("diagnostic JSON")),
        stderr: String::new(),
        code: if report["runtimeSupported"] == true && report["platformSupported"] == true { 0 } else { 1 },
    }
}
pub fn main() -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(_) => return 1,
    };
    tokio::task::LocalSet::new().block_on(&runtime, run(&std::env::args().skip(1).collect::<Vec<_>>())).emit()
}
