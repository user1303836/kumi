//! Files the producer adds to a message: dragged in or pasted as paths, or a picture on the
//! clipboard (ctrl+v). Each shows as a chip above the box and goes with the next message.

use kumi_runtime::core::contracts::Attachment;
use std::path::{Path, PathBuf};

/// A file's media type, by its extension.
pub fn media_type_of(path: &Path) -> &'static str {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff",
        "bmp" => "image/bmp",
        "wav" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "mid" | "midi" => "audio/midi",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// What a chip calls a file: "PNG picture", "WAV audio", "Live Set", "preset" or "PDF file".
pub fn kind_of(attachment: &Attachment) -> String {
    let extension = Path::new(&attachment.name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_uppercase();
    match attachment.media_type.split('/').next() {
        Some("image") => format!("{extension} picture"),
        Some("audio") => format!("{extension} audio"),
        _ => match extension.as_str() {
            "ALS" => "Live Set".into(),
            "ADG" | "ADV" | "FXP" | "FXB" | "VSTPRESET" | "AUPRESET" | "NMSV" | "H2P" | "SERUMPRESET" => "preset".into(),
            "" => "file".into(),
            _ => format!("{extension} file"),
        },
    }
}

pub fn size_of(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024).max(1))
    }
}

/// The file at `path` as an attachment, when it is a file.
pub fn attachment(path: &Path) -> Option<Attachment> {
    let metadata = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some(Attachment {
        path: path.to_string_lossy().into_owned(),
        name: path.file_name()?.to_string_lossy().into_owned(),
        media_type: media_type_of(path).into(),
        bytes: metadata.len(),
    })
}

/// The files a paste names, when it names nothing else: what a terminal types for files dragged
/// into it (quoted, or with spaces escaped) or `file://` links.
pub fn pasted_files(text: &str) -> Option<Vec<PathBuf>> {
    let words = shell_words(text.trim())?;
    if words.is_empty() {
        return None;
    }
    words
        .into_iter()
        .map(|word| {
            let word = match word.strip_prefix("file://") {
                Some(rest) => percent_decoded(rest)?,
                None => word,
            };
            let path = PathBuf::from(word);
            (path.is_absolute() && path.is_file()).then_some(path)
        })
        .collect()
}

/// Words split as a shell would: quotes group, and outside Windows a backslash escapes.
fn shell_words(text: &str) -> Option<Vec<String>> {
    let mut words = vec![];
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '\\' && !cfg!(windows) => word.push(chars.next()?),
            None if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            None => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    // file:///C:/… on Windows names C:/…
    Some(if cfg!(windows) {
        decoded.strip_prefix('/').filter(|rest| rest.get(1..2) == Some(":")).unwrap_or(&decoded).to_owned()
    } else {
        decoded
    })
}

/// A picture on the clipboard, saved as a PNG file in `folder` (readable only by you); None when
/// the clipboard holds none.
pub async fn clipboard_picture(folder: &Path) -> Result<Option<PathBuf>, String> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(folder).await.map_err(|e| e.to_string())?;
    prune(folder).await;
    let file = folder.join(format!("clipboard-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")));
    let Some(bytes) = clipboard_png(&file).await? else { return Ok(None) };
    if let Some(bytes) = bytes {
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        use tokio::io::AsyncWriteExt;
        let mut handle = options.open(&file).await.map_err(|e| e.to_string())?;
        handle.write_all(&bytes).await.map_err(|e| e.to_string())?;
        handle.flush().await.map_err(|e| e.to_string())?;
    }
    Ok(tokio::fs::metadata(&file).await.is_ok_and(|m| m.len() > 0).then_some(file))
}

const PNG: &[u8] = b"\x89PNG";
/// Clipboard pictures older than this are cleared away when another is pasted.
const KEEP_DAYS: u64 = 7;

/// A clipboard program's output, given up after 10 s; it's stopped if it's still running then.
async fn bounded(program: &str, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).kill_on_drop(true).stdin(std::process::Stdio::null());
    match tokio::time::timeout(std::time::Duration::from_secs(10), command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(format!("{program} didn't start ({error})")),
        Err(_) => Err(format!("{program} took more than 10 s")),
    }
}

/// This process's environment, a name or value that isn't Unicode read as near as it can be.
pub fn lossy_env() -> kumi_runtime::system::Env {
    std::env::vars_os().map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())).collect()
}

/// Clears clipboard pictures pasted more than KEEP_DAYS ago: they can be private.
async fn prune(folder: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(folder).await else { return };
    let keep = std::time::Duration::from_secs(KEEP_DAYS * 24 * 60 * 60);
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let pasted = name.to_str().is_some_and(|name| name.starts_with("clipboard-") && name.ends_with(".png"));
        let old =
            entry.metadata().await.ok().and_then(|m| m.modified().ok()).and_then(|at| at.elapsed().ok()).is_some_and(|age| age > keep);
        if pasted && old {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// The clipboard's picture: Some(Some(bytes)) to write to `file`, Some(None) when the system wrote
/// it there itself, None when there's no picture.
#[cfg(target_os = "macos")]
async fn clipboard_png(_file: &Path) -> Result<Option<Option<Vec<u8>>>, String> {
    let output = bounded("osascript", &["-e", "the clipboard as «class PNGf»"]).await?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(hex) = text.trim().strip_prefix("«data PNGf").and_then(|rest| rest.strip_suffix('»')) else { return Ok(None) };
    let bytes: Option<Vec<u8>> =
        (0..hex.len()).step_by(2).map(|at| hex.get(at..at + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok())).collect();
    Ok(bytes.filter(|b| b.starts_with(PNG)).map(Some))
}

#[cfg(windows)]
async fn clipboard_png(file: &Path) -> Result<Option<Option<Vec<u8>>>, String> {
    let target = file.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; $p = [System.Windows.Forms.Clipboard]::GetImage(); if ($p) {{ $p.Save('{target}', [System.Drawing.Imaging.ImageFormat]::Png); 'saved' }}"
    );
    // Windows' own PowerShell, wherever PATH points.
    let env = lossy_env();
    let powershell = kumi_runtime::system::system_program(kumi_runtime::system::SystemProgram::Powershell, &env, "win32");
    let output = bounded(&powershell, &["-NoProfile", "-NonInteractive", "-STA", "-Command", &script]).await?;
    Ok(String::from_utf8_lossy(&output.stdout).contains("saved").then_some(None))
}

#[cfg(all(unix, not(target_os = "macos")))]
async fn clipboard_png(_file: &Path) -> Result<Option<Option<Vec<u8>>>, String> {
    for (program, args) in
        [("wl-paste", &["--type", "image/png"][..]), ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"][..])]
    {
        if let Ok(output) = bounded(program, args).await {
            if output.status.success() && output.stdout.starts_with(PNG) {
                return Ok(Some(Some(output.stdout)));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragged_paths_quoted_or_escaped_and_file_links_are_files() {
        let dir = tempfile::tempdir().unwrap();
        let spaced = dir.path().join("Screen Shot.png");
        let plain = dir.path().join("kick.wav");
        std::fs::write(&spaced, b"x").unwrap();
        std::fs::write(&plain, b"x").unwrap();
        let (spaced_text, plain_text) = (spaced.to_string_lossy().into_owned(), plain.to_string_lossy().into_owned());
        assert_eq!(pasted_files(&format!("'{spaced_text}' {plain_text}")), Some(vec![spaced.clone(), plain.clone()]));
        assert_eq!(pasted_files(&format!("\"{spaced_text}\"\n")), Some(vec![spaced.clone()]));
        if !cfg!(windows) {
            assert_eq!(pasted_files(&spaced_text.replace(' ', "\\ ")), Some(vec![spaced.clone()]));
            assert_eq!(pasted_files(&format!("file://{}", spaced_text.replace(' ', "%20"))), Some(vec![spaced]));
        }
        // Anything else is text to type: words, a missing file, a folder, a relative path.
        for text in
            ["make this warmer", &format!("{plain_text} please"), "/no/such/file.wav", &dir.path().to_string_lossy(), "kick.wav", ""]
        {
            assert_eq!(pasted_files(text), None, "{text}");
        }
    }

    #[test]
    fn chips_name_the_kind_and_size() {
        let chip = |name: &str, bytes: u64| {
            let path = Path::new(name);
            let attachment = Attachment { path: name.into(), name: name.into(), media_type: media_type_of(path).into(), bytes };
            format!("{} · {}", kind_of(&attachment), size_of(bytes))
        };
        assert_eq!(chip("synth.png", 1500), "PNG picture · 2 KB");
        assert_eq!(chip("ref.wav", 3 * 1024 * 1024), "WAV audio · 3.0 MB");
        assert_eq!(chip("Song.als", 10), "Live Set · 1 KB");
        assert_eq!(chip("Lead.adv", 10), "preset · 1 KB");
        assert_eq!(chip("notes.pdf", 10), "PDF file · 1 KB");
    }
}
