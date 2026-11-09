//! The `kumi` command, the jobs it runs as a process of its own (`kumi::workers`), `kumi serve`, the headless runtime
//! another agent's host starts, and `kumi design`, one headless sound-design render: they branch off here, before the
//! app reads its settings, keys or sign-in.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    let serve = first.as_deref() == Some(std::ffi::OsStr::new("serve"));
    if serve || first.as_deref() == Some(std::ffi::OsStr::new("design")) {
        let rest: Result<Vec<String>, std::ffi::OsString> = args.map(std::ffi::OsString::into_string).collect();
        let code = match (rest, serve) {
            (Ok(rest), true) => kumi::serve::main(&rest),
            (Ok(rest), false) => kumi::design::main(&rest),
            (Err(bad), _) => {
                use std::io::Write;
                let _ = writeln!(std::io::stderr(), "kumi: arguments are text, not {}", bad.to_string_lossy());
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
