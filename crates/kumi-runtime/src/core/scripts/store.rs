//! Kept scripts, Kumi's own copies: a file each in Kumi's folder (one folder a DAW), named for the script: its first line
//! says what it is (as JSON: name, what it does, when it was kept, where its action is), then the chunk as it ran.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The first line's start.
const HEADER: &str = "-- kumi-script: ";
/// The most scripts kept a DAW.
pub const MOST_KEPT: usize = 200;
const MOST_NAME: usize = 80;
const MOST_DESCRIPTION: usize = 500;

/// A kept script: its name and its file's (slug), what it does, when it was kept, its action in the DAW, and its code.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Saved {
    pub name: String,
    pub slug: String,
    pub description: String,
    pub kept: String,
    pub action: Option<String>,
    pub code: String,
}

/// A name's file name: lower-case letters, digits and dashes, 48 at most ("Colour the drums" → "colour-the-drums").
pub fn slug(name: &str) -> Result<String, String> {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(48).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        return Err("name: words with letters or digits in them".into());
    }
    Ok(out)
}

/// A name or what a script does, checked: one line, not too long.
pub fn line(text: &str, key: &str, required: bool) -> Result<String, String> {
    let most = if key == "name" { MOST_NAME } else { MOST_DESCRIPTION };
    let text = text.trim();
    if (required && text.is_empty()) || text.chars().count() > most || text.chars().any(char::is_control) {
        return Err(format!("{key}: one line of at most {most} characters"));
    }
    Ok(text.to_string())
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    fn path(&self, slug: &str) -> PathBuf {
        self.dir.join(format!("{slug}.lua"))
    }

    /// Every kept script, by name.
    pub fn list(&self) -> Vec<Saved> {
        let mut out: Vec<Saved> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "lua"))
            .filter_map(|path| read(&path))
            .take(MOST_KEPT)
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    /// The kept script of this name (or slug).
    pub fn get(&self, name: &str) -> Option<Saved> {
        let slug = slug(name).ok()?;
        read(&self.path(&slug))
    }

    /// Writes it (over one of its name only when `replace`): its file.
    pub fn save(&self, saved: &Saved, replace: bool) -> Result<PathBuf, String> {
        let path = self.path(&saved.slug);
        if path.exists() && !replace {
            return Err(format!("a script named \u{201c}{}\u{201d} is kept already: replace: true writes over it", saved.name));
        }
        if !path.exists() && self.list().len() >= MOST_KEPT {
            return Err(format!("{MOST_KEPT} scripts are kept already: remove one first"));
        }
        std::fs::create_dir_all(&self.dir).map_err(|error| format!("couldn't make {}: {error}", self.dir.display()))?;
        let header = json!({"name": saved.name, "description": saved.description, "kept": saved.kept, "action": saved.action});
        let text = format!("{HEADER}{header}\n{}\n", saved.code);
        std::fs::write(&path, text).map_err(|error| format!("couldn't write {}: {error}", path.display()))?;
        Ok(path)
    }

    pub fn remove(&self, slug: &str) -> Result<(), String> {
        let path = self.path(slug);
        std::fs::remove_file(&path).map_err(|error| format!("couldn't delete {}: {error}", path.display()))
    }
}

fn read(path: &Path) -> Option<Saved> {
    let text = std::fs::read_to_string(path).ok()?;
    let (first, code) = text.split_once('\n')?;
    let header: Value = serde_json::from_str(first.strip_prefix(HEADER)?).ok()?;
    let text = |key: &str| header.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    Some(Saved {
        name: text("name"),
        slug: path.file_stem()?.to_string_lossy().into_owned(),
        description: text("description"),
        kept: text("kept"),
        action: header.get("action").and_then(Value::as_str).map(str::to_owned),
        code: code.strip_suffix('\n').unwrap_or(code).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_is_kept_by_its_names_slug_and_read_back_whole() {
        assert_eq!(slug("Colour the drums!").unwrap(), "colour-the-drums");
        assert_eq!(slug("  Hats: tidy  ").unwrap(), "hats-tidy");
        assert!(slug("!!!").is_err());
        assert_eq!(slug(&"a".repeat(100)).unwrap().len(), 48);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("reaper"));
        assert!(store.list().is_empty(), "no folder yet: nothing kept");
        let saved = Saved {
            name: "Colour the drums".into(),
            slug: "colour-the-drums".into(),
            description: "Drum tracks red".into(),
            kept: "2026-10-22".into(),
            action: Some("_RSabc".into()),
            code: "local n = 0\nreturn n\n".into(),
        };
        store.save(&saved, false).unwrap();
        assert_eq!(store.get("Colour the drums"), Some(saved.clone()));
        assert_eq!(store.list(), vec![saved.clone()]);
        assert!(store.save(&saved, false).unwrap_err().contains("kept already"));
        store.save(&saved, true).unwrap();
        store.remove("colour-the-drums").unwrap();
        assert!(store.get("colour-the-drums").is_none());
        assert!(line("two\nlines", "name", true).is_err());
        assert!(line("", "description", false).is_ok());
    }
}
