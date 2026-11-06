//! The host starts this companion with a pipe on stdin; EOF ends the bridge.
use lynx_kumi::{launch, native, server};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};
use tokio::io::AsyncReadExt;

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["--version"] {
        println!("lynx-kumi {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let launching = args.first().is_some_and(|arg| arg == "launch");
    let (mut kumi_home, mut native_path, mut session) = (None, None, None);
    let mut background = false;
    let mut iter = args.iter().skip(usize::from(launching));
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--kumi-home" => match iter.next() {
                Some(path) => kumi_home = Some(PathBuf::from(path)),
                None => return usage("--kumi-home needs a folder"),
            },
            "--native-config" => match iter.next() {
                Some(path) => native_path = Some(PathBuf::from(path)),
                None => return usage("--native-config needs an absolute file"),
            },
            "--background" if launching => background = true,
            name if launching && !name.starts_with('-') && session.is_none() => session = Some(PathBuf::from(name)),
            _ => {
                return usage(
                    "usage: lynx-kumi [launch] [--kumi-home <folder>] [--native-config <absolute discovery file>] [session for launch]",
                )
            }
        }
    }
    let Some(home) = kumi_home.or_else(daw_bridge::paths::kumi_home) else {
        return failed("KUMI_HOME or --kumi-home is required");
    };
    let native_path = match native_path {
        Some(path) => path,
        None => match if launching { Ok(launch::native_path()) } else { launch::serving_path(&home) } {
            Ok(Some(path)) => path,
            Ok(None) => return failed("LYNX's native discovery path is unavailable; use --native-config or LYNX_CONTROL_DISCOVERY"),
            Err(error) => return failed(&error.to_string()),
        },
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(r) => r,
        Err(_) => return failed("Cannot start the bridge runtime"),
    };
    if launching {
        let result = runtime.block_on(launch::run(&home, &native_path, session.as_deref(), background));
        runtime.shutdown_background();
        return match result {
            Ok(value) => {
                println!("{value}");
                ExitCode::SUCCESS
            }
            Err(error) => failed(&error.to_string()),
        };
    }
    let found = match native::discovery(&native_path) {
        Ok(found) => found,
        Err(error) => return failed(&error.to_string()),
    };
    let folder = home.join("bridges/lynx");
    let serving = match serving_lock(&folder, found.pid) {
        Ok(Some(file)) => file,
        Ok(None) => return failed("Another bridge already serves this LYNX process"),
        Err(_) => return failed("Cannot hold the LYNX serving lock"),
    };
    let mut config = server::Config::new(native_path, folder.join("bridge.secret"), folder.join("discovery"));
    config.serving = Some(serving);
    let result = runtime.block_on(server::serve(config, async {
        let mut input = tokio::io::stdin();
        let mut bytes = [0; 128];
        while matches!(input.read(&mut bytes).await, Ok(1..)) {}
    }));
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => failed(&error.to_string()),
    }
}
fn failed(message: &str) -> ExitCode {
    let _ = writeln!(std::io::stderr(), "lynx-kumi: {message}");
    ExitCode::FAILURE
}
fn serving_lock(folder: &Path, pid: u32) -> std::io::Result<Option<std::fs::File>> {
    launch::private_directory(folder).map_err(std::io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(folder.join(format!("serving-{pid}.lock")))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

fn usage(message: &str) -> ExitCode {
    let _ = writeln!(std::io::stderr(), "lynx-kumi: {message}");
    ExitCode::from(2)
}
