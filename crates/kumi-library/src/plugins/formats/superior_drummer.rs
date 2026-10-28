//! Superior Drummer 3's text presets and the same text inside VST3 component
//! state. Edits replace only an existing record's values; all other text and
//! the component's opaque trailing data survive unchanged.

use std::collections::BTreeMap;
use std::ops::Range;

use serde_json::Value;

use super::{tree::Node, vstpreset, FormatError, MAX_DECODED_BYTES};

const MAX_PRESET_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOKENS: usize = 65_536;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    text: String,
}

#[derive(Debug)]
struct Record {
    path: String,
    values: Range<usize>,
    tokens: Vec<(String, bool)>,
}

impl Preset {
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() > MAX_PRESET_BYTES.min(MAX_DECODED_BYTES) {
            return Err(error("document is too large"));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| error("document is not UTF-8"))?.to_string();
        if text.contains('\0') {
            return Err(error("NUL in text document"));
        }
        let result = Self { text };
        let records = result.records()?;
        if records.iter().filter(|record| record.path.starts_with("PlugVersion[") && !record.path.contains('/')).count() > 1 {
            return Err(error("ambiguous serialized plug-in version"));
        }
        Ok(result)
    }

    /// An initialized sampler with no embedded samples. The installed plug-in
    /// resolves its default library when it loads this preset.
    pub fn create(version: &str) -> Result<Self, FormatError> {
        if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(error("invalid version"));
        }
        Self::read(format!("PlugVersion {version}\nSAMPLER {{\nHQ {{\nmastervol 1\nmastervol2 1\n}}\n}}\n").as_bytes())
    }

    pub fn version(&self) -> Option<String> {
        self.records()
            .ok()?
            .into_iter()
            .find(|record| record.path == "PlugVersion[0]" && record.tokens.len() == 1)
            .map(|record| record.tokens[0].0.clone())
    }

    /// Record paths include zero-based occurrence indices, e.g.
    /// `SAMPLER[0]/HQ[0]/mastervol[0]`. Strings are safely quoted and arrays
    /// address multi-value records. Unknown paths are refused.
    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        if path == "PlugVersion[0]" {
            return Err(error("the serialized plug-in version is read-only"));
        }
        let records = self.records()?;
        let record = records.iter().find(|r| r.path == path).ok_or_else(|| error("unknown record path"))?;
        let values: Vec<&Value> = match value {
            Value::Array(values) if !values.is_empty() => values.iter().collect(),
            Value::Array(_) => return Err(error("empty record values")),
            value => vec![value],
        };
        if values.len() != record.tokens.len() {
            return Err(error("an edit must preserve the record's number of values"));
        }
        for ((old, quoted), value) in record.tokens.iter().zip(&values) {
            let numeric = !quoted && old.parse::<f64>().is_ok_and(f64::is_finite);
            if (numeric && !value.is_number() && !value.is_boolean()) || (!numeric && !value.is_string()) {
                return Err(error("an edit must preserve each record value's type"));
            }
        }
        let encoded = values
            .into_iter()
            .zip(&record.tokens)
            .map(|(value, (_, quoted))| encode_token(value, *quoted))
            .collect::<Result<Vec<_>, _>>()?
            .join(" ");
        if self.text.len() - record.values.len() + encoded.len() > MAX_PRESET_BYTES.min(MAX_DECODED_BYTES) {
            return Err(error("document is too large"));
        }
        self.text.replace_range(record.values.clone(), &encoded);
        Ok(())
    }

    pub fn to_node(&self) -> Node {
        let records = self.records().unwrap_or_default();
        Node::Map(
            records
                .into_iter()
                .map(|r| {
                    let mut values: Vec<Node> = r
                        .tokens
                        .into_iter()
                        .map(|(token, quoted)| {
                            if quoted {
                                Node::Text(token)
                            } else {
                                token.parse::<f64>().ok().filter(|n| n.is_finite()).map(Node::Float).unwrap_or(Node::Text(token))
                            }
                        })
                        .collect();
                    let value = if values.len() == 1 { values.pop().unwrap() } else { Node::List(values) };
                    (r.path, value)
                })
                .collect(),
        )
    }

    pub fn write(&self) -> Vec<u8> {
        self.text.as_bytes().to_vec()
    }

    fn records(&self) -> Result<Vec<Record>, FormatError> {
        let mut result = Vec::new();
        let mut sections: Vec<String> = Vec::new();
        let mut counts: Vec<BTreeMap<String, usize>> = vec![BTreeMap::new()];
        let mut offset = 0;
        let mut sampler = false;
        let mut token_budget = MAX_TOKENS;
        for line in self.text.split_inclusive('\n') {
            let content = line.trim();
            if content.is_empty() || content.starts_with("//") || content.starts_with('#') {
                offset += line.len();
                continue;
            }
            if content == "}" {
                if sections.pop().is_none() {
                    return Err(error("unmatched closing brace"));
                }
                counts.pop();
                offset += line.len();
                continue;
            }
            let tokens = tokenize(content, token_budget)?;
            token_budget -= tokens.len();
            let Some(first) = tokens.first() else {
                offset += line.len();
                continue;
            };
            let open = tokens.last().is_some_and(|t| t.0 == "{" && !t.2);
            if tokens.iter().take(tokens.len() - usize::from(open)).any(|t| !t.2 && (t.0 == "{" || t.0 == "}")) {
                return Err(error("unexpected brace inside a record"));
            }
            let name = first.0.replace('~', "~0").replace('/', "~1");
            let count = counts.last_mut().unwrap().entry(name.clone()).or_default();
            let segment = format!("{name}[{count}]");
            *count += 1;
            let path = sections.iter().chain(std::iter::once(&segment)).cloned().collect::<Vec<_>>().join("/");
            if path.len() > MAX_PATH_BYTES {
                return Err(error("record path is too long"));
            }
            if open {
                if sections.is_empty() && first.0 == "SAMPLER" {
                    sampler = true;
                }
                if tokens.len() > 2 {
                    let start = line.find(content).unwrap() + tokens[1].1.start;
                    let end = line.find(content).unwrap() + tokens[tokens.len() - 2].1.end;
                    result.push(Record {
                        path: format!("{path}/$args"),
                        values: offset + start..offset + end,
                        tokens: tokens[1..tokens.len() - 1].iter().map(|t| (t.0.clone(), t.2)).collect(),
                    });
                }
                if sections.len() >= 128 {
                    return Err(error("sections nested too deeply"));
                }
                sections.push(segment);
                counts.push(BTreeMap::new());
            } else if tokens.len() >= 2 {
                let start = line.find(content).unwrap() + tokens[1].1.start;
                let end = line.find(content).unwrap() + tokens.last().unwrap().1.end;
                result.push(Record {
                    path,
                    values: offset + start..offset + end,
                    tokens: tokens.into_iter().skip(1).map(|t| (t.0, t.2)).collect(),
                });
            }
            offset += line.len();
        }
        if !sections.is_empty() || !sampler {
            return Err(error("unclosed section or missing SAMPLER"));
        }
        Ok(result)
    }
}

fn tokenize(line: &str, budget: usize) -> Result<Vec<(String, Range<usize>, bool)>, FormatError> {
    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if tokens.len() >= budget {
            return Err(error("too many record tokens"));
        }
        let mut value = String::new();
        let mut end = start + ch.len_utf8();
        let quoted = ch == '"';
        if ch == '"' {
            let mut closed = false;
            while let Some((at, ch)) = chars.next() {
                end = at + ch.len_utf8();
                if ch == '"' {
                    closed = true;
                    break;
                }
                if ch == '\\' {
                    let (at, escaped) = chars.next().ok_or_else(|| error("unfinished escape"))?;
                    end = at + escaped.len_utf8();
                    if escaped != '\\' && escaped != '"' {
                        value.push('\\');
                    }
                    value.push(escaped);
                } else {
                    value.push(ch);
                }
            }
            if !closed {
                return Err(error("unclosed quoted value"));
            }
        } else {
            value.push(ch);
            while let Some(&(at, ch)) = chars.peek() {
                if ch.is_whitespace() {
                    break;
                }
                chars.next();
                value.push(ch);
                end = at + ch.len_utf8();
            }
        }
        tokens.push((value, start..end, quoted));
    }
    Ok(tokens)
}

fn encode_token(value: &Value, quoted: bool) -> Result<String, FormatError> {
    match value {
        Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => Ok(n.to_string()),
        Value::String(text) if !text.chars().any(|c| matches!(c, '\0' | '\r' | '\n')) => {
            if quoted {
                Ok(format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")))
            } else if text.is_empty()
                || text.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '{' | '}'))
                || text.parse::<f64>().is_ok_and(f64::is_finite)
            {
                Err(error("the edited text must remain one nonnumeric bare token"))
            } else {
                Ok(text.clone())
            }
        }
        Value::Bool(value) => Ok(if *value { "1" } else { "0" }.to_string()),
        _ => Err(error("record values must be finite numbers, booleans, or single-line strings")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub preset: Option<Preset>,
    header: Vec<u8>,
    tail: Vec<u8>,
    trailer: Vec<u8>,
    bank: usize,
}

impl State {
    pub fn read(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() > MAX_PRESET_BYTES.min(MAX_DECODED_BYTES) {
            return Err(error("component state is too large"));
        }
        let chunk = vstpreset::vst2_chunk(bytes)?.ok_or_else(|| error("missing VST3 compatibility wrapper"))?;
        if chunk.plugin != *b"SD3a" || chunk.kind != *b"FBCh" {
            return Err(error("component state is not Superior Drummer 3"));
        }
        let bank = 8 + u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let start = bank + 160;
        let end = start + chunk.data.len();
        let (preset, tail) = if chunk.data.first() == Some(&0) {
            (None, chunk.data.to_vec())
        } else {
            let zero = chunk.data.iter().position(|b| *b == 0).ok_or_else(|| error("missing text terminator"))?;
            (Some(Preset::read(&chunk.data[..zero])?), chunk.data[zero + 1..].to_vec())
        };
        Ok(Self { preset, header: bytes[..start].to_vec(), tail, trailer: bytes[end..].to_vec(), bank })
    }

    /// Product version in the compatibility-bank header, independent of the
    /// native text's PlugVersion. Both must be checked before writing a state.
    pub fn version(&self) -> String {
        let version = u32::from_be_bytes(self.header[self.bank + 20..self.bank + 24].try_into().unwrap());
        format!("{}.{}.{}", version >> 16, (version >> 8) & 0xff, version & 0xff)
    }

    pub fn set(&mut self, path: &str, value: &Value) -> Result<(), FormatError> {
        self.preset.as_mut().ok_or_else(|| error("initialize the sampler before editing it"))?.set(path, value)
    }

    pub fn to_node(&self) -> Node {
        self.preset.as_ref().map(Preset::to_node).unwrap_or(Node::Map(Vec::new()))
    }

    pub fn write(&self) -> Result<Vec<u8>, FormatError> {
        let mut body = self.preset.as_ref().map(Preset::write).unwrap_or_default();
        if self.preset.is_some() {
            body.push(0);
        }
        body.extend_from_slice(&self.tail);
        if self.header.len() + body.len() + self.trailer.len() > MAX_PRESET_BYTES.min(MAX_DECODED_BYTES) {
            return Err(error("component state is too large"));
        }
        let mut bytes = self.header.clone();
        bytes[self.bank + 156..self.bank + 160].copy_from_slice(&(body.len() as u32).to_be_bytes());
        bytes[self.bank + 4..self.bank + 8].copy_from_slice(&((152 + body.len()) as u32).to_be_bytes());
        bytes.extend(body);
        bytes.extend_from_slice(&self.trailer);
        Ok(bytes)
    }
}

fn error(message: &str) -> FormatError {
    FormatError::new(format!("Superior Drummer preset: {message}"))
}
