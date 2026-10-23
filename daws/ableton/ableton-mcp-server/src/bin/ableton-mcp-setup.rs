//! The `ableton-mcp-setup` command.
fn main() {
    std::process::exit(ableton_mcp_server::lifecycle::setup::main());
}
