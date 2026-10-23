//! The `ableton-mcp-diagnostics` command.
fn main() {
    std::process::exit(ableton_mcp_server::lifecycle::diagnostics::main());
}
