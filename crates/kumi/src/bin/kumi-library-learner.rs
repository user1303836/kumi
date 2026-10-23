//! Low-priority background learner for the producer's library.
fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("library runtime");
    let local = tokio::task::LocalSet::new();
    let result = runtime.block_on(local.run_until(kumi_library::learner::main(kumi::daw::registry::libraries())));
    // Like the source's process.exit, end after flushing replies. Tokio's stdin uses a blocking
    // reader; waiting for runtime shutdown would wait for the parent's still-open IPC pipe.
    std::process::exit(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    });
}
