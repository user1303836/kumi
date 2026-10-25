//! The `kumi` command, and the jobs it runs as a process of its own (`kumi::workers`).
fn main() {
    if let Some(code) = std::env::args().nth(1).as_deref().and_then(kumi::workers::run) {
        std::process::exit(code);
    }
    std::process::exit(kumi::cli::main());
}
