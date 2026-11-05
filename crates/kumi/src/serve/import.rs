//! The import step (CC-11): a file step's field holds a source, which the runtime resolves to one checked audio file
//! with W3's resolvers, never a path a plan gave.
//! - A sound from the map, of its current revision (else `stale`): the producer's own library holds it.
//! - A file Kumi made and keeps (an artifact): the artifacts are kept for good (11a).
//! - A file in an approved folder, with the host tool that made it: copied into the artifacts first and imported from
//!   there (11a), since the host may clean the folder it came from. Its provenance is recorded, never a grant.
//!
//! The resolved file's identity is pinned (11b): the preview's and the dispatch's must be the same file, unchanged.
//! Only its name reaches the preview and HISTORY (11c); the path goes to the DAW alone.

use super::artifacts::StagedArtifact;
use super::ctx::{Ctx, OpError};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The audio files a DAW's file step takes, by extension.
const AUDIO: [(&str, &str); 7] = [
    ("wav", "audio/wav"),
    ("wave", "audio/wav"),
    ("aif", "audio/aiff"),
    ("aiff", "audio/aiff"),
    ("flac", "audio/flac"),
    ("mp3", "audio/mpeg"),
    ("ogg", "audio/ogg"),
];

/// A source resolved to the file it names.
#[derive(Debug, Clone)]
pub struct Found {
    path: PathBuf,
    /// The file's name, as the preview and HISTORY say it.
    pub name: String,
    /// The source, as HISTORY keeps it.
    record: Value,
    mime: &'static str,
    /// The identity of the file the source named: the preview's and the dispatch's must match (11b).
    pub named: Value,
}

impl Found {
    /// Where the file is: for an operation that stages the file itself (a generation's upload, through
    /// `Artifacts::stage` with [`Found::named`]), never for a path given to anyone.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// `source` resolved to the one checked audio file it names, changing nothing.
pub async fn find(ctx: &Ctx, source: &Value, signal: &Signal) -> Result<Found, OpError> {
    let text = |name: &str| source.get(name).and_then(Value::as_str);
    let (found, record) = if let (Some(id), Some(revision)) = (text("sound_id"), text("map_revision")) {
        (ctx.maps().resolve(ctx, id, revision, signal).await?, json!({"sound_id": id, "map_revision": revision}))
    } else if let Some(id) = text("artifact_id") {
        (ctx.artifacts().resolve(ctx, id)?, json!({"artifact_id": id}))
    } else if let Some(file) = text("file") {
        let host_tool = source.pointer("/provenance/host_tool").and_then(Value::as_str).unwrap_or_default();
        let mut record = json!({"host_tool": host_tool});
        if let Some(call) = source.pointer("/provenance/tool_call_id").and_then(Value::as_str) {
            record["host_tool_call_id"] = json!(call);
        }
        (ctx.approved(Path::new(file))?, record)
    } else {
        return Err(OpError::Invalid { reason: "A source is {sound_id, map_revision}, {artifact_id} or {file, provenance}.".into() });
    };
    // Neither a map nor an artifact is itself a grant: the file must be inside an approved root now.
    let path = ctx.approved(&found)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = path.extension().map(|extension| extension.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let Some((_, mime)) = AUDIO.iter().find(|(known, _)| *known == extension) else {
        let reason = format!("{} isn't an audio file a DAW takes here (wav, aif, flac, mp3 or ogg).", super::change::one_line(&name, 120));
        return Err(OpError::Refused { code: "not_allowed", reason, owner: None });
    };
    let named = identity(&path)?;
    Ok(Found { path, name, record, mime, named })
}

/// A host's file for `found` copied into the artifacts' private staging (11a), not yet kept: only the one the approval
/// pinned, read through one opened handle, so another put in its place since is refused stale (11b). None for a map
/// sound or an artifact, which the DAW gets as they are. A staged copy that's never kept goes with it.
pub async fn stage(ctx: &Ctx, found: &Found, signal: &Signal) -> Result<Option<StagedArtifact>, OpError> {
    if found.record.get("host_tool").is_none() {
        return Ok(None);
    }
    Ok(Some(ctx.artifacts().stage(ctx, &found.path, Some(&found.named), signal).await?))
}

/// The file the DAW gets for `found` ([`stage`]'s copy kept in the artifacts, or the file as it is), with the source
/// as HISTORY keeps it and the identity of the file the DAW gets.
pub async fn keep(ctx: &Ctx, found: Found, staged: Option<StagedArtifact>, signal: &Signal) -> Result<(PathBuf, Value), OpError> {
    let mut record = found.record;
    let (path, kept) = match staged {
        Some(staged) => {
            let saved = ctx.artifacts().publish(ctx, staged, "audio", found.mime, "change", signal).await?;
            record["artifact_id"] = json!(saved.id);
            let kept = identity(&saved.path)?;
            (saved.path, kept)
        }
        // The file as the dispatch checked it against the approval.
        None => (found.path, found.named),
    };
    record["file"] = json!(super::change::one_line(&found.name, 120));
    record["identity"] = kept;
    Ok((plain(path), record))
}

/// `path` as a DAW opens it: on Windows without the `\\?\` verbatim prefix canonicalizing gives (`\\?\C:\…` as `C:\…`,
/// `\\?\UNC\server\share\…` as `\\server\share\…`), which some DAWs don't take; elsewhere as it is.
fn plain(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        if let Some(Component::Prefix(prefix)) = components.next() {
            let rest = components.as_path();
            match prefix.kind() {
                Prefix::VerbatimDisk(letter) => return PathBuf::from(format!("{}:\\", letter as char)).join(rest),
                Prefix::VerbatimUNC(server, share) => {
                    let root = format!("\\\\{}\\{}\\", server.to_string_lossy(), share.to_string_lossy());
                    return PathBuf::from(root).join(rest);
                }
                _ => {}
            }
        }
    }
    path
}

/// The identity of the file at `path`, as the artifacts' copy checks it ([`super::artifacts::file_identity`]: its
/// volume and file, size and last write, read from the open file).
fn identity(path: &Path) -> Result<Value, OpError> {
    let unreadable = |error: std::io::Error| OpError::Failed { reason: format!("Kumi couldn't read the file: {error}") };
    let file = std::fs::File::open(path).map_err(unreadable)?;
    super::artifacts::file_identity(&file).map_err(unreadable)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_daw_gets_a_windows_path_without_the_verbatim_prefix() {
        assert_eq!(plain(PathBuf::from(r"\\?\C:\Samples\kick.wav")), PathBuf::from(r"C:\Samples\kick.wav"));
        assert_eq!(plain(PathBuf::from(r"\\?\UNC\studio\share\kick.wav")), PathBuf::from(r"\\studio\share\kick.wav"));
        assert_eq!(plain(PathBuf::from(r"D:\Samples\kick.wav")), PathBuf::from(r"D:\Samples\kick.wav"));
    }
}
