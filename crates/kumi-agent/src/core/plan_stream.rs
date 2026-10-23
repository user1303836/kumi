//! Deliver complete top-level plan steps while the model is still writing the input.
use serde_json::Value;
pub struct StepScanner<F: FnMut(Value)> {
    on_step: F,
    text: String,
    at: usize,
    depth: i64,
    in_string: bool,
    escaped: bool,
    string_start: usize,
    key: Option<String>,
    in_steps: bool,
    step_start: Option<usize>,
    broken: bool,
}
pub fn step_scanner<F: FnMut(Value)>(on_step: F) -> StepScanner<F> {
    StepScanner {
        on_step,
        text: String::new(),
        at: 0,
        depth: 0,
        in_string: false,
        escaped: false,
        string_start: 0,
        key: None,
        in_steps: false,
        step_start: None,
        broken: false,
    }
}
impl<F: FnMut(Value)> StepScanner<F> {
    pub fn push(&mut self, delta: &str) {
        if self.broken {
            return;
        }
        self.text.push_str(delta);
        while self.at < self.text.len() {
            let char = self.text.as_bytes()[self.at];
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if char == b'\\' {
                    self.escaped = true;
                } else if char == b'"' {
                    self.in_string = false;
                    if self.depth == 1 {
                        match serde_json::from_str::<String>(&self.text[self.string_start..=self.at]) {
                            Ok(key) => self.key = Some(key),
                            Err(_) => {
                                self.broken = true;
                                return;
                            }
                        }
                    }
                }
                self.at += 1;
                continue;
            }
            if char == b'"' {
                self.in_string = true;
                self.string_start = self.at;
                self.at += 1;
                continue;
            }
            if char == b'{' || char == b'[' {
                if self.depth == 1 && char == b'[' && self.key.as_deref() == Some("steps") {
                    self.in_steps = true;
                } else if self.in_steps && self.depth == 2 && char == b'{' {
                    self.step_start = Some(self.at);
                }
                self.depth += 1;
            } else if char == b'}' || char == b']' {
                self.depth -= 1;
                if self.depth < 0 {
                    self.broken = true;
                    return;
                }
                if self.in_steps && self.depth == 2 && self.step_start.is_some() && char == b'}' {
                    let step = match serde_json::from_str(&self.text[self.step_start.unwrap()..=self.at]) {
                        Ok(step) => step,
                        Err(_) => {
                            self.broken = true;
                            return;
                        }
                    };
                    self.step_start = None;
                    (self.on_step)(step);
                } else if self.in_steps && self.depth == 1 {
                    self.in_steps = false;
                }
            }
            self.at += 1;
        }
        if self.step_start.is_none() && !self.in_string {
            self.text.clear();
            self.at = 0;
        }
    }
}
