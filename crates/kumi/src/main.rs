//! The `kumi` command, the jobs it runs as a process of its own (`kumi::workers`), and `kumi serve`, the headless
//! runtime another agent's host starts: it branches off here, before the app reads its settings, keys or sign-in.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("serve")) {
        let rest: Result<Vec<String>, std::ffi::OsString> = args.map(std::ffi::OsString::into_string).collect();
        let code = match rest {
            Ok(rest) => kumi::serve::main(&rest),
            Err(bad) => {
                use std::io::Write;
                let _ = writeln!(std::io::stderr(), "kumi serve: arguments are text, not {}", bad.to_string_lossy());
                2
            }
        };
        std::process::exit(code);
    }
    if let Some(code) = first.as_ref().and_then(|first| first.to_str()).and_then(kumi::workers::run) {
        std::process::exit(code);
    }
    std::process::exit(kumi::cli::main());
}
