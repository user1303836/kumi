//! JSON syntax diagnostics used by file surfaces that retain JSON.parse's error text.
use super::*;

#[derive(Clone, Copy)]
enum Frame {
    Value,
    Done,
    ObjectKey(bool),
    ObjectColon,
    ObjectNext,
    ArrayFirst,
    ArrayNext,
}
struct Parser {
    units: Vec<u16>,
    at: usize,
}
impl Parser {
    fn current(&self) -> Option<u16> {
        self.units.get(self.at).copied()
    }
    fn whitespace(&mut self) {
        while self.current().is_some_and(|c| [9, 10, 13, 32].contains(&c)) {
            self.at += 1;
        }
    }
    fn at_error(&self, message: &str) -> Vec<u16> {
        let (mut line, mut column) = (1, 1);
        let mut previous = 0;
        for &unit in &self.units[..self.at.min(self.units.len())] {
            if unit == 13 || (unit == 10 && previous != 13) {
                line += 1;
                column = 1;
            } else if unit != 10 || previous != 13 {
                column += 1;
            }
            previous = unit;
        }
        let suffix = if message.ends_with("JSON") { "" } else { " in JSON" };
        format!("{message}{suffix} at position {} (line {line} column {column})", self.at).encode_utf16().collect()
    }
    fn unexpected(&self) -> Vec<u16> {
        let Some(unit) = self.current() else { return "Unexpected end of JSON input".encode_utf16().collect() };
        if unit == 34 {
            return self.at_error("Unexpected string");
        }
        if matches!(unit, 48..=57) {
            return self.at_error("Unexpected number");
        }
        let text = String::from_utf16_lossy(&self.units);
        if ["undefined", "NaN", "Infinity", "[object Object]"].contains(&text.as_str()) {
            return format!("\"{text}\" is not valid JSON").encode_utf16().collect();
        }
        let mut result: Vec<u16> = "Unexpected token '".encode_utf16().collect();
        result.push(unit);
        result.extend("', ".encode_utf16());
        let (from, to) =
            if self.units.len() <= 20 { (0, self.units.len()) } else { (self.at.saturating_sub(10), (self.at + 10).min(self.units.len())) };
        if self.units.len() > 20 && self.at >= 10 {
            result.extend("...".encode_utf16());
        }
        result.push(34);
        result.extend(&self.units[from..to]);
        result.push(34);
        if to < self.units.len() {
            result.extend("...".encode_utf16());
        }
        result.extend(" is not valid JSON".encode_utf16());
        result
    }
    fn string(&mut self) -> Result<(), Vec<u16>> {
        self.at += 1;
        loop {
            match self.current() {
                None => return Err(self.at_error("Unterminated string")),
                Some(34) => {
                    self.at += 1;
                    return Ok(());
                }
                Some(0..=31) => return Err(self.at_error("Bad control character in string literal")),
                Some(92) => {
                    self.at += 1;
                    match self.current() {
                        Some(34 | 92 | 47 | 98 | 102 | 110 | 114 | 116) => self.at += 1,
                        Some(117) => {
                            self.at += 1;
                            for _ in 0..4 {
                                if !self.current().is_some_and(|c| matches!(c,48..=57|65..=70|97..=102)) {
                                    return Err(self.at_error("Bad Unicode escape"));
                                }
                                self.at += 1;
                            }
                        }
                        None => return Err(self.unexpected()),
                        Some(256..=u16::MAX) => return Err(self.unexpected()),
                        _ => return Err(self.at_error("Bad escaped character")),
                    }
                }
                _ => self.at += 1,
            }
        }
    }
    fn number(&mut self) -> Result<(), Vec<u16>> {
        if self.current() == Some(45) {
            self.at += 1;
            if !self.current().is_some_and(|c| matches!(c, 48..=57)) {
                return Err(self.at_error("No number after minus sign"));
            }
        }
        if self.current() == Some(48) {
            self.at += 1;
            if self.current().is_some_and(|c| matches!(c, 48..=57)) {
                return Err(self.at_error("Unexpected number"));
            }
        } else {
            while self.current().is_some_and(|c| matches!(c, 48..=57)) {
                self.at += 1;
            }
        }
        if self.current() == Some(46) {
            self.at += 1;
            if !self.current().is_some_and(|c| matches!(c, 48..=57)) {
                return Err(self.at_error("Unterminated fractional number"));
            }
            while self.current().is_some_and(|c| matches!(c, 48..=57)) {
                self.at += 1;
            }
        }
        if matches!(self.current(), Some(101 | 69)) {
            self.at += 1;
            if matches!(self.current(), Some(43 | 45)) {
                self.at += 1;
            }
            if !self.current().is_some_and(|c| matches!(c, 48..=57)) {
                return Err(self.at_error("Exponent part is missing a number"));
            }
            while self.current().is_some_and(|c| matches!(c, 48..=57)) {
                self.at += 1;
            }
        }
        Ok(())
    }
    fn parse(&mut self) -> Result<(), Vec<u16>> {
        let mut stack = vec![Frame::Done, Frame::Value];
        while let Some(frame) = stack.pop() {
            self.whitespace();
            match frame {
                Frame::Value => match self.current() {
                    Some(123) => {
                        self.at += 1;
                        stack.push(Frame::ObjectKey(true));
                    }
                    Some(91) => {
                        self.at += 1;
                        stack.push(Frame::ArrayFirst);
                    }
                    Some(34) => self.string()?,
                    Some(45 | 48..=57) => self.number()?,
                    Some(unit @ (116 | 102 | 110)) => {
                        let literal = match unit {
                            116 => "true",
                            102 => "false",
                            _ => "null",
                        };
                        for expected in literal.encode_utf16() {
                            if self.current() != Some(expected) {
                                return Err(self.unexpected());
                            }
                            self.at += 1;
                        }
                    }
                    _ => return Err(self.unexpected()),
                },
                Frame::Done => {
                    if self.current().is_some() {
                        return Err(self.at_error("Unexpected non-whitespace character after JSON"));
                    }
                }
                Frame::ObjectKey(first) => {
                    if first && self.current() == Some(125) {
                        self.at += 1;
                        continue;
                    }
                    if self.current() != Some(34) {
                        return Err(self.at_error(if first {
                            "Expected property name or '}'"
                        } else {
                            "Expected double-quoted property name"
                        }));
                    }
                    self.string()?;
                    stack.push(Frame::ObjectColon);
                }
                Frame::ObjectColon => {
                    if self.current() != Some(58) {
                        return Err(self.at_error("Expected ':' after property name"));
                    }
                    self.at += 1;
                    stack.push(Frame::ObjectNext);
                    stack.push(Frame::Value);
                }
                Frame::ObjectNext => match self.current() {
                    Some(125) => self.at += 1,
                    Some(44) => {
                        self.at += 1;
                        stack.push(Frame::ObjectKey(false));
                    }
                    _ => return Err(self.at_error("Expected ',' or '}' after property value")),
                },
                Frame::ArrayFirst => {
                    if self.current() == Some(93) {
                        self.at += 1;
                    } else {
                        stack.push(Frame::ArrayNext);
                        stack.push(Frame::Value);
                    }
                }
                Frame::ArrayNext => match self.current() {
                    Some(93) => self.at += 1,
                    Some(44) => {
                        self.at += 1;
                        stack.push(Frame::ArrayNext);
                        stack.push(Frame::Value);
                    }
                    _ => return Err(self.at_error("Expected ',' or ']' after array element")),
                },
            }
        }
        Ok(())
    }
}
pub fn syntax_error_units(units: &[u16]) -> Option<Vec<u16>> {
    Parser { units: units.to_vec(), at: 0 }.parse().err()
}
pub fn syntax_error(text: &str) -> Option<String> {
    syntax_error_units(&text.encode_utf16().collect::<Vec<_>>()).map(|units| String::from_utf16_lossy(&units))
}
pub fn parse_json(text: &str) -> Result<Value, LiveError> {
    serde_json::from_str(text).map_err(|cause| LiveError::error(syntax_error(text).unwrap_or_else(|| cause.to_string())))
}
