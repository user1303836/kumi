//! Isolated sound measurement for the library learner.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = kumi_runtime::library::measure_worker::main().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
