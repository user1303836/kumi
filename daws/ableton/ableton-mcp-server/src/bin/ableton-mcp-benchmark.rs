#[global_allocator]
static ALLOCATOR: ableton_mcp_server::benchmark::memory::MeasuredAllocator = ableton_mcp_server::benchmark::memory::MeasuredAllocator;

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let result = tokio::task::LocalSet::new().block_on(&runtime, ableton_mcp_server::benchmark::run_benchmarks());
    match result {
        Ok(report) => {
            println!("{}", serde_json::to_string(&report).unwrap());
            if !report.passed {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
