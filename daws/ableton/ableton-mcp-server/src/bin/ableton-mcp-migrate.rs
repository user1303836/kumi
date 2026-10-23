//! The `ableton-mcp-migrate` command.
fn main() {
    std::process::exit(ableton_mcp_server::lifecycle::migrate::main());
}
