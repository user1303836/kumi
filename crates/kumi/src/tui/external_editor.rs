//! The producer's own editor for the draft (ctrl+g): which one, and running it on a file while the terminal is lent to it.

use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// The editor to run: VISUAL, then EDITOR, as the shell's own tools choose; none when neither is set. Not Notepad on
/// Windows unasked: Windows 11's returns at once when a window of it is open, and the draft would come back unchanged.
pub fn editor_command(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["VISUAL", "EDITOR"].into_iter().filter_map(get).map(|command| command.trim().to_string()).find(|command| !command.is_empty())
}

/// A command line's words the way Windows splits them: at spaces, but not inside double quotes, which go.
pub fn words(command: &str) -> Vec<String> {
    let (mut words, mut word, mut quoted, mut started) = (Vec::new(), String::new(), false, false);
    for character in command.chars() {
        match character {
            '"' => (quoted, started) = (!quoted, true),
            character if character.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            character => {
                word.push(character);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// Where Windows' shell finds `program`: in the first PATH folder that has it with one of PATHEXT's extensions (`code`
/// is `code.cmd`). A program named with a folder or an extension, or found nowhere, is run as named.
pub fn resolve(program: &str, path: &str, extensions: &str) -> PathBuf {
    let named = Path::new(program);
    if named.extension().is_some() || program.contains(['/', '\\']) {
        return named.to_path_buf();
    }
    path.split(';')
        .filter(|folder| !folder.is_empty())
        .flat_map(|folder| {
            extensions
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(move |extension| Path::new(folder).join(format!("{program}{}", extension.to_ascii_lowercase())))
        })
        .find(|file| file.is_file())
        .unwrap_or_else(|| named.to_path_buf())
}

/// Runs the editor on `file` and waits for it to close. Elsewhere it runs through the shell, as git runs it, so EDITOR
/// may carry arguments (`code --wait`). On Windows its first word is found through PATHEXT.
pub async fn run(command: &str, file: &Path) -> std::io::Result<ExitStatus> {
    #[cfg(windows)]
    let mut editor = {
        let mut words = words(command);
        if words.is_empty() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no editor"));
        }
        let get = |name: &str| std::env::var(name).unwrap_or_default();
        let extensions = Some(get("PATHEXT")).filter(|e| !e.is_empty()).unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        let mut editor = tokio::process::Command::new(resolve(&words.remove(0), &get("PATH"), &extensions));
        editor.args(words);
        editor
    };
    #[cfg(not(windows))]
    let mut editor = {
        let mut editor = tokio::process::Command::new("sh");
        editor.arg("-c").arg(format!("{command} \"$@\"")).arg(command);
        editor
    };
    editor.arg(file).status().await
}
