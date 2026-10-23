//! The `ableton-mcp-lifecycle` command.
fn main() {
    std::process::exit(ableton_mcp_server::lifecycle::cli::main());
}
