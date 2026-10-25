//! The jobs kumi runs as a process of its own, told by a first argument no producer types: the library learner
//! (`kumi __library-learner`, with `--music` for the producer's own music) and the sound measurer
//! (`kumi __library-measure`). Until 1.24 they were programs of their own beside kumi, each nine tenths kumi's code
//! again; as doors into the one program they cost a bundle nothing.

/// Runs the job `argument` names, when it names one: the process's exit code.
pub fn run(argument: &str) -> Option<i32> {
    match argument {
        "__library-learner" => Some(library_learner(std::env::args().nth(2).as_deref() == Some("--music"))),
        "__library-measure" => Some(library_measure()),
        _ => None,
    }
}

/// Low-priority background learner for the producer's library; `music`, for their own music (the music scan).
fn library_learner(music: bool) -> i32 {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("library runtime");
    let local = tokio::task::LocalSet::new();
    let daws = crate::daw::registry::libraries();
    let result = if music {
        // Where Kumi keeps the programs it fetches: the scan finds its ffmpeg there (or fetches it), as the app does.
        let tools = crate::config::load_tools_dir(&kumi_types::system::process_env()).ok();
        runtime.block_on(local.run_until(kumi_listening::music::worker::main(daws, tools)))
    } else {
        runtime.block_on(local.run_until(kumi_library::learner::main(daws)))
    };
    // Like the source's process.exit, end after flushing replies. Tokio's stdin uses a blocking reader; waiting for
    // runtime shutdown would wait for the parent's still-open IPC pipe.
    std::process::exit(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    });
}

/// Isolated sound measurement for the library learner.
fn library_measure() -> i32 {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("measure runtime");
    match runtime.block_on(kumi_library::measure_worker::main()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}
