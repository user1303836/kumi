//! `kumi design`: one sound-design render, headless, for a host that has already resolved and approved its sources
//! (Kumi for Hermes's Desktop runs it). It reads the request, decodes the files it names, renders, writes the WAV where
//! it's told and answers in JSON on stdout. No DAW, no session, no settings, no network.

use kumi_design::{fresh_seed, load_source, render, Answer, Request, MAX_SOURCES};
use kumi_media::audio::DecodeOptions;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const USAGE: &str = "usage: kumi design --json <request | -> --out <file.wav> [--ffmpeg <path>]
  request: {\"op\": \"cross\" | \"fuse\" | \"stack\", \"mode\": ..., \"sources\": [{\"file\": \"/abs/path\", \"name\": \"...\"}, ...],
            \"seed\": 0, \"options\": {...}}
  The answer (one JSON line on stdout): {op, mode, seed, sources, duration, sample_rate, channels, peaks, recipe};
  a refusal: {\"error\": \"...\"} and exit 1.";

/// The most request text read: far past 16 sources' paths and options.
const MOST_REQUEST: u64 = 256 * 1024;

struct Args {
    request: String,
    out: PathBuf,
    ffmpeg: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let (mut request, mut out, mut ffmpeg) = (None, None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |flag: &str| rest.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--json" => request = Some(value("--json")?),
            "--out" => out = Some(absolute("--out", &value("--out")?)?),
            "--ffmpeg" => ffmpeg = Some(absolute("--ffmpeg", &value("--ffmpeg")?)?),
            other => return Err(format!("unknown argument \"{other}\"")),
        }
    }
    let request = request.ok_or("--json is needed")?;
    let request = if request == "-" {
        let mut text = String::new();
        std::io::stdin().take(MOST_REQUEST + 1).read_to_string(&mut text).map_err(|e| format!("couldn't read the request: {e}"))?;
        text
    } else {
        request
    };
    if request.len() as u64 > MOST_REQUEST {
        return Err("the request is too long".into());
    }
    let out = out.ok_or("--out is needed")?;
    if out.extension().is_none_or(|e| !e.eq_ignore_ascii_case("wav")) {
        return Err("--out names a .wav file".into());
    }
    Ok(Args { request, out, ffmpeg })
}

fn absolute(flag: &str, path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("{flag} is an absolute path"))
    }
}

/// The request's files, each one an absolute path to a regular file.
fn files(request: &Request) -> Result<Vec<(PathBuf, String)>, String> {
    request
        .sources
        .iter()
        .map(|source| {
            if source.sound_id.is_some() || source.artifact_id.is_some() || source.map_revision.is_some() {
                return Err("kumi design reads files: its caller resolves sound and artifact ids".to_string());
            }
            let file = source.file.as_deref().ok_or("each source is a file")?;
            let path = absolute("a source's file", file)?;
            if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                return Err(format!("there's no file at {}", path.display()));
            }
            let name =
                source.name.clone().unwrap_or_else(|| path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()));
            Ok((path, name))
        })
        .collect()
}

async fn design(args: Args) -> Result<Answer, String> {
    let mut request: Request = serde_json::from_str(&args.request).map_err(|e| format!("the request isn't one: {e}"))?;
    request.check().map_err(|e| e.0)?;
    if request.sources.len() > MAX_SOURCES {
        return Err("too many sources".into());
    }
    let files = files(&request)?;
    let decode = DecodeOptions { installed_only: true, ffmpeg: args.ffmpeg };
    let mut sources = Vec::with_capacity(files.len());
    for (path, name) in &files {
        sources.push(load_source(path, name, &decode, None).await.map_err(|e| format!("{name}: {}", e.0))?);
    }
    let seed = *request.seed.get_or_insert_with(fresh_seed);
    let rendered = render(&request, seed, &sources, &mut |_| {}).map_err(|e| e.0)?;
    write_atomically(&args.out, &rendered.wav())?;
    Ok(Answer::new(&request, seed, &rendered, None))
}

/// Writes beside the destination, then renames over it: a reader never sees half a WAV.
fn write_atomically(out: &Path, bytes: &[u8]) -> Result<(), String> {
    let folder = out.parent().filter(|p| p.is_dir()).ok_or_else(|| format!("there's no folder for {}", out.display()))?;
    let mut file = tempfile::Builder::new().prefix(".kumi-design-").suffix(".wav").tempfile_in(folder).map_err(|e| e.to_string())?;
    file.write_all(bytes).and_then(|()| file.as_file().sync_all()).map_err(|e| format!("couldn't write the sound: {e}"))?;
    file.persist(out).map_err(|e| format!("couldn't keep the sound: {}", e.error))?;
    Ok(())
}

fn say(out: &mut impl Write, value: &serde_json::Value) {
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

/// The arguments after `design`; the exit code.
pub fn main(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let mut stdout = std::io::stdout();
    let args = match parse(args) {
        Ok(args) => args,
        Err(why) => {
            eprintln!("kumi design: {why}\n{USAGE}");
            say(&mut stdout, &serde_json::json!({"error": why}));
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("kumi design: no runtime: {error}");
            return 1;
        }
    };
    match runtime.block_on(design(args)) {
        Ok(answer) => {
            say(&mut stdout, &serde_json::to_value(&answer).unwrap_or_default());
            0
        }
        Err(why) => {
            say(&mut stdout, &serde_json::json!({"error": why}));
            1
        }
    }
}
