//! What make_device says beside a device it made. A device is made whenever its file can be written, the way Max opens
//! any patcher: what needs fixing in it comes back first, each with its line where it has one, then what Kumi made
//! differently from what was asked so it could make it, then what's worth a look. A rule's level in Kumi's
//! standard decides which of these a broken rule is, never whether the device is made.

use super::patch::standard::Level;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Findings {
    /// What needs fixing: what Max will show as wrong, what the device lacks (a line Kumi couldn't read and left out, a
    /// cord to a port that isn't there), and what Kumi's checks and the device's tests found.
    pub wrong: Vec<String>,
    /// What Kumi made differently from what was asked: the nearest valid value, a name made unique.
    pub changed: Vec<String>,
    /// What's worth a look, though nothing's broken.
    pub notes: Vec<String>,
}

impl Findings {
    pub fn is_empty(&self) -> bool {
        self.wrong.is_empty() && self.changed.is_empty() && self.notes.is_empty()
    }

    pub fn append(&mut self, other: Findings) {
        self.wrong.extend(other.wrong);
        self.changed.extend(other.changed);
        self.notes.extend(other.notes);
    }

    /// A broken rule of Kumi's standard, where its level puts it: an error needs fixing, a warning is worth a look, and
    /// advice is kept for when a patcher is looked over.
    pub fn rule(&mut self, level: Level, says: String) {
        match level {
            Level::Error => self.wrong.push(says),
            Level::Warn => self.notes.push(says),
            Level::Advice | Level::Off => {}
        }
    }

    /// Everything said, what's wrong first.
    pub fn all(&self) -> impl Iterator<Item = &String> {
        self.wrong.iter().chain(&self.changed).chain(&self.notes)
    }

    /// Each list in the order of the lines it speaks of ("line 4: …"), then what's said of no one line.
    pub fn by_line(&mut self) {
        let line =
            |text: &String| text.strip_prefix("line ").and_then(|rest| rest.split(':').next()?.parse::<usize>().ok()).unwrap_or(usize::MAX);
        for said in [&mut self.wrong, &mut self.changed, &mut self.notes] {
            said.sort_by_key(line);
        }
    }
}
