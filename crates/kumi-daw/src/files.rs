//! Files as a DAW's steps and the app's report read them: a file's text, a folder's names, when a file changed, a
//! long line clipped, and a file or folder removed.
use kumi_common::js::string::utf16_len;
use std::{fs, time::UNIX_EPOCH};

fn head(text: &str, most: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(most).collect::<Vec<_>>())
}

/// `text`, its first `most` characters (UTF-16, as the report has always counted) and how many more there were.
pub fn clip(text: &str, most: usize) -> String {
    let len = utf16_len(text);
    if len > most {
        format!("{}… ({} more characters)", head(text, most), len - most)
    } else {
        text.into()
    }
}

/// A file's text (lossy), when it can be read.
pub async fn read(file: &str) -> Option<String> {
    tokio::fs::read(file).await.ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// The names in a folder, sorted; none when it can't be read.
pub async fn names(folder: &str) -> Vec<String> {
    let Ok(mut reader) = tokio::fs::read_dir(folder).await else { return vec![] };
    let mut result = vec![];
    while let Ok(Some(entry)) = reader.next_entry().await {
        result.push(entry.file_name().to_string_lossy().into_owned())
    }
    result.sort();
    result
}

/// When a file last changed (ms since 1970).
pub async fn modified(file: &str) -> Option<f64> {
    let time = tokio::fs::metadata(file).await.ok()?.modified().ok()?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(time) => Some(time.as_secs_f64() * 1000.),
        Err(error) => Some(-error.duration().as_secs_f64() * 1000.),
    }
}

/// Removes a file, or a folder with what's in it; one that isn't there is fine.
pub fn remove(path: &str) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
