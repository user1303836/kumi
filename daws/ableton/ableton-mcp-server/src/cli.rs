//! Native entry point for the MCP host and its delivery commands.
use crate::command::CommandOutput;
use crate::{
    bridge::{
        extension_setup::{with_extension, ExtensionSetup},
        live_extension_folders::kumi_extension_folders,
        remote_adapter::{RemoteScriptEndpoint, RemoteScriptLiveAdapter},
    },
    delivery::{read_any_config, read_secret_file},
    live::{AsyncLiveAdapter, LiveError},
};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

/// Delivery and metadata commands complete without opening the MCP transport.
pub async fn auxiliary_command(args: &[String]) -> Option<CommandOutput> {
    if args.len() == 1 && args[0] == "--version" {
        return Some(CommandOutput { stdout: format!("ableton-mcp-server {}\n", crate::host::SERVER_VERSION), ..Default::default() });
    }
    let command = args.first()?.as_str();
    let arguments = &args[1..];
    Some(match command {
        "lifecycle" => crate::lifecycle_cli::run(arguments).await,
        "setup" => crate::setup::run(arguments),
        "migrate" => crate::migrate::run(arguments),
        "diagnostics" => crate::diagnostics::run(arguments).await,
        "install-remote-script" => crate::install_remote_script::run(arguments),
        _ => return None,
    })
}

/// Validate exactly the source's normal stdio command shape before reading configuration.
pub fn config_argument(args: &[String]) -> Result<Option<PathBuf>, CommandOutput> {
    if args.iter().filter(|s| s.as_str() == "--config").count() > 1 {
        return Err(CommandOutput::error("mcp-host: repeated --config", 2));
    }
    if args.is_empty() {
        return Ok(None);
    }
    if args.len() == 2 && args[0] == "--config" && !args[1].starts_with('-') {
        if args[1].is_empty() {
            return Err(CommandOutput::error("mcp-host: --config requires a path", 2));
        }
        return Ok(Some(PathBuf::from(&args[1])));
    }
    Err(CommandOutput::error("mcp-host: unknown option", 2))
}
pub async fn configured_adapter(path: &Path) -> Result<Rc<dyn AsyncLiveAdapter>, LiveError> {
    let config = read_any_config(path)?;
    let bridge = config.bridge().ok_or_else(|| LiveError::error("version-1 configuration does not enable a Live adapter"))?;
    let endpoint = RemoteScriptEndpoint {
        host: bridge.host.clone(),
        port: bridge.port,
        secret: read_secret_file(&bridge.secret_file)?,
        timeout_ms: Some(bridge.timeout_ms),
        mutation_path: None,
        retire_after: None,
    };
    let remote: Rc<dyn AsyncLiveAdapter> = Rc::new(RemoteScriptLiveAdapter::connect(endpoint).await?);
    if std::env::var("ABLETON_MCP_EXTENSION").ok().as_deref() == Some("off") {
        return Ok(remote);
    }
    let mut setup = ExtensionSetup::new(
        std::env::var_os("ABLETON_MCP_EXTENSION_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).join("live-extension")),
    );
    setup.installed_storage = kumi_extension_folders(None, None, None).map(|f| f.data);
    setup.launch = Some(std::env::var("ABLETON_MCP_EXTENSION").ok().as_deref() != Some("external"));
    setup.log = Some(Rc::new(|line| eprintln!("mcp-host: {line}")));
    Ok(Rc::new(with_extension(remote, setup)))
}

pub fn main() -> i32 {
    let args: Vec<String> = kumi_common::env::args().skip(1).collect();
    // The metadata probe needs neither configuration nor asynchronous runtime initialization.
    if args.len() == 1 && args[0] == "--version" {
        println!("ableton-mcp-server {}", crate::host::SERVER_VERSION);
        return 0;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return CommandOutput::error(format!("mcp-host: {error}"), 1).emit(),
    };
    tokio::task::LocalSet::new().block_on(&runtime, async {
        if let Some(output) = auxiliary_command(&args).await {
            return output.emit();
        }
        let path = match config_argument(&args) {
            Ok(path) => path,
            Err(output) => return output.emit(),
        };
        let result = async {
            let adapter = match path {
                Some(path) => Some(configured_adapter(&path).await?),
                None => None,
            };
            crate::serve::serve(tokio::io::stdin(), tokio::io::stdout(), tokio::io::stderr(), adapter, Default::default()).await
        }
        .await;
        match result {
            Ok(()) => 0,
            Err(error) => CommandOutput::error(format!("mcp-host: {error}"), 1).emit(),
        }
    })
}
