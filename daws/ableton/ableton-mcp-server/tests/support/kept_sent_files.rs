//! Keep replaying the recorded source for staged media once a change has gone to Live (#1151). The source deleted a
//! change's staged copy when its undo ran; the bridge now keeps every copy it sent to Live, since Live's own Undo after
//! Kumi's, or a Set saved before Kumi's undo, can bring back the clip, Simpler or pad that plays it. This projection puts
//! back, in each recorded step after the change first went to Live, the staged files the source took away; nothing else.

use serde_json::{json, Value};

/// The staged files a case's change sent to Live, as its recorded steps listed them.
#[derive(Default)]
pub struct Kept {
    sent: Vec<Value>,
}

impl Kept {
    /// A step's recorded staged files as this bridge leaves them. `record_state` is the change's state the step recorded:
    /// the first `applied` or `uncertain` one is when it went to Live, and the files listed then stay from there on.
    pub fn files(&mut self, recorded: &Value, record_state: &Value) -> Value {
        let files = recorded.as_array().cloned().unwrap_or_default();
        if self.sent.is_empty() && matches!(record_state.as_str(), Some("applied" | "uncertain")) {
            self.sent = files.clone();
        }
        self.with_sent(files)
    }

    /// A file the case itself wrote beside a sent copy (an analysis file), which stays with it.
    pub fn beside(&mut self, file: Value) {
        self.sent.push(file);
    }

    fn with_sent(&self, mut files: Vec<Value>) -> Value {
        for file in &self.sent {
            if !files.iter().any(|listed| listed["name"] == file["name"]) {
                files.push(file.clone());
            }
        }
        files.sort_by_key(|file| file["name"].as_str().unwrap_or_default().to_string());
        json!(files)
    }
}
