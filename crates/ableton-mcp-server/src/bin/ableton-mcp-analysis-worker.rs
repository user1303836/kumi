//! The analysis job worker the bridge spawns.
#[global_allocator]
static ALLOCATOR: ableton_mcp_server::benchmark::memory::MeasuredAllocator = ableton_mcp_server::benchmark::memory::MeasuredAllocator;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    std::process::exit(if args.first().is_some_and(|arg| arg == "--benchmark") {
        ableton_mcp_server::analysis_worker::main(&args[1..])
    } else {
        ableton_mcp_server::analysis_job_worker::main()
    });
}
