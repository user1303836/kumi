//! Code in an answer, coloured by its language: its comments, strings, numbers and keywords, each language read by a
//! small lexer of its own (JavaScript, gen~'s GenExpr, Lua, Python and REAPER's EEL). A line is read in the state the
//! line before left (inside a block comment, say), so a block's lines can be read one at a time as they stream.

use std::ops::Range;

/// A language Kumi colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    /// JavaScript and TypeScript: Live's extension, Max's `js`.
    JavaScript,
    /// GenExpr, gen~'s language.
    GenExpr,
    /// Lua: Renoise's tools.
    Lua,
    /// Python: Live's Remote Script and `run_python`.
    Python,
    /// EEL2: REAPER's scripts and JSFX.
    Eel,
}

impl Language {
    /// The language a fence's info string names by its first word (```py, ```{.python}), in any case; None for one
    /// Kumi doesn't colour.
    pub fn named(info: &str) -> Option<Language> {
        let word = info.split_whitespace().next()?.trim_start_matches(['{', '.']).trim_end_matches('}').to_ascii_lowercase();
        Some(match word.as_str() {
            "js" | "javascript" | "jsx" | "mjs" | "cjs" | "ts" | "typescript" | "tsx" => Language::JavaScript,
            "genexpr" | "gen" | "gen~" | "codebox" => Language::GenExpr,
            "lua" => Language::Lua,
            "py" | "python" | "python3" | "py3" => Language::Python,
            "eel" | "eel2" | "jsfx" => Language::Eel,
            _ => return None,
        })
    }

    /// Its keywords, the literals among them (true, nil, None).
    fn keywords(self) -> &'static [&'static str] {
        match self {
            Language::JavaScript => &[
                "async",
                "await",
                "break",
                "case",
                "catch",
                "class",
                "const",
                "continue",
                "debugger",
                "default",
                "delete",
                "do",
                "else",
                "enum",
                "export",
                "extends",
                "false",
                "finally",
                "for",
                "function",
                "if",
                "implements",
                "import",
                "in",
                "instanceof",
                "interface",
                "let",
                "new",
                "null",
                "private",
                "protected",
                "public",
                "return",
                "static",
                "super",
                "switch",
                "this",
                "throw",
                "true",
                "try",
                "typeof",
                "undefined",
                "var",
                "void",
                "while",
                "with",
                "yield",
            ],
            Language::GenExpr => {
                &["Buffer", "Data", "Delay", "History", "Param", "break", "continue", "else", "for", "if", "return", "while"]
            }
            Language::Lua => &[
                "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or",
                "repeat", "return", "then", "true", "until", "while",
            ],
            Language::Python => &[
                "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
                "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or",
                "pass", "raise", "return", "try", "while", "with", "yield",
            ],
            Language::Eel => &["function", "global", "globals", "instance", "local", "loop", "static", "this", "while"],
        }
    }

    /// Whether `byte` carries a name on: letters, digits, `_`, any byte of a character past ASCII, and JavaScript's `$`.
    fn names(self, byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80 || (byte == b'$' && self == Language::JavaScript)
    }
}

/// What a run of code is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Comment,
    String,
    Number,
    Keyword,
}

/// Where a line of code begins: in plain code, or inside what a line before it opened and didn't close.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Within {
    #[default]
    Code,
    /// A `/* */` comment.
    BlockComment,
    /// Lua's long comment (`--[[`, or `--[==[` with as many `=` as its level).
    LongComment(usize),
    /// Lua's long string (`[[`, or `[==[`).
    LongString(usize),
    /// Python's string in three quotes, by its quote (`'''` or `"""`).
    Triple(u8),
    /// JavaScript's template string.
    Template,
}

impl Within {
    fn token(self) -> Token {
        match self {
            Within::BlockComment | Within::LongComment(_) => Token::Comment,
            _ => Token::String,
        }
    }
}

/// `line`'s comments, strings, numbers and keywords in `language`, read from `within`: each run's bytes and what it is,
/// in order, and where the next line begins. Anything else (names, operators, spaces) is plain code.
pub fn line(language: Language, line: &str, within: Within) -> (Vec<(Range<usize>, Token)>, Within) {
    let mut lexer = Lexer { language, text: line.as_bytes(), runs: vec![], no_regex: false };
    let within = lexer.read(within);
    (lexer.runs, within)
}

struct Lexer<'a> {
    language: Language,
    text: &'a [u8],
    runs: Vec<(Range<usize>, Token)>,
    /// A regular expression's scan ran to the line's end and consumed nothing: the rest of the line's `/` divide, so a
    /// line costs its length however its slashes fall (a scan that closes consumes what it read).
    no_regex: bool,
}

impl Lexer<'_> {
    /// Reads the line, the rest of what an earlier line opened first; where the next line begins.
    fn read(&mut self, within: Within) -> Within {
        let mut at = 0;
        if within != Within::Code {
            match self.close(within, 0) {
                Some(end) => {
                    self.runs.push((0..end, within.token()));
                    at = end;
                }
                None => {
                    self.push(0..self.text.len(), within.token());
                    return within;
                }
            }
        }
        while at < self.text.len() {
            match self.next(at) {
                Ok(end) => at = end,
                Err(open) => return open,
            }
        }
        Within::Code
    }

    /// Reads what begins at `at`: where it ends, or Err with what it leaves open, having run to the line's end.
    fn next(&mut self, at: usize) -> Result<usize, Within> {
        let (text, language) = (self.text, self.language);
        let rest = &text[at..];
        let byte = rest[0];
        // Comments.
        let slashes = matches!(language, Language::JavaScript | Language::GenExpr | Language::Eel);
        if (slashes && rest.starts_with(b"//")) || (language == Language::Python && byte == b'#') {
            return self.to_end(at, Token::Comment, Within::Code);
        }
        if slashes && rest.starts_with(b"/*") {
            return self.open(at, at + 2, Within::BlockComment);
        }
        // JavaScript's regular expression: a `/` where a value begins, to its closing `/` and its flags. Any other `/`
        // divides, and so does one whose line ends first.
        if language == Language::JavaScript && byte == b'/' && !self.no_regex && self.value_begins(at) {
            match self.regex_end(at) {
                Some(end) => {
                    self.runs.push((at..end, Token::String));
                    return Ok(end);
                }
                None => self.no_regex = true,
            }
        }
        if language == Language::Lua && rest.starts_with(b"--") {
            return match long_bracket(&rest[2..]) {
                Some((level, opener)) => self.open(at, at + 2 + opener, Within::LongComment(level)),
                None => self.to_end(at, Token::Comment, Within::Code),
            };
        }
        // Strings.
        if language == Language::Lua && byte == b'[' {
            if let Some((level, opener)) = long_bracket(rest) {
                return self.open(at, at + opener, Within::LongString(level));
            }
        }
        if language == Language::JavaScript && byte == b'`' {
            return self.open(at, at + 1, Within::Template);
        }
        // EEL's 'a' is a number (its character's code, up to four of them); an apostrophe on its own is a word's, in
        // JSFX's `desc: Kumi's gain`.
        if language == Language::Eel && byte == b'\'' {
            let close = text.iter().skip(at + 1).take(5).position(|byte| *byte == b'\'').filter(|close| *close > 0);
            return Ok(match close {
                Some(close) => {
                    self.runs.push((at..at + close + 2, Token::Number));
                    at + close + 2
                }
                None => at + 1,
            });
        }
        if byte == b'"' || byte == b'\'' {
            return self.string(at, at);
        }
        // EEL's constants: $x1F, $'a', $pi, $e and $phi.
        if language == Language::Eel && byte == b'$' {
            return Ok(self.eel_constant(at));
        }
        // JSFX's sections: @init, @slider, @block, @sample, @serialize and @gfx.
        if language == Language::Eel && byte == b'@' {
            let end = self.word_end(at + 1);
            if ["init", "slider", "block", "sample", "serialize", "gfx"].contains(&std::str::from_utf8(&text[at + 1..end]).unwrap_or("")) {
                self.runs.push((at..end, Token::Keyword));
                return Ok(end);
            }
            return Ok(at + 1);
        }
        // Numbers: a digit, or a point and a digit.
        if byte.is_ascii_digit() || (byte == b'.' && rest.get(1).is_some_and(u8::is_ascii_digit)) {
            let end = self.number_end(at);
            self.runs.push((at..end, Token::Number));
            return Ok(end);
        }
        // Names, some of them keywords, and Python's prefixed strings (r"", b'', f"…").
        if language.names(byte) {
            let end = self.word_end(at);
            let word = std::str::from_utf8(&text[at..end]).unwrap_or("");
            let prefix = || matches!(word.to_ascii_lowercase().as_str(), "r" | "b" | "f" | "u" | "rb" | "br" | "fr" | "rf");
            if language == Language::Python && matches!(text.get(end), Some(b'"' | b'\'')) && prefix() {
                return self.string(at, end);
            }
            if language.keywords().contains(&word) {
                self.runs.push((at..end, Token::Keyword));
            }
            return Ok(end);
        }
        Ok(at + 1)
    }

    /// A run from `at` to the line's end, leaving `open` open.
    fn to_end(&mut self, at: usize, token: Token, open: Within) -> Result<usize, Within> {
        self.push(at..self.text.len(), token);
        if open == Within::Code {
            Ok(self.text.len())
        } else {
            Err(open)
        }
    }

    /// What opens at `at` with its opener running to `inside`: closed on this line, or open past it.
    fn open(&mut self, at: usize, inside: usize, within: Within) -> Result<usize, Within> {
        match self.close(within, inside) {
            Some(end) => {
                self.runs.push((at..end, within.token()));
                Ok(end)
            }
            None => self.to_end(at, within.token(), within),
        }
    }

    /// Where what's open closes, reading from `from`: the byte after its close, or None when it runs past the line.
    fn close(&self, within: Within, from: usize) -> Option<usize> {
        let text = self.text;
        let escaped = |close: &[u8]| {
            let mut at = from;
            while at < text.len() {
                if text[at] == b'\\' {
                    at += 2;
                } else if text[at..].starts_with(close) {
                    return Some(at + close.len());
                } else {
                    at += 1;
                }
            }
            None
        };
        match within {
            Within::Code => Some(from),
            Within::BlockComment => find(&text[from..], b"*/").map(|at| from + at + 2),
            Within::LongComment(level) | Within::LongString(level) => {
                let close = [&b"]"[..], &b"=".repeat(level), b"]"].concat();
                find(&text[from..], &close).map(|at| from + at + close.len())
            }
            Within::Triple(quote) => escaped(&[quote; 3]),
            Within::Template => escaped(b"`"),
        }
    }

    /// A quoted string whose prefix begins at `at` and whose quote is at `quote`: where it ends. Python's three quotes
    /// may run on past the line; any other string ends with the line if its quote doesn't close it.
    fn string(&mut self, at: usize, quote: usize) -> Result<usize, Within> {
        let text = self.text;
        let mark = text[quote];
        if self.language == Language::Python && text[quote..].starts_with(&[mark; 3]) {
            return self.open(at, quote + 3, Within::Triple(mark));
        }
        let mut end = quote + 1;
        while end < text.len() {
            match text[end] {
                b'\\' => end += 2,
                byte if byte == mark => {
                    end += 1;
                    break;
                }
                _ => end += 1,
            }
        }
        let end = end.min(text.len());
        self.runs.push((at..end, Token::String));
        Ok(end)
    }

    /// Whether a JavaScript value, not a division, begins at `at`: at the line's start, after an opening or an operator,
    /// or after a keyword that takes one (`return /x/`). Not after `}`, which more often closes a value (TSX's `<a b={c} />`).
    fn value_begins(&self, at: usize) -> bool {
        let before = self.text[..at].trim_ascii_end();
        let Some(last) = before.last() else { return true };
        if b"(,=:[!&|?{;".contains(last) {
            return true;
        }
        let start = before.iter().rposition(|byte| !self.language.names(*byte)).map_or(0, |at| at + 1);
        let word = std::str::from_utf8(&before[start..]).unwrap_or("");
        // (`obj.default` is a property, a value.)
        before[..start].last() != Some(&b'.')
            && self.language.keywords().contains(&word)
            && !["this", "super", "true", "false", "null", "undefined"].contains(&word)
    }

    /// Where the regular expression from `at` (its opening `/`) ends: past its closing `/` and its flags. A `/` escaped
    /// or inside a class (`[/]`) doesn't close it; None when the line ends first.
    fn regex_end(&self, at: usize) -> Option<usize> {
        let text = self.text;
        let (mut end, mut class) = (at + 1, false);
        while end < text.len() {
            match text[end] {
                b'\\' => end += 1,
                b'[' => class = true,
                b']' => class = false,
                b'/' if !class => return Some(end + 1 + text[end + 1..].iter().take_while(|byte| byte.is_ascii_alphabetic()).count()),
                _ => {}
            }
            end += 1;
        }
        None
    }

    /// EEL's constant at `at` (its `$`): a number when it's one, else the `$` alone as plain code.
    fn eel_constant(&mut self, at: usize) -> usize {
        let text = self.text;
        let end = match text.get(at + 1) {
            Some(b'x' | b'X') => {
                let digits = text[at + 2..].iter().take_while(|byte| byte.is_ascii_hexdigit()).count();
                (digits > 0).then_some(at + 2 + digits)
            }
            Some(b'\'') => text[at + 2..].iter().position(|byte| *byte == b'\'').map(|close| at + 2 + close + 1),
            _ => {
                let end = self.word_end(at + 1);
                let word = std::str::from_utf8(&text[at + 1..end]).unwrap_or("").to_ascii_lowercase();
                matches!(word.as_str(), "pi" | "e" | "phi").then_some(end)
            }
        };
        match end {
            Some(end) => {
                self.runs.push((at..end, Token::Number));
                end
            }
            None => at + 1,
        }
    }

    /// Where the name from `at` ends.
    fn word_end(&self, at: usize) -> usize {
        at + self.text[at..].iter().take_while(|byte| self.language.names(**byte)).count()
    }

    /// A run, unless it's empty (an empty line inside a comment).
    fn push(&mut self, range: Range<usize>, token: Token) {
        if !range.is_empty() {
            self.runs.push((range, token));
        }
    }

    /// Where the number from `at` ends: its digits, letters (0x, 1e3, 10n, 2j), points and `_`, and an exponent's sign.
    fn number_end(&self, at: usize) -> usize {
        let text = self.text;
        let hex = text[at..].len() > 1 && text[at] == b'0' && matches!(text[at + 1], b'x' | b'X');
        let mut end = at;
        while end < text.len() {
            let byte = text[end];
            let exponent = if hex { matches!(byte, b'p' | b'P') } else { matches!(byte, b'e' | b'E') };
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' {
                end += 1;
                if exponent && matches!(text.get(end), Some(b'+' | b'-')) {
                    end += 1;
                }
            } else {
                break;
            }
        }
        end
    }
}

/// Where `needle` first is in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// Lua's long bracket at the start of `text` (`[[`, `[=[`, …): its level and how many bytes it takes.
fn long_bracket(text: &[u8]) -> Option<(usize, usize)> {
    let level = text.get(1..)?.iter().take_while(|byte| **byte == b'=').count();
    (text.first() == Some(&b'[') && text.get(1 + level) == Some(&b'[')).then_some((level, level + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `code`'s runs, each as its text and what it is, read line by line from plain code.
    fn runs(language: Language, code: &str) -> Vec<(String, Token)> {
        let mut within = Within::Code;
        let mut found = vec![];
        for text in code.split('\n') {
            let (runs, next) = line(language, text, within);
            found.extend(runs.into_iter().map(|(range, token)| (text[range].to_string(), token)));
            within = next;
        }
        found
    }

    // (`Quoted` for a string, so a `String` stays the type.)
    use Token::{Comment, Keyword, Number, String as Quoted};

    #[test]
    fn a_fence_names_its_language_by_its_first_word_in_any_case() {
        assert_eq!(Language::named("py"), Some(Language::Python));
        assert_eq!(Language::named("Python title=\"chorus.py\""), Some(Language::Python));
        assert_eq!(Language::named("{.lua}"), Some(Language::Lua));
        assert_eq!(Language::named("gen~"), Some(Language::GenExpr));
        assert_eq!(Language::named("TypeScript"), Some(Language::JavaScript));
        assert_eq!(Language::named("jsfx"), Some(Language::Eel));
        assert_eq!(Language::named("rust"), None);
        assert_eq!(Language::named(""), None);
    }

    #[test]
    fn javascript_colours_its_keywords_strings_numbers_and_comments() {
        assert_eq!(
            runs(Language::JavaScript, "const gain = -3.5e-2; // quieter\nlet name = 'Kick' + `808 ${n}`;\nif (x) return null;"),
            [
                ("const".into(), Keyword),
                ("3.5e-2".into(), Number),
                ("// quieter".into(), Comment),
                ("let".into(), Keyword),
                ("'Kick'".into(), Quoted),
                ("`808 ${n}`".into(), Quoted),
                ("if".into(), Keyword),
                ("return".into(), Keyword),
                ("null".into(), Keyword),
            ]
        );
        // A name that holds a keyword or a digit is a name, and so is one with `$`.
        assert!(runs(Language::JavaScript, "iffy in1 $if _for").is_empty());
        // A regular expression where a value begins is one string, a quote in it included; a `/` after a value divides.
        assert_eq!(
            runs(Language::JavaScript, "s.replace(/\"/g, \"\"); x = /[/]+$/i.test(y)\nz = a / b / 2"),
            [("/\"/g".into(), Quoted), ("\"\"".into(), Quoted), ("/[/]+$/i".into(), Quoted), ("2".into(), Number),]
        );
        // After a keyword that takes a value it's one too; after a value that's a keyword, or a `}`, it divides or closes.
        assert_eq!(
            runs(Language::JavaScript, "return /\"/.test(s)\nthis / a / b\n<A b={c} /><D e={f} />\nobj.default / 2 / y"),
            [
                ("return".into(), Keyword),
                ("/\"/".into(), Quoted),
                ("this".into(), Keyword),
                ("default".into(), Keyword),
                ("2".into(), Number)
            ]
        );
    }

    #[test]
    fn a_block_comment_and_a_template_run_on_to_the_lines_after() {
        assert_eq!(
            runs(Language::JavaScript, "a /* one\ntwo */ b\nc = `x\ny` + 1"),
            [("/* one".into(), Comment), ("two */".into(), Comment), ("`x".into(), Quoted), ("y`".into(), Quoted), ("1".into(), Number),]
        );
        let (_, within) = line(Language::JavaScript, "/* open", Within::Code);
        assert_eq!(within, Within::BlockComment);
        assert_eq!(line(Language::JavaScript, "still", within), (vec![(0..5, Comment)], Within::BlockComment));
    }

    #[test]
    fn genexpr_colours_its_declarations_and_control_words() {
        assert_eq!(
            runs(Language::GenExpr, "Param rate(2, min=0);\nHistory z(0);\nif (in1 > 0.5) { out1 = z; } // gate"),
            [
                ("Param".into(), Keyword),
                ("2".into(), Number),
                ("0".into(), Number),
                ("History".into(), Keyword),
                ("0".into(), Number),
                ("if".into(), Keyword),
                ("0.5".into(), Number),
                ("// gate".into(), Comment),
            ]
        );
    }

    #[test]
    fn lua_colours_its_long_comments_and_strings_by_their_level() {
        assert_eq!(
            runs(Language::Lua, "local s = [==[a]]b]==] -- note\n--[[ long\nstill ]] x = nil\nprint(\"hi\", 0x1F)"),
            [
                ("local".into(), Keyword),
                ("[==[a]]b]==]".into(), Quoted),
                ("-- note".into(), Comment),
                ("--[[ long".into(), Comment),
                ("still ]]".into(), Comment),
                ("nil".into(), Keyword),
                ("\"hi\"".into(), Quoted),
                ("0x1F".into(), Number),
            ]
        );
    }

    #[test]
    fn python_colours_prefixed_and_three_quoted_strings() {
        assert_eq!(
            runs(
                Language::Python,
                "def track(song):\n    \"\"\"The first\n    track.\"\"\"\n    return f'{song}' if song else None  # none"
            ),
            [
                ("def".into(), Keyword),
                ("\"\"\"The first".into(), Quoted),
                ("    track.\"\"\"".into(), Quoted),
                ("return".into(), Keyword),
                ("f'{song}'".into(), Quoted),
                ("if".into(), Keyword),
                ("else".into(), Keyword),
                ("None".into(), Keyword),
                ("# none".into(), Comment),
            ]
        );
        // A string's escaped quote doesn't end it.
        assert_eq!(runs(Language::Python, r#"x = "a \" b""#), [(r#""a \" b""#.into(), Quoted)]);
    }

    #[test]
    fn eel_colours_its_constants_and_jsfx_sections() {
        assert_eq!(
            runs(
                Language::Eel,
                "desc: Kumi's gain\n@sample\nspl0 *= $pi * $x1F + $'a' + 'ab'; // gain\nfunction f() local(t) ( t = this.x; );"
            ),
            [
                ("@sample".into(), Keyword),
                ("$pi".into(), Number),
                ("$x1F".into(), Number),
                ("$'a'".into(), Number),
                ("'ab'".into(), Number),
                ("// gain".into(), Comment),
                ("function".into(), Keyword),
                ("local".into(), Keyword),
                ("this".into(), Keyword),
            ]
        );
    }

    #[test]
    fn a_long_line_costs_its_length_however_its_slashes_fall() {
        // Each `/` here begins a regular expression whose `[` never closes: an open fence's unfinished line is read again
        // every frame, on the screen's thread.
        let text = "(/[".repeat(200_000);
        let started = std::time::Instant::now();
        line(Language::JavaScript, &text, Within::Code);
        assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    }

    #[test]
    fn any_line_is_read_without_slicing_a_character_or_reading_past_it() {
        let pieces = [
            "\"", "'", "`", "/*", "*/", "--[[", "]]", "#", "\\", "$", "$x", "$'", "@", "0x", "1e", ".5", "é", "名", "🥁", " ", "r", "f'",
            "[=", "--", "/", "[",
        ];
        for language in [Language::JavaScript, Language::GenExpr, Language::Lua, Language::Python, Language::Eel] {
            for a in pieces {
                for b in pieces {
                    for c in pieces {
                        let text = format!("{a}{b}{c}");
                        for within in [Within::Code, Within::BlockComment, Within::LongString(1), Within::Triple(b'"'), Within::Template] {
                            let (runs, _) = line(language, &text, within);
                            let mut last = 0;
                            for (range, _) in runs {
                                assert!(range.start >= last && range.start < range.end && range.end <= text.len(), "{text:?}");
                                assert!(text.get(range.clone()).is_some(), "{language:?} {text:?} {range:?}");
                                last = range.end;
                            }
                        }
                    }
                }
            }
        }
    }
}
