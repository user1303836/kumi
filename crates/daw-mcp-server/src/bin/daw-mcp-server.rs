//! The `daw-mcp-server` command: `daw-mcp-server --daw reaper|renoise|logic`.
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match daw_mcp_server::parse_args(&args, &|name| std::env::var(name).ok(), home_dir()) {
        Ok(options) => options,
        // `--version` is said on stdout, as programs do; a wrong argument on stderr.
        Err(message) if message.starts_with("daw-mcp-server ") => {
            println!("{message}");
            std::process::exit(0);
        }
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
    let mut host = daw_mcp_server::host::Host::new(options.daw, &options.kumi_home);
    if let Some(binary) = options.logic_bridge {
        host = host.with_bridge_binary(binary);
    }
    let host = Arc::new(host);
    let served = runtime.block_on(daw_mcp_server::stdio::serve(host, tokio::io::stdin(), tokio::io::stdout()));
    if let Err(error) = served {
        eprintln!("mcp-host: {error}");
        std::process::exit(1);
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|home| !home.is_empty()).map(Into::into)
}
