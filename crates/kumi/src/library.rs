//! Inspect and rebuild the producer's library.
use crate::{
    config::{load_library_dir, load_projects_dir, load_settings_file, read_settings},
    tui::tty::TtyOutput,
};
use kumi_common::{abort::Signal, js::number, time::now_ms};
use kumi_runtime::{
    core::errors::RuntimeError,
    integrations::ableton::project::since,
    library::{create_library, learn::LearnPhase, library_logs, read_state, LearnNowOptions, LearnProgress, Library, LibraryOptions},
    system::Env,
    KUMI,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[derive(Clone)]
pub struct LibraryIo {
    pub out: Rc<dyn TtyOutput>,
    pub env: Env,
    pub rebuild: bool,
    pub signal: Option<Signal>,
    pub library: Option<Rc<Library>>,
    pub now: Option<Rc<dyn Fn() -> f64>>,
}
impl LibraryIo {
    pub fn new(out: Rc<dyn TtyOutput>, env: Env) -> Self {
        Self { out, env, rebuild: false, signal: None, library: None, now: None }
    }
}
fn tilde(path: &str) -> String {
    let home = home::home_dir().unwrap_or_default().display().to_string();
    path.strip_prefix(&home).map(|rest| format!("~{rest}")).unwrap_or_else(|| path.into())
}
fn grouped(value: usize) -> String {
    let text = value.to_string();
    let mut out = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i > 0 && (text.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
fn counted(value: usize, one: &str) -> String {
    format!("{} {one}{}", grouped(value), if value == 1 { "" } else { "s" })
}
fn minutes(ms: f64) -> String {
    if ms < 90000. {
        format!("{} seconds", number::to_string(number::round(ms / 1000.).max(1.)))
    } else {
        format!("{} minutes", number::to_string(number::round(ms / 60000.)))
    }
}
fn phase_name(phase: LearnPhase) -> &'static str {
    match phase {
        LearnPhase::Looking => "looking",
        LearnPhase::Presets => "presets",
        LearnPhase::Sets => "sets",
        LearnPhase::Sounds => "sounds",
        LearnPhase::Tidying => "tidying",
        LearnPhase::Done => "done",
    }
}
pub fn progress_line(progress: &LearnProgress) -> String {
    let at = || progress.at.as_ref().filter(|at| !at.is_empty()).map(|at| format!(" ({at})")).unwrap_or_default();
    match progress.phase {
        LearnPhase::Looking => format!("Looking through your folders{}…", at()),
        LearnPhase::Presets => format!("Presets: {} of {}", grouped(progress.presets.done), grouped(progress.presets.todo)),
        LearnPhase::Sets => format!("Sets: {} of {}", grouped(progress.sets.done), grouped(progress.sets.todo)),
        LearnPhase::Sounds => format!("Sounds: {} of {}{}", grouped(progress.sounds.done), grouped(progress.sounds.todo), at()),
        LearnPhase::Tidying => "Tidying up…".into(),
        LearnPhase::Done => "Done.".into(),
    }
}
pub async fn run_library(io: LibraryIo) -> Result<i32, RuntimeError> {
    let now = io.now.clone().unwrap_or_else(|| Rc::new(|| now_ms() as f64));
    let dir = load_library_dir(&io.env)?;
    let owned = io.library.is_none();
    let library = if let Some(library) = io.library.clone() {
        library
    } else {
        create_library(LibraryOptions {
            dir: dir.clone(),
            folders: Some(read_settings(&load_settings_file(&io.env)?).library_folders),
            projects_dir: Some(load_projects_dir(&io.env)?),
            ..Default::default()
        })
    };
    let result = async {
        let out = &io.out;
        if io.rebuild {
            out.write("Learning your library again, from the start. Ctrl-C stops; Kumi carries on from there next time it runs.\n");
            let shown = RefCell::new(String::new());
            let last = Cell::new(0.);
            let progress_out = out.clone();
            let progress_now = now.clone();
            let result = library
                .learn_now(LearnNowOptions {
                    rebuild: true,
                    signal: io.signal.clone().unwrap_or_default(),
                    on_progress: Some(Rc::new(move |progress| {
                        let line = progress_line(&progress);
                        if *shown.borrow() == line || (progress_now() - last.get() < 1000. && progress.phase != LearnPhase::Done) {
                            return;
                        }
                        *shown.borrow_mut() = line.clone();
                        last.set(progress_now());
                        progress_out.write(&if progress_out.is_tty() { format!("\r\x1b[2K  {line}") } else { format!("  {line}\n") });
                    })),
                })
                .await;
            let result = match result {
                Err(_) if io.signal.as_ref().is_some_and(Signal::is_cancelled) => {
                    out.write(&format!("{}Stopped. What Kumi learned so far is kept.\n", if out.is_tty() { "\n" } else { "" }));
                    return Ok(1);
                }
                other => other?,
            };
            let Some(result) = result else {
                out.write("Kumi is learning your library in another window right now. Quit that Kumi first, then run this again.\n");
                return Ok(1);
            };
            out.write(&format!(
                "{}Learned {}, {} and {} in {}.\n\n",
                if out.is_tty() { "\n" } else { "" },
                counted(result.sounds.known, "sound"),
                counted(result.presets.known, "preset"),
                counted(result.sets.known, "Set"),
                minutes(result.finished_at.map(|ms| ms as f64).unwrap_or_else(|| now()) - result.started_at as f64)
            ));
        }
        let state = read_state(&dir).await;
        let logs = library_logs(&dir);
        let (sounds, presets, sets) = tokio::join!(logs.sounds.load(), logs.presets.load(), logs.sets.load());
        let name_only = sounds.values().filter(|entry| entry.vector.as_ref().is_none_or(|v| v.is_empty())).count();
        let mut lines = vec![format!("Kumi's library ({})", tilde(&dir)), "".into()];
        if sounds.is_empty() && presets.is_empty() && sets.is_empty() && state.as_ref().and_then(|s| s.learning.as_ref()).is_none() {
            lines.extend([
                "  Not learned yet. Kumi learns it by itself in the background while it runs,".into(),
                format!("  or here, now: {} library --rebuild", *KUMI),
            ]);
        } else {
            lines.extend([
                format!(
                    "  Sounds    {}{}",
                    grouped(sounds.len()),
                    if name_only > 0 {
                        format!(
                            " ({} known by name only: Kumi couldn't read {})",
                            grouped(name_only),
                            if name_only == 1 { "it" } else { "them" }
                        )
                    } else {
                        String::new()
                    }
                ),
                format!("  Presets   {}", grouped(presets.len())),
                format!("  Sets      {}", grouped(sets.len())),
            ]);
            if let Some(learning) = state.as_ref().and_then(|s| s.learning.as_ref()) {
                lines.push(format!(
                    "  Learning  now: {}",
                    if learning.phase == LearnPhase::Sounds {
                        format!("{} of {} new sounds", grouped(learning.sounds.done), grouped(learning.sounds.todo))
                    } else {
                        format!("{}…", phase_name(learning.phase))
                    }
                ));
            }
            if let Some(last) = state.as_ref().and_then(|s| s.last.as_ref()) {
                lines.push(format!(
                    "  Learned   {} (in {}); Kumi looks again every half hour while it runs",
                    since(last.finished_at as f64, now()),
                    minutes((last.finished_at - last.started_at) as f64)
                ));
            }
        }
        lines.extend(["".into(), "Where Kumi looks".into()]);
        let sources = library.sources();
        let width = sources.iter().map(|source| source.label.encode_utf16().count()).max().unwrap_or(0).max(12) + 2;
        for source in &sources {
            lines.push(format!("  {}{}{}", source.label, " ".repeat(width - source.label.encode_utf16().count()), tilde(&source.path)));
        }
        if sources.is_empty() {
            lines.push("  Nowhere yet: Live's User Library wasn't found.".into());
        }
        let taste = library.taste().await?;
        if !taste.is_empty() {
            lines.extend(["".into(), "From your Sets (forget a line in Kumi's /memory)".into()]);
            for line in taste {
                lines.push(format!("  {}", line.line));
            }
        }
        lines.extend([
            "".into(),
            format!(
                "More folders: add them to {} as \"libraryFolders\": [\"~/Samples\"]. {} library --rebuild learns everything again.",
                tilde(&load_settings_file(&io.env)?),
                *KUMI
            ),
        ]);
        out.write(&format!("{}\n", lines.join("\n")));
        Ok(0)
    }
    .await;
    if owned {
        library.close().await;
    }
    result
}
