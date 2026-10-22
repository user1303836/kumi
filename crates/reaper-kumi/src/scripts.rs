//! Scripts kept as REAPER actions (#401, a repeated chore): a chunk Kumi wrote and ran once, kept once the producer said yes,
//! in REAPER's Scripts folder under Kumi/, wrapped to run as one step of REAPER's undo, and registered in the Action List (and
//! REAPER's key map, so it stays) for a shortcut or a toolbar button. Removing one unregisters it and deletes its file.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::queue::Answer;
use crate::read::{bad, failed};
use crate::reaper::Reaper;

/// The most of a kept script's code (bytes), and of its name and what it does (characters).
const MOST_CODE: usize = 200_000;
const MOST_NAME: usize = 80;
const MOST_DESCRIPTION: usize = 500;

/// Its file's name: a-z, 0-9 and -, from Kumi.
fn slug_of(params: &Value) -> Result<String, (String, String)> {
    let slug = params.get("slug").and_then(Value::as_str).unwrap_or_default();
    if slug.is_empty() || slug.len() > 64 || !slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(bad("slug: a-z, 0-9 and -, 1 to 64 of them"));
    }
    Ok(slug.to_string())
}

/// Text of one line (a name, what it does), at most `most` characters.
fn line_of(params: &Value, key: &str, most: usize, required: bool) -> Result<String, (String, String)> {
    let text = params.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    if (required && text.is_empty()) || text.chars().count() > most || text.chars().any(char::is_control) {
        return Err(bad(format!("{key}: one line of at most {most} characters")));
    }
    Ok(text)
}

/// Kumi's folder of kept scripts: Scripts/Kumi in REAPER's resource folder.
fn folder(reaper: &Reaper) -> Result<PathBuf, (String, String)> {
    let resource = reaper.resource_path();
    if resource.is_empty() {
        return Err(failed("REAPER didn't say where its resource folder is"));
    }
    Ok(PathBuf::from(resource).join("Scripts").join("Kumi"))
}

/// A Lua string literal of any text.
fn lua_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push_str(&format!("\\{}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The longest a kept chore runs (seconds): REAPER waits for an action, its window frozen.
const LIMIT_S: u32 = 10;

/// The file REAPER runs: the chunk as a function, called inside one undo block named for it, the screen held still while it
/// runs, stopped past its time where REAPER's Lua has a debug hook, what it prints and an error on REAPER's console.
pub fn wrapped(name: &str, description: &str, code: &str) -> String {
    let about = if description.is_empty() { String::new() } else { format!("-- {description}\n") };
    format!(
        r##"-- Kumi: {name}
{about}-- Kept by Kumi (its scripts tool removes it). It runs as one step of REAPER's undo.
local function print(...)
  local parts = {{}}
  for i = 1, select("#", ...) do parts[i] = tostring((select(i, ...))) end
  reaper.ShowConsoleMsg(table.concat(parts, "\t") .. "\n")
end
local function chore()
{code}
end
reaper.Undo_BeginBlock()
reaper.PreventUIRefresh(1)
local hooked = debug and debug.sethook
if hooked then
  local deadline = reaper.time_precise() + {LIMIT_S}
  debug.sethook(function()
    if reaper.time_precise() > deadline then error("Kumi stopped it: it ran longer than {LIMIT_S} s", 2) end
  end, "", 1000)
end
local ok, err = pcall(chore)
if hooked then debug.sethook() end
reaper.PreventUIRefresh(-1)
reaper.UpdateArrange()
reaper.Undo_EndBlock({title}, -1)
if not ok then reaper.ShowConsoleMsg({prefix} .. tostring(err) .. "\n") end
"##,
        title = lua_string(name),
        prefix = lua_string(&format!("{name}: "))
    )
}

/// `script_install {slug, name, description?, code, replace?}`: the script written and registered as an action:
/// {file, command (its named command, "_RS…"), action (its name in the Action List), where (both, in words)}.
pub fn install(reaper: &Reaper, params: &Value) -> Answer {
    let slug = slug_of(params)?;
    let name = line_of(params, "name", MOST_NAME, true)?;
    let description = line_of(params, "description", MOST_DESCRIPTION, false)?;
    let code = params.get("code").and_then(Value::as_str).unwrap_or_default();
    if code.trim().is_empty() || code.len() > MOST_CODE {
        return Err(bad(format!("code: the Lua to keep, at most {MOST_CODE} bytes")));
    }
    let dir = folder(reaper)?;
    std::fs::create_dir_all(&dir).map_err(|error| failed(format!("couldn't make {}: {error}", dir.display())))?;
    let path = dir.join(format!("{slug}.lua"));
    if path.exists() && params.get("replace") != Some(&Value::Bool(true)) {
        return Err(failed(format!("a script is kept as {} already: replace: true writes over it", path.display())));
    }
    std::fs::write(&path, wrapped(&name, &description, code))
        .map_err(|error| failed(format!("couldn't write {}: {error}", path.display())))?;
    let Some(command) = reaper.add_script(&path, true) else {
        let _ = std::fs::remove_file(&path);
        return Err(failed("REAPER wouldn't take the script as an action: nothing was kept"));
    };
    let action = reaper.action_name(command).unwrap_or_else(|| format!("Script: {slug}.lua"));
    let named = reaper.named_command(command).map(|named| format!("_{named}"));
    let place = match &named {
        Some(named) => {
            format!("\u{201c}{action}\u{201d} in REAPER's Action List (its command {named}), for a shortcut or a toolbar button")
        }
        None => format!("\u{201c}{action}\u{201d} in REAPER's Action List, for a shortcut or a toolbar button"),
    };
    Ok(json!({"file": path, "command": named, "action": action, "where": place}))
}

/// `script_remove {slug}`: the script's action unregistered and its file deleted.
pub fn remove(reaper: &Reaper, params: &Value) -> Answer {
    let slug = slug_of(params)?;
    let path = folder(reaper)?.join(format!("{slug}.lua"));
    if !path.exists() {
        return Err(("gone".into(), format!("no script is kept as {}", path.display())));
    }
    reaper.remove_script(&path, true);
    std::fs::remove_file(&path).map_err(|error| failed(format!("couldn't delete {}: {error}", path.display())))?;
    Ok(json!({"removed": path}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reaper::fake;

    #[test]
    fn a_kept_script_is_one_undo_step_registered_as_an_action_and_removed_whole() {
        fake::lab_project();
        let dir = tempfile::tempdir().unwrap();
        fake::with(|s| s.resource = dir.path().to_string_lossy().into_owned());
        let api = fake::api();
        let reaper = Reaper::new(&api);
        let asked = json!({"slug": "color-drums", "name": "Colour the drums", "description": "Drums red, by name", "code": "return 1"});
        let kept = install(&reaper, &asked).unwrap();
        let file = dir.path().join("Scripts/Kumi/color-drums.lua");
        assert_eq!(kept["file"], json!(file));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("-- Kumi: Colour the drums\n-- Drums red, by name\n"), "{text}");
        assert!(text.contains("local function chore()\nreturn 1\nend") && text.contains("reaper.Undo_EndBlock(\"Colour the drums\", -1)"));
        assert!(
            text.contains("local function print(...)") && text.contains("reaper.ShowConsoleMsg(table.concat(parts, \"\\t\") .. \"\\n\")"),
            "{text}"
        );
        assert!(text.contains("error(\"Kumi stopped it: it ran longer than 10 s\", 2)"), "{text}");
        assert_eq!(fake::with(|s| (s.scripts.len(), s.kb_writes)), (1, 1), "registered, in REAPER's key map");
        assert!(kept["command"].as_str().unwrap().starts_with("_RS"), "{kept}");
        assert!(kept["where"].as_str().unwrap().contains("in REAPER's Action List (its command _RS"), "{kept}");
        // Kept already: written over only when asked.
        assert_eq!(install(&reaper, &asked).unwrap_err().0, "failed");
        let mut again = asked.clone();
        again["replace"] = true.into();
        install(&reaper, &again).unwrap();
        // Removed: unregistered and deleted; a second time, gone.
        remove(&reaper, &json!({"slug": "color-drums"})).unwrap();
        assert!(!file.exists());
        assert!(fake::with(|s| s.scripts.is_empty()));
        assert_eq!(remove(&reaper, &json!({"slug": "color-drums"})).unwrap_err().0, "gone");
        for wrong in [
            json!({"slug": "../x", "name": "x", "code": "x"}),
            json!({"slug": "x", "name": "two\nlines", "code": "x"}),
            json!({"slug": "x", "name": "x", "code": " "}),
        ] {
            assert_eq!(install(&reaper, &wrong).unwrap_err().0, "bad-params", "{wrong}");
        }
    }

    #[test]
    fn a_name_is_a_lua_string_whatever_it_holds() {
        assert_eq!(lua_string("say \"hi\" \\ now"), "\"say \\\"hi\\\" \\\\ now\"");
    }
}
