//! The `kumi` command, the jobs it runs as a process of its own (`kumi::workers`), and `kumi serve`, the headless
//! runtime another agent's host starts: it branches off here, before the app reads its settings, keys or sign-in.
fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("serve") {
        let rest: Vec<String> = args.collect();
        std::process::exit(kumi::serve::main(&rest));
    }
    if let Some(code) = first.as_deref().and_then(kumi::workers::run) {
        std::process::exit(code);
    }
    std::process::exit(kumi::cli::main());
}
