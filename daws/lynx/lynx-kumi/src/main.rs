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
    let mut iter = args.iter().skip(usize::from(launching));
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--kumi-home" => match iter.next() {
                Some(path) => kumi_home = Some(PathBuf::from(path)),
                None => return failed("--kumi-home needs a folder"),
            },
            "--native-config" => match iter.next() {
                Some(path) => native_path = Some(PathBuf::from(path)),
                None => return failed("--native-config needs an absolute file"),
            },
            "--background" if launching => {}
            name if launching && !name.starts_with('-') && session.is_none() => session = Some(PathBuf::from(name)),
            _ => {
                return failed(
                    "usage: lynx-kumi [launch] [--kumi-home <folder>] [--native-config <absolute discovery file>] [session for launch]",
                )
            }
        }
    }
    let Some(home) = kumi_home.or_else(daw_bridge::paths::kumi_home) else {
        return failed("KUMI_HOME or --kumi-home is required");
    };
    let Some(native_path) = native_path.or_else(launch::native_path) else {
        return failed("KUMI_LYNX_CONFIG must name the opt-in LYNX control discovery file");
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(r) => r,
        Err(_) => return failed("Cannot start the bridge runtime"),
    };
    if launching {
        let result = runtime.block_on(launch::run(&home, &native_path, session.as_deref()));
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
    daw_bridge::owner::create_owner_only_dir(folder)?;
    #[cfg(unix)]
    {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(folder.join(format!("serving-{pid}.lock")))?;
        // SAFETY: this open regular file owns the descriptor throughout the lock.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error);
        }
        Ok(Some(file))
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "Native LYNX endpoint is POSIX-only"))
    }
}
