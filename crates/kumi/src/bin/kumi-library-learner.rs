//! Low-priority background learner for the producer's library; with `--music`, for their own music (the music scan).
fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("library runtime");
    let local = tokio::task::LocalSet::new();
    let daws = kumi::daw::registry::libraries();
    let result = if std::env::args().nth(1).as_deref() == Some("--music") {
        runtime.block_on(local.run_until(kumi_listening::music::worker::main(daws)))
    } else {
        runtime.block_on(local.run_until(kumi_library::learner::main(daws)))
    };
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
