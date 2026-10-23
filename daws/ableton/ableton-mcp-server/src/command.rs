//! Shared output for native delivery commands and their compatibility binaries.
use crate::live::LiveError;
use serde::Serialize;
use std::{
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}
impl CommandOutput {
    pub fn error(message: impl AsRef<str>, code: i32) -> Self {
        Self { stderr: format!("{}\n", message.as_ref()), code, ..Self::default() }
    }
    pub fn json(value: &impl Serialize) -> Self {
        Self {
            stdout: format!("{}\n", kumi_common::js::json::stringify(&serde_json::to_value(value).expect("serializable command result"))),
            ..Self::default()
        }
    }
    pub fn emit(self) -> i32 {
        let _ = std::io::stdout().write_all(self.stdout.as_bytes());
        let _ = std::io::stderr().write_all(self.stderr.as_bytes());
        self.code
    }
    pub(crate) fn report(&mut self, message: impl AsRef<str>) {
        self.stderr.push_str(message.as_ref());
        self.stderr.push('\n');
        self.code = 2;
    }
}
pub(crate) fn resolve(path: impl AsRef<Path>) -> Result<PathBuf, LiveError> {
    let path = path.as_ref();
    let absolute =
        if path.is_absolute() { path.to_owned() } else { std::env::current_dir().map_err(|e| LiveError::error(e.to_string()))?.join(path) };
    let mut result = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            part => result.push(part.as_os_str()),
        }
    }
    Ok(result)
}
pub(crate) fn value<'a>(args: &'a [String], option: &str) -> Option<&'a str> {
    args.iter().position(|v| v == option).and_then(|i| args.get(i + 1)).map(String::as_str)
}
pub(crate) fn number(value: Option<&str>) -> f64 {
    value.and_then(kumi_common::js::number::parse).unwrap_or(f64::NAN)
}
pub(crate) fn validate(args: &[String], prefix: &str, values: &[&str], flags: &[&str]) -> CommandOutput {
    let mut out = CommandOutput::default();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if values.contains(&arg.as_str()) {
            if args.get(i + 1).is_none_or(|value| value.is_empty() || value.starts_with('-')) {
                out.report(format!("{prefix}: {arg} requires a value"));
            }
            i += 2;
        } else {
            if !flags.contains(&arg.as_str()) {
                out.report(format!("{prefix}: unknown option {arg}"));
            }
            i += 1;
        }
    }
    for option in values {
        if args.iter().filter(|arg| arg.as_str() == *option).count() > 1 {
            out.report(format!("{prefix}: repeated {option}"));
        }
    }
    out
}
