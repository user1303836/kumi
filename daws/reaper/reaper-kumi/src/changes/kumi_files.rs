//! Files in Kumi's own folder (its renders and captures, which Kumi clears after two hours; all of it goes with `kumi
//! uninstall`) aren't the project's to keep: a change that would make REAPER play one from there plays a copy in the
//! project's media folder instead (where REAPER keeps the project's recordings), made at apply. Any other file stays where
//! it is.

use std::path::{Path, PathBuf};

use super::{Ctx, Done};
use crate::read::failed;

/// What a change says of a file it plays from where it is.
pub const IN_PLACE: &str = "The file stays where it is: REAPER plays it from there.";
/// What it says of one in Kumi's own folder.
pub const COPIED: &str =
    "The file is in Kumi's own folder, which Kumi clears: REAPER plays a copy of it in the project's media folder (it stays there after an undo).";

/// The most copies of one name the media folder takes before Kumi gives up looking for a free one.
const MOST_NAMES: u32 = 1000;

/// Kumi's folder for a bridge whose discovery folder is this: `<Kumi's folder>/bridges/reaper/discovery`; the bridge's own
/// folder when it isn't laid out so.
pub fn kumi_folder_beside(discovery: &Path) -> Option<PathBuf> {
    let bridge = discovery.parent()?;
    match bridge.parent() {
        Some(bridges) if bridges.file_name().is_some_and(|name| name == "bridges") => bridges.parent().map(Path::to_path_buf),
        _ => Some(bridge.to_path_buf()),
    }
}

/// Whether a file is in Kumi's own folder (links followed where they can be).
pub fn is_kumis(ctx: &Ctx, path: &str) -> bool {
    let Some(folder) = ctx.reads.kumi_folder() else { return false };
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    real(Path::new(path)).starts_with(real(&folder))
}

/// What a change says of playing this file.
pub fn note(ctx: &Ctx, path: &str) -> &'static str {
    if is_kumis(ctx, path) {
        COPIED
    } else {
        IN_PLACE
    }
}

/// The file the project plays: this one, or (one in Kumi's own folder) a copy in the project's media folder, under its
/// own name or the first free "name (2)", "name (3)"…
pub fn playable(ctx: &Ctx, path: &str) -> Done<String> {
    if !is_kumis(ctx, path) {
        return Ok(path.to_string());
    }
    let folder = ctx.reaper.project_media_folder(ctx.proj).filter(|f| Path::new(f).is_absolute()).ok_or_else(|| {
        failed("the file is in Kumi's own folder, which Kumi clears, and REAPER gives no media folder for this project to copy it into: nothing was made")
    })?;
    let folder = PathBuf::from(folder);
    std::fs::create_dir_all(&folder)
        .map_err(|e| failed(format!("Kumi couldn't make the project's media folder {} ({e}): nothing was made", folder.display())))?;
    let source = Path::new(path);
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Kumi".into());
    let extension = source.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 1..=MOST_NAMES {
        let name = if n == 1 { format!("{stem}{extension}") } else { format!("{stem} ({n}){extension}") };
        let target = folder.join(name);
        // (created new or not at all: a file of that name that appears meanwhile is never written over)
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(mut out) => {
                let copied = std::fs::File::open(source).and_then(|mut input| std::io::copy(&mut input, &mut out));
                return match copied {
                    Ok(_) => Ok(target.to_string_lossy().into_owned()),
                    Err(e) => {
                        let _ = std::fs::remove_file(&target);
                        Err(failed(format!("Kumi couldn't copy the file into the project's media folder ({e}): nothing was made")))
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(failed(format!("Kumi couldn't copy the file into {} ({e}): nothing was made", folder.display())));
            }
        }
    }
    Err(failed(format!("the project's media folder has {MOST_NAMES} files named after {stem}{extension} already: nothing was made")))
}
