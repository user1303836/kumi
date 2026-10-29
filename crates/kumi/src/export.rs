//! A conversation as a file to keep or share: `kumi export` and `kumi import`, the hooks Kumi's exports draw with (step
//! words, pictures, the time zone, the redactor `kumi report` uses), and files written beside the producer, private and
//! never over another.
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;

use chrono::{FixedOffset, Local, Offset};
use futures::future::{FutureExt, LocalBoxFuture};
use kumi_agent::core::contracts::{ConversationMatch, ConversationStore, ConversationSummary, ConversationToExport, SessionController};
use kumi_agent::core::conversations::FileConversationStore;
use kumi_agent::core::export::{file_name, html, html_text, import, importable, json, markdown, Format, Options, Source, IMPORT_MAX_BYTES};
use kumi_agent::gone_words;
use kumi_common::js::string::head;
use kumi_common::time::now_ms;
use kumi_types::contracts::{ChangePicture, ChangeRecord, ColorChange, DevicePlacement};
use kumi_types::place::IMPORTED;
use kumi_types::system::{self, Env};
use kumi_types::{errors::RuntimeError, version::KUMI_VERSION};

use crate::config::{load_projects_dir, load_settings_file, read_settings};
use crate::report::{known_secrets, redactor, username};
use crate::tui::pictures::scene::{scene_of, Shape, GHOST, GUIDE, NOW};
use crate::tui::style::{palette, Rgb};
use crate::tui::transcript::step_label;
use crate::tui::tty::TtyOutput;

/// The most `/export` copies through the terminal; a longer conversation is saved as a file instead.
pub const COPY_MOST: usize = 64 * 1024;
/// The DAW's words in what Kumi's commands export for a conversation of Live's (or one imported from a file).
pub const WORDS: (&str, &str) = ("Live", "Set");
/// A picture's size on the page, in its own units: wide, as the terminal draws it.
const WIDTH: f64 = 400.;
const HEIGHT: f64 = 160.;

/// The redactor `kumi report` uses, with what this computer knows to be secret: sign-ins, keys in the environment, the
/// home folder and the account name.
pub async fn redactor_for(env: &Env) -> Box<dyn Fn(&str) -> String> {
    let secrets = known_secrets(env).await;
    let home = home::home_dir().unwrap_or_default().display().to_string();
    Box::new(redactor(&secrets, &home, &username()))
}

/// A conversation in `format`, its dates told in `zone` ([`local_zone`]). `redact` is None only for a file the producer
/// asked for as it is (`--raw`).
pub fn render(format: Format, source: &Source, full: bool, redact: Option<&dyn Fn(&str) -> String>, zone: FixedOffset, now: i64) -> String {
    render_with(format, source, full, redact, zone, now, &step_label)
}

/// `render`, each step named by `label` (its DAW's words for its tool).
fn render_with(
    format: Format,
    source: &Source,
    full: bool,
    redact: Option<&dyn Fn(&str) -> String>,
    zone: FixedOffset,
    now: i64,
    label: &dyn Fn(&str) -> String,
) -> String {
    let same = |text: &str| text.to_string();
    let options = Options {
        full,
        redact: redact.unwrap_or(&same),
        redacted: redact.is_some(),
        label,
        picture: &picture_of,
        zone,
        now,
        version: KUMI_VERSION,
    };
    match format {
        Format::Markdown => markdown(source, &options),
        Format::Html => html(source, &options),
        Format::Json => json(source, &options),
    }
}

/// This computer's time zone, as it is now.
pub fn local_zone() -> FixedOffset {
    Local::now().offset().fix()
}

/// A change's picture for the web page, from its record alone, as HISTORY draws it: what the change drew (an equaliser's
/// curve, a compressor's, notes, a lane), else a new clip's notes, where a device went, the colours it moved between or
/// the value it moved from and to. None when there's nothing to draw.
pub fn picture_of(change: &ChangeRecord) -> Option<String> {
    // As its saved record keeps it, so a page written in the run that made the change draws what one written after a
    // restart does: a note restated with fewer decimals is the note it was.
    if let Some(picture) = &change.picture {
        return svg_of(&picture.clone().as_kept());
    }
    if let Some(clip) = &change.clip {
        return svg_of(&ChangePicture::Notes { length: clip.length, before: vec![], after: clip.notes.clone(), partial: false });
    }
    if let Some(devices) = &change.devices {
        return devices_of(devices);
    }
    if let Some(colors) = &change.colors {
        return swatches_of(colors);
    }
    bars_of(change.from?, change.to?, change.range?)
}

/// Where a device went, as a line of names with the new one lit; inside a rack, its chains, the one it went in lit.
fn devices_of(placement: &DevicePlacement) -> Option<String> {
    let row = |devices: &[String], lit: Option<f64>| {
        devices
            .iter()
            .enumerate()
            .map(|(at, name)| {
                let name = html_text(if name.is_empty() { "Device" } else { name });
                if lit == Some(at as f64) {
                    format!("<strong>{name}</strong>")
                } else {
                    name
                }
            })
            .collect::<Vec<_>>()
            .join(" <span>→</span> ")
    };
    let chains = placement.chains.as_deref().unwrap_or_default();
    if chains.is_empty() {
        let devices = placement.devices.as_ref().filter(|devices| !devices.is_empty())?;
        return Some(format!("<p class=\"chain\">{}</p>", row(devices, placement.index)));
    }
    let focus = placement.chain.filter(|chain| chain.is_finite() && *chain >= 0.).unwrap_or(0.) as usize;
    let mut out = String::from("<div class=\"chain\">");
    if let Some(rack) = placement.rack.as_deref().filter(|rack| !rack.is_empty()) {
        out.push_str(&format!("<p class=\"rack\">{}</p>", html_text(rack)));
    }
    for (at, chain) in chains.iter().enumerate() {
        let name = html_text(&chain.name);
        // A chain lit whole is the new one; otherwise the new device in it is.
        let name = if at == focus && placement.index.is_none() { format!("<strong>{name}</strong>") } else { name };
        let devices =
            if chain.devices.is_empty() { "empty".to_string() } else { row(&chain.devices, placement.index.filter(|_| at == focus)) };
        out.push_str(&format!("<p>{name}: {devices}</p>"));
    }
    out.push_str("</div>");
    Some(out)
}

/// A colour change's before and after, as swatches in their own colours (written as "#rrggbb"; anything else draws none).
fn swatches_of(colors: &ColorChange) -> Option<String> {
    let hex = |color: &str| {
        (color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| color.to_ascii_lowercase())
    };
    let to = hex(&colors.to)?;
    let mut shapes = String::new();
    let mut x = 0.;
    if let Some(from) = colors.from.as_deref().and_then(hex) {
        shapes.push_str(&format!("<rect class=\"swatch\" x=\"2\" y=\"10\" width=\"120\" height=\"60\" rx=\"6\" fill=\"{from}\"/>"));
        shapes.push_str("<polyline class=\"guide\" points=\"140,40 182,40\" stroke-width=\"3\"/>");
        shapes.push_str("<polyline class=\"guide\" points=\"172,31 182,40 172,49\" stroke-width=\"3\"/>");
        x = 200.;
    }
    shapes.push_str(&format!("<rect class=\"swatch\" x=\"{x}\" y=\"10\" width=\"120\" height=\"60\" rx=\"6\" fill=\"{to}\"/>"));
    Some(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {WIDTH} 80\" aria-hidden=\"true\">{shapes}</svg>"))
}

/// A value's move within its range, as two bars: where it was, faint, and where it is now.
fn bars_of(from: f64, to: f64, [min, max]: [f64; 2]) -> Option<String> {
    if ![from, to, min, max].iter().all(|value| value.is_finite()) || max <= min {
        return None;
    }
    let width = |value: f64| ((value - min) / (max - min)).clamp(0., 1.) * WIDTH;
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {WIDTH} 70\" aria-hidden=\"true\"><rect class=\"guide\" x=\"0\" y=\"10\" width=\"{WIDTH}\" height=\"20\"/><rect class=\"ghost\" x=\"0\" y=\"10\" width=\"{:.1}\" height=\"20\"/><rect class=\"guide\" x=\"0\" y=\"40\" width=\"{WIDTH}\" height=\"20\"/><rect class=\"now\" x=\"0\" y=\"40\" width=\"{:.1}\" height=\"20\"/></svg>",
        width(from),
        width(to)
    ))
}

/// A change's picture as an `<svg>` the page colours itself, light or dark: curves as lines, notes as boxes. None for a
/// picture that reads as values (the page writes those out), or one with nothing to draw.
pub fn svg_of(picture: &ChangePicture) -> Option<String> {
    let scene = scene_of(picture)?;
    let mut shapes = String::new();
    for shape in &scene.shapes {
        match shape {
            Shape::Line { points, color, alpha, width } => {
                let points: Vec<String> = points
                    .iter()
                    .filter(|(x, y)| x.is_finite() && y.is_finite())
                    .map(|(x, y)| format!("{:.1},{:.1}", x * WIDTH, (1. - y) * HEIGHT))
                    .collect();
                if points.len() < 2 {
                    continue;
                }
                shapes.push_str(&format!(
                    "<polyline{} points=\"{}\" stroke-width=\"{:.2}\" stroke-opacity=\"{:.2}\"/>",
                    paint(*color, "stroke"),
                    points.join(" "),
                    width / 1000. * HEIGHT,
                    alpha.clamp(0., 1.)
                ));
            }
            Shape::Dot { x, y, radius, color, alpha } => {
                if ![x, y, radius].iter().all(|value| value.is_finite()) {
                    continue;
                }
                shapes.push_str(&format!(
                    "<circle{} cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.2}\" fill-opacity=\"{:.2}\"/>",
                    paint(*color, "fill"),
                    x * WIDTH,
                    (1. - y) * HEIGHT,
                    radius / 1000. * HEIGHT,
                    alpha.clamp(0., 1.)
                ));
            }
            Shape::Rect { x0, y0, x1, y1, color, alpha } => {
                if ![x0, y0, x1, y1].iter().all(|value| value.is_finite()) {
                    continue;
                }
                shapes.push_str(&format!(
                    "<rect{} x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill-opacity=\"{:.2}\"/>",
                    paint(*color, "fill"),
                    x0.min(*x1) * WIDTH,
                    (1. - y0.max(*y1)) * HEIGHT,
                    (x1 - x0).abs() * WIDTH,
                    (y1 - y0).abs() * HEIGHT,
                    alpha.clamp(0., 1.)
                ));
            }
        }
    }
    (!shapes.is_empty())
        .then(|| format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {WIDTH} {HEIGHT}\" aria-hidden=\"true\">{shapes}</svg>"))
}

/// A shape's colour: the page's own for what's there now, what came before, a guide or what stayed (so it follows light
/// and dark), else the colour itself.
fn paint(color: Rgb, attribute: &str) -> String {
    let role = if color == NOW {
        "now"
    } else if color == GHOST {
        "ghost"
    } else if color == GUIDE {
        "guide"
    } else if color == palette::DIM {
        "dim"
    } else {
        return format!(" {attribute}=\"#{:02x}{:02x}{:02x}\"", color[0], color[1], color[2]);
    };
    format!(" class=\"{role}\"")
}

/// Writes `text` to `path`, or beside it as `name-2.ext`, `name-3.ext`… when that's taken: never over another file. Only
/// the producer can read it, where the system allows. Returns where it went.
pub fn write_new(path: &Path, text: &str) -> io::Result<PathBuf> {
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = path.extension().map(|extension| format!(".{}", extension.to_string_lossy())).unwrap_or_default();
    for number in 1..=999 {
        let candidate = if number == 1 { path.to_path_buf() } else { path.with_file_name(format!("{stem}-{number}{extension}")) };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(text.as_bytes()).and_then(|()| file.flush()) {
                    drop(file);
                    let _ = std::fs::remove_file(&candidate);
                    return Err(error);
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "999 files of that name are there already"))
}

/// Shows a file in the system's file manager: selected in Finder or Explorer, its folder elsewhere.
pub fn reveal(path: &Path) {
    let mut command = match system::platform() {
        "darwin" => {
            let mut command = std::process::Command::new("open");
            command.arg("-R").arg(path);
            command
        }
        "win32" => {
            let mut command = std::process::Command::new("explorer");
            command.arg(format!("/select,{}", path.display()));
            command
        }
        _ => {
            let mut command = std::process::Command::new("xdg-open");
            command.arg(path.parent().unwrap_or(Path::new(".")));
            command
        }
    };
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    if let Ok(mut child) = command.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// The redactor for /export and /share, made now: `kumi report`'s, from what this computer knows then, else one taking out
/// `secrets`, the home folder and the account name.
pub async fn redactor_now(make: Option<&MakeRedactor>, secrets: &[String]) -> Rc<dyn Fn(&str) -> String> {
    match make {
        Some(make) => make().await,
        None => {
            let home = home::home_dir().unwrap_or_default().display().to_string();
            Rc::new(redactor(secrets, &home, &username()))
        }
    }
}

/// Makes what an export takes out, when it's made: [`redactor_for`] then, so a key added since Kumi started (signed in
/// with /login, say) is known.
pub type MakeRedactor = Rc<dyn Fn() -> LocalBoxFuture<'static, Rc<dyn Fn(&str) -> String>>>;

/// A [`MakeRedactor`] for these variables.
pub fn redactor_maker(env: &Env) -> MakeRedactor {
    let env = env.clone();
    Rc::new(move || {
        let env = env.clone();
        async move { Rc::from(redactor_for(&env).await) }.boxed_local()
    })
}

/// The conversation going on now as a file: its text, its name, and what it holds as a key (the same while nothing in
/// the conversation changes, whenever it's exported).
pub struct Exported {
    pub text: String,
    pub name: String,
    pub key: u64,
}

/// The conversation going on now in `format`, redacted; Err, in the producer's words, before there's anything to export.
pub async fn conversation_now(
    controller: &dyn SessionController,
    format: Format,
    redact: &dyn Fn(&str) -> String,
) -> Result<Exported, String> {
    exported(controller.conversation_to_export().await, format, redact)
}

/// The conversation going on now (`found`, as the session gave it to export) in `format`, redacted; Err, in the
/// producer's words, before there's anything to export.
pub fn exported(found: Option<ConversationToExport>, format: Format, redact: &dyn Fn(&str) -> String) -> Result<Exported, String> {
    let found = found.ok_or("There's nothing to export yet: ask Kumi something first.")?;
    let source = Source {
        id: &found.id,
        conversation: &found.conversation,
        title: &found.title,
        place: found.set.as_deref(),
        words: found.words,
        created: found.created,
    };
    let text = render(format, &source, false, Some(redact), local_zone(), now_ms());
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (format.extension(), &found.id, &found.title, &found.set).hash(&mut hasher);
        serde_json::to_string(&found.conversation.checkpoint.messages).unwrap_or_default().hash(&mut hasher);
        serde_json::to_string(&found.conversation.changes).unwrap_or_default().hash(&mut hasher);
        hasher.finish()
    };
    Ok(Exported { text, name: file_name(&redact(&found.title), &found.id, format), key })
}

/// Writes an export into `folder`, or the folder Kumi was started in, never over another file; Err says why not.
pub fn save_in(folder: Option<&Path>, text: &str, name: &str) -> Result<PathBuf, String> {
    let folder = match folder {
        Some(folder) => folder.to_path_buf(),
        None => std::env::current_dir().map_err(|error| format!("Kumi can't tell which folder it's in ({error}), so it saved nothing."))?,
    };
    let path = folder.join(name);
    write_new(&path, text).map_err(|error| format!("Kumi couldn't save {}: {error}.", path.display()))
}

/// What Kumi says once it saved this conversation in `format` at `path`; `command` is how the producer runs Kumi.
pub fn saved_words(format: Format, path: &Path, command: &str) -> String {
    let (what, after) = match format {
        Format::Markdown => ("as Markdown", String::new()),
        Format::Html => ("as a web page", String::new()),
        Format::Json => ("for Kumi", format!(" Another Kumi carries it on after: {command} import <this file>.")),
    };
    format!("Saved this conversation {what}: {}.{after} Secrets, your home folder and your account name are taken out.", path.display())
}

/// What `kumi export` was asked for.
pub struct ExportRequest {
    /// Its id, the start of one or its name; None for the latest.
    pub conversation: Option<String>,
    pub format: Format,
    /// A file, or a folder for the usual name; None for the folder Kumi runs in.
    pub out: Option<String>,
    pub full: bool,
    /// As it is, without the redactor.
    pub raw: bool,
}

/// What a kept conversation's export calls the DAW and a place in it, and how it names each step, from the conversation's
/// place: REAPER's and Renoise's places are theirs; Live's, and conversations imported from files, are Live's.
pub fn words_of(place: &str) -> ((&'static str, &'static str), impl Fn(&str) -> String) {
    use kumi_types::{daw::DawId, place::Place};
    let daw = Place::parse(place).and_then(|place| place.daw()).filter(|daw| *daw != DawId::Ableton);
    let own = daw.and_then(|daw| crate::daw::registry::profile(daw).zip(crate::daw::registry::presentation(daw)));
    let words = own.map_or(WORDS, |(profile, _)| profile.word_pair());
    let label = move |tool: &str| match own {
        Some((profile, shown)) => kumi_agent::core::presentation::step_label(shown, profile, tool),
        None => step_label(tool),
    };
    (words, label)
}

/// The kept conversations, kept as the settings say.
fn kept(env: &Env) -> Result<Rc<FileConversationStore>, RuntimeError> {
    let settings = read_settings(&load_settings_file(env)?);
    Ok(kumi_agent::core::conversations::create_conversation_store_in(
        load_projects_dir(env)?,
        settings.conversations.as_ref().and_then(|c| c.keep_days),
        crate::daw::registry::places(),
    ))
}

/// Words from a file or the store, for the terminal: without control characters.
fn plain(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// `kumi export`: a kept conversation as a file in the folder Kumi runs in (or `out`), never over another, and redacted
/// unless it's `raw`. With none named, the latest, and it says which.
pub async fn export_command(
    request: ExportRequest,
    env: &Env,
    out: &dyn TtyOutput,
    err: &dyn TtyOutput,
    command: &str,
) -> Result<i32, RuntimeError> {
    let store = kept(env)?;
    let row = match &request.conversation {
        Some(word) => match store.find(word).await? {
            ConversationMatch::One(row) => row,
            ConversationMatch::Several(rows) => {
                err.write(&several(word, &rows));
                return Ok(1);
            }
            ConversationMatch::Gone => {
                err.write(&format!("{}\n", gone_words(store.days_kept().await)));
                return Ok(1);
            }
            ConversationMatch::None => {
                err.write(&format!("No conversation {}. {command} -r lists them.\n", head(&plain(word), 60)));
                return Ok(1);
            }
        },
        None => match store.all().await?.into_iter().next() {
            Some(row) => {
                out.write(&format!("Your latest conversation: {} · {}\n", plain(&row.title()), row.id));
                row
            }
            None => {
                err.write("No conversation is kept yet: talk with Kumi about a Set first.\n");
                return Ok(1);
            }
        },
    };
    let Some(conversation) = store.load(&row.place, &row.id).await? else {
        err.write(&format!("{}\n", gone_words(store.days_kept().await)));
        return Ok(1);
    };
    let redact = if request.raw { None } else { Some(redactor_for(env).await) };
    let title = row.title();
    // In the words of the DAW it was about (its place's), whichever DAW this Kumi was started for.
    let (words, label) = words_of(&row.place);
    let source = Source { id: &row.id, conversation: &conversation, title: &title, place: row.set.as_deref(), words, created: row.created };
    let text = render_with(request.format, &source, request.full, redact.as_deref(), local_zone(), now_ms(), &label);
    // The file's name comes from its title, so it's redacted too.
    let name = file_name(&redact.as_deref().map_or_else(|| title.clone(), |redact| redact(&title)), &row.id, request.format);
    let path = match request.out.as_deref() {
        Some(out) if Path::new(out).is_dir() => Path::new(out).join(&name),
        Some(out) => PathBuf::from(out),
        None => match std::env::current_dir() {
            Ok(folder) => folder.join(&name),
            Err(error) => {
                err.write(&format!("Kumi can't tell which folder it's in ({error}): choose where with -o <path>.\n"));
                return Ok(1);
            }
        },
    };
    let written = match write_new(&path, &text) {
        Ok(written) => written,
        Err(error) => {
            err.write(&format!("Kumi couldn't save {}: {error}.\n", path.display()));
            return Ok(1);
        }
    };
    let written = std::path::absolute(&written).unwrap_or(written);
    let what = match request.format {
        Format::Markdown => "Saved as Markdown",
        Format::Html => "Saved as a web page",
        Format::Json => "Saved for Kumi",
    };
    out.write(&format!("{what}: {}\n", written.display()));
    if request.format == Format::Json {
        out.write(&format!("Another Kumi carries it on after: {command} import <this file>\n"));
    }
    if request.raw {
        err.write("Nothing was taken out (--raw): this file may hold keys, tokens and paths. Share it with care.\n");
    } else {
        out.write("Secrets, your home folder and your account name are taken out.\n");
    }
    Ok(0)
}

/// The conversations a word matches, for the producer to name one by its id.
fn several(word: &str, rows: &[ConversationSummary]) -> String {
    let mut text = format!("Several conversations match {}; name one by its id:\n", head(&plain(word), 60));
    for row in rows.iter().take(10) {
        let set = row.set.as_deref().map(|set| format!(" · {}", plain(set))).unwrap_or_default();
        text.push_str(&format!("  {}  {}{set}\n", row.id, plain(&row.title())));
    }
    if rows.len() > 10 {
        text.push_str(&format!("  and {} more.\n", rows.len() - 10));
    }
    text
}

/// `kumi import`: a conversation saved with `kumi export --json`, kept with the others to carry on with `kumi -r` (in a
/// copy, where the producer is). The file is read as untrusted, and keeps its id when no kept conversation has it.
pub async fn import_command(file: &str, env: &Env, out: &dyn TtyOutput, err: &dyn TtyOutput, command: &str) -> Result<i32, RuntimeError> {
    let text = match read_bounded(Path::new(file), command) {
        Ok(text) => text,
        Err(why) => {
            err.write(&format!("{why}\n"));
            return Ok(1);
        }
    };
    let imported = match import(&text, now_ms(), command) {
        Ok(imported) => imported,
        Err(why) => {
            err.write(&format!("{why}\n"));
            return Ok(1);
        }
    };
    let store = kept(env)?;
    // The file's id, unless a conversation here has it (or had it): then another, saying why.
    let (taken, before) = match &imported.id {
        Some(id) => match store.find(id).await? {
            ConversationMatch::Gone => (true, false),
            ConversationMatch::One(row) if row.id == *id => (true, row.place == IMPORTED),
            ConversationMatch::Several(rows) if rows.iter().any(|row| row.id == *id) => (true, false),
            _ => (false, false),
        },
        None => (false, false),
    };
    let id = match imported.id.clone().filter(|_| !taken) {
        Some(id) => id,
        None => store.new_id().await,
    };
    // A name runs to 60 characters, with the mark.
    let name = format!("{} (imported)", head(&imported.title, 49));
    store.create(IMPORTED, &id, &imported.conversation, Some(name.as_str()), None).await?;
    // The store keeps the latest part of a long conversation; one whose last exchange alone is too long, it can't. What
    // it can't read back goes too: nothing's left of a refused import.
    if store.load(IMPORTED, &id).await?.is_none() {
        let why = match store.delete(IMPORTED, &id).await {
            Ok(true) => "Kumi can't keep that conversation: it couldn't read it back once kept, so it kept nothing of it.",
            _ => "Kumi can't keep that conversation: its last exchange runs longer than Kumi keeps of one (about 1 MB).",
        };
        err.write(&format!("{why}\n"));
        return Ok(1);
    }
    if let Some(set) = &imported.place {
        store.note_set(IMPORTED, &id, set).await?;
    }
    let why = match (&imported.id, taken, before) {
        (Some(its), true, true) => format!(" ({its} is here already, imported before)"),
        (Some(its), true, false) => format!(" (its own id, {its}, is taken here)"),
        _ => String::new(),
    };
    out.write(&format!("Imported “{}” as {id}{why}. Carry it on: {command} -r {id}\n", imported.title));
    Ok(0)
}

/// A file's text, if it's at most [`IMPORT_MAX_BYTES`].
fn read_bounded(path: &Path, command: &str) -> Result<String, String> {
    let shown = plain(&path.display().to_string());
    let size = std::fs::metadata(path).map_err(|error| format!("Kumi can't read {shown}: {error}."))?.len();
    if size > IMPORT_MAX_BYTES as u64 {
        return Err("That file is too big for a Kumi conversation (16 MB at most).".into());
    }
    let bytes = std::fs::read(path).map_err(|error| format!("Kumi can't read {shown}: {error}."))?;
    String::from_utf8(bytes).map_err(|_| format!("That file isn't a Kumi conversation. {}", importable(command)))
}
