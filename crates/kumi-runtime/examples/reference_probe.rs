//! Runs the reference tool once, for checking it against the real services: reference_probe <folder> "<what>" [tracks]
//! The folder keeps the measured reference and its audio; yt-dlp comes from KUMI_YTDLP or is fetched into it.
use kumi_common::abort::Signal;
use kumi_runtime::{
    references::{store::ReferenceStore, tool::reference_tools},
    video::programs::ProgramOptions,
};
use std::rc::Rc;

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    tokio::task::LocalSet::new().block_on(&runtime, async {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let (Some(folder), Some(what)) = (argv.first(), argv.get(1)) else {
            eprintln!("reference_probe <folder> \"<what>\" [tracks]");
            std::process::exit(2);
        };
        let tracks: u64 = argv.get(2).and_then(|n| n.parse().ok()).unwrap_or(3);
        let store = Rc::new(ReferenceStore::new(std::path::Path::new(folder).join("kept")));
        let programs =
            ProgramOptions { tools_dir: std::path::Path::new(folder).join("tools").to_string_lossy().into(), ..Default::default() };
        let tool = reference_tools(store, programs, None).remove(0);
        let input = serde_json::json!({"what": what, "tracks": tracks}).as_object().unwrap().clone();
        let started = std::time::Instant::now();
        let result = tool.execute(input, Signal::new()).await.expect("the tool ran");
        println!("{} after {:.0} s{}", if result.is_error { "failed" } else { "done" }, started.elapsed().as_secs_f64(), "");
        println!("{}", result.text);
    });
}
