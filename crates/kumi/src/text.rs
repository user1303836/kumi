use kumi_common::js;
use kumi_runtime::core::contracts::{LibraryState, LibraryStatus, WebAction, WebEvent, WebWhere};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Text,
    Escape,
    Csi,
    String,
    StringEscape,
}

/// Incremental terminal sanitizer: escape sequences and secret prefixes may span chunks.
#[derive(Clone, Debug)]
pub struct StreamingText {
    state: State,
    pending: String,
    secrets: Vec<String>,
    /// Each secret as a pattern: its UTF-16 units and their KMP prefix function.
    patterns: Vec<Pattern>,
}

#[derive(Clone, Debug)]
struct Pattern {
    units: Vec<u16>,
    prefix: Vec<usize>,
}

impl Pattern {
    fn new(secret: &str) -> Pattern {
        let units: Vec<u16> = secret.encode_utf16().collect();
        let mut prefix = vec![0; units.len()];
        let mut k = 0;
        for i in 1..units.len() {
            while k > 0 && units[i] != units[k] {
                k = prefix[k - 1];
            }
            if units[i] == units[k] {
                k += 1;
            }
            prefix[i] = k;
        }
        Pattern { units, prefix }
    }

    /// How much of the secret's start, short of all of it, `text` (UTF-16) ends with, never half a character: in one
    /// pass over its last units, not a prefix built and compared for each length.
    fn held(&self, text: &[u16]) -> usize {
        let units = &self.units;
        if units.len() < 2 {
            return 0;
        }
        let mut held = 0;
        for &unit in &text[text.len().saturating_sub(units.len() - 1)..] {
            while held > 0 && (held == units.len() || unit != units[held]) {
                held = self.prefix[held - 1];
            }
            if unit == units[held] {
                held += 1;
            }
        }
        while held > 0 && (0xd800..=0xdbff).contains(&units[held - 1]) {
            held = self.prefix[held - 1];
        }
        held
    }
}

fn is_invisible(character: char) -> bool {
    matches!(character, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

impl Default for StreamingText {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl StreamingText {
    pub fn new(secrets: &[String]) -> StreamingText {
        StreamingText {
            state: State::Text,
            pending: String::new(),
            secrets: secrets.to_vec(),
            patterns: secrets.iter().map(|secret| Pattern::new(secret)).collect(),
        }
    }

    /// A key saved while the app is running joins its existing streaming redactions.
    pub fn add_secret(&mut self, secret: String) {
        self.patterns.push(Pattern::new(&secret));
        self.secrets.push(secret);
    }

    pub fn push(&mut self, input: &str) -> String {
        let mut clean = String::new();
        for character in input.chars() {
            let code = character as u32;
            if self.state == State::String {
                if code == 7 || code == 0x9c {
                    self.state = State::Text;
                } else if code == 27 {
                    self.state = State::StringEscape;
                }
                continue;
            }
            if self.state == State::StringEscape {
                self.state = if character == '\\' || code == 7 {
                    State::Text
                } else if code == 27 {
                    State::StringEscape
                } else {
                    State::String
                };
                continue;
            }
            if self.state == State::Csi {
                if (0x40..=0x7e).contains(&code) {
                    self.state = State::Text;
                } else if code == 27 {
                    self.state = State::Escape;
                }
                continue;
            }
            if self.state == State::Escape {
                self.state = if character == '[' {
                    State::Csi
                } else if matches!(character, ']' | 'P' | '^' | '_' | 'X') {
                    State::String
                } else if code == 27 {
                    State::Escape
                } else {
                    State::Text
                };
                continue;
            }
            if code == 27 {
                self.state = State::Escape;
                continue;
            }
            if code == 0x9b {
                self.state = State::Csi;
                continue;
            }
            if matches!(code, 0x90 | 0x98 | 0x9d | 0x9e | 0x9f) {
                self.state = State::String;
                continue;
            }
            if character == '\n' {
                clean.push(character);
            } else if character == '\t' {
                clean.push_str("    ");
            } else if code >= 0x20 && !(0x7f..=0x9f).contains(&code) && !is_invisible(character) {
                clean.push(character);
            }
        }
        self.pending.push_str(&clean);
        for secret in &self.secrets {
            if !secret.is_empty() {
                self.pending = self.pending.replace(secret.as_str(), "[redacted]");
            }
        }
        // What may be a secret's start, held back until the next chunk says.
        let units: Vec<u16> = self.pending.encode_utf16().collect();
        let pending_length = units.len();
        let retain = self.patterns.iter().map(|pattern| pattern.held(&units)).max().unwrap_or(0);
        let visible = js::string::slice(&self.pending, 0, Some((pending_length - retain) as i64));
        self.pending = if retain > 0 { js::string::slice(&self.pending, -(retain as i64), None) } else { String::new() };
        visible
    }

    pub fn finish(&mut self) -> String {
        let text = std::mem::take(&mut self.pending);
        self.discard();
        text
    }

    pub fn discard(&mut self) {
        self.pending.clear();
        self.state = State::Text;
    }
}

pub fn sanitize_text(text: &str, secrets: &[String]) -> String {
    let mut stream = StreamingText::new(secrets);
    let mut out = stream.push(text);
    out.push_str(&stream.finish());
    out
}

/// A search or a page Kumi read, in a line's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebWords {
    pub lead: String,
    pub title: String,
    pub detail: String,
}

/// A search or a page Kumi read, in a line's words: "Searched the web for", “the words”, "8 results";
/// "Read", “its title”, "where it is · what it is". `clean` makes a page's words safe to show.
pub fn web_words(event: &WebEvent, clean: &dyn Fn(&str, usize) -> String) -> WebWords {
    if event.action == WebAction::Searched {
        let results = event.results.unwrap_or(0);
        let github = event.r#where == Some(WebWhere::Github);
        let noun = match (github, results) {
            (true, 1) => "repository",
            (true, _) => "repositories",
            (false, 1) => "result",
            (false, _) => "results",
        };
        return WebWords {
            lead: format!("Searched {} for", if github { "GitHub" } else { "the web" }),
            title: format!("“{}”", clean(&event.title, 120)),
            detail: if results > 0 { format!("{results} {noun}") } else { "nothing found".to_string() },
        };
    }
    let mut place = String::new();
    let mut path = String::new();
    if let Ok(url) = url::Url::parse(event.url.as_deref().unwrap_or("")) {
        let hostname = url.host_str().unwrap_or("");
        place = hostname.strip_prefix("www.").unwrap_or(hostname).to_string();
        path = if url.path().len() > 1 { url.path().to_string() } else { String::new() };
    }
    // No address: nothing to place the page by.
    let titled = !event.title.is_empty() && event.url.as_deref() != Some(event.title.as_str());
    let files = event.files.map(|files| format!("{files} {}", if files == 1 { "file" } else { "files" })).unwrap_or_default();
    let kind = event.kind.as_deref().filter(|kind| *kind != "a page").unwrap_or("");
    WebWords {
        lead: "Read".to_string(),
        title: if titled { format!("“{}”", clean(&event.title, 120)) } else { clean(&format!("{place}{path}"), 120) },
        detail: [if titled { place.as_str() } else { "" }, kind, files.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
    }
}

/// `value.toLocaleString("en-US")` for a whole number: thousands apart with commas.
fn locale_count(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if value < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// The library in a line: learning it (and how far), or what Kumi knows of it.
pub fn library_line(status: Option<&LibraryStatus>) -> Option<String> {
    let status = status?;
    if status.state == LibraryState::New {
        return None;
    }
    let count = |value: usize| locale_count(value as i64);
    if status.learned_at.is_none_or(|at| at == 0) {
        if status.state == LibraryState::Paused {
            return Some("Learning your library · paused while Live plays".to_string());
        }
        let progress = match status.todo {
            Some(todo) if todo != 0 => format!(" · {} of {} sounds", count(status.done.unwrap_or(0)), count(todo)),
            _ => "…".to_string(),
        };
        return Some(format!("Learning your library in the background{progress}"));
    }
    let known = format!("Your library: {} sounds · {} presets · {} Sets", count(status.sounds), count(status.presets), count(status.sets));
    Some(match status.todo {
        Some(todo) if status.state == LibraryState::Learning && todo != 0 => {
            format!("{known} · learning {} new", locale_count(todo as i64 - status.done.unwrap_or(0) as i64))
        }
        _ => known,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule `held` keeps, as it was first written: each length from the longest down, built and compared.
    fn held_by_search(text: &str, secret: &str) -> usize {
        let mut length = (js::string::utf16_len(secret).saturating_sub(1)).min(js::string::utf16_len(text));
        while length > 0 {
            if String::from_utf16(&secret.encode_utf16().take(length).collect::<Vec<_>>())
                .ok()
                .is_some_and(|prefix| text.ends_with(&prefix))
            {
                return length;
            }
            length -= 1;
        }
        0
    }

    #[test]
    fn a_secrets_start_at_the_end_is_found_in_one_pass_as_the_search_found_it() {
        // Repeats, accents and surrogate pairs (🎹 is two units), so starts overlap and can end mid-character.
        let pieces = ["a", "b", "ab", "aa", "é", "🎹", "🎹a", "a🎹"];
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..5000 {
            let mut make = |count: u64| (0..next() % count).map(|_| pieces[(next() % pieces.len() as u64) as usize]).collect::<String>();
            let (secret, text) = (make(8), make(10));
            let units: Vec<u16> = text.encode_utf16().collect();
            assert_eq!(Pattern::new(&secret).held(&units), held_by_search(&text, &secret), "{text:?} ends with a start of {secret:?}");
        }
        // A long token: a near miss everywhere is still one pass.
        let token = format!("{}b", "a".repeat(4095));
        let text = "a".repeat(100_000);
        assert_eq!(Pattern::new(&token).held(&text.encode_utf16().collect::<Vec<_>>()), 4095);
    }

    #[test]
    fn counts_group_thousands_as_en_us_does() {
        assert_eq!(locale_count(0), "0");
        assert_eq!(locale_count(999), "999");
        assert_eq!(locale_count(1_000), "1,000");
        assert_eq!(locale_count(1_234_567), "1,234,567");
        assert_eq!(locale_count(-1_234), "-1,234");
    }

    #[test]
    fn the_library_line_says_how_learning_goes() {
        let status = |state, todo: Option<usize>, done: Option<usize>, learned_at: Option<i64>| LibraryStatus {
            state,
            sounds: 12_345,
            presets: 678,
            sets: 9,
            todo,
            done,
            learned_at,
        };
        assert_eq!(library_line(None), None);
        assert_eq!(library_line(Some(&status(LibraryState::New, None, None, None))), None);
        assert_eq!(
            library_line(Some(&status(LibraryState::Paused, Some(4), None, None))).as_deref(),
            Some("Learning your library · paused while Live plays")
        );
        assert_eq!(
            library_line(Some(&status(LibraryState::Learning, None, None, None))).as_deref(),
            Some("Learning your library in the background…")
        );
        assert_eq!(
            library_line(Some(&status(LibraryState::Learning, Some(2_500), Some(1_000), None))).as_deref(),
            Some("Learning your library in the background · 1,000 of 2,500 sounds")
        );
        assert_eq!(
            library_line(Some(&status(LibraryState::Ready, None, None, Some(1)))).as_deref(),
            Some("Your library: 12,345 sounds · 678 presets · 9 Sets")
        );
        assert_eq!(
            library_line(Some(&status(LibraryState::Learning, Some(30), Some(10), Some(1)))).as_deref(),
            Some("Your library: 12,345 sounds · 678 presets · 9 Sets · learning 20 new")
        );
    }

    #[test]
    fn web_words_say_what_was_searched_and_read() {
        let clean = |text: &str, _max: usize| text.to_string();
        let searched = WebEvent {
            action: WebAction::Searched,
            title: "sidechain compression".to_string(),
            url: None,
            r#where: Some(WebWhere::Web),
            via: None,
            results: Some(8),
            kind: None,
            files: None,
        };
        assert_eq!(
            web_words(&searched, &clean),
            WebWords {
                lead: "Searched the web for".to_string(),
                title: "“sidechain compression”".to_string(),
                detail: "8 results".to_string()
            }
        );
        let github = WebEvent { r#where: Some(WebWhere::Github), results: Some(1), ..searched.clone() };
        assert_eq!(web_words(&github, &clean).lead, "Searched GitHub for");
        assert_eq!(web_words(&github, &clean).detail, "1 repository");
        assert_eq!(web_words(&WebEvent { results: None, ..searched.clone() }, &clean).detail, "nothing found");
        let read = WebEvent {
            action: WebAction::Read,
            title: "Compressor basics".to_string(),
            url: Some("https://www.example.com/guides/compressor".to_string()),
            r#where: None,
            via: None,
            results: None,
            kind: Some("a page".to_string()),
            files: None,
        };
        assert_eq!(
            web_words(&read, &clean),
            WebWords { lead: "Read".to_string(), title: "“Compressor basics”".to_string(), detail: "example.com".to_string() }
        );
        let repository = WebEvent {
            title: "https://github.com/user1303836/kumi".to_string(),
            url: Some("https://github.com/user1303836/kumi".to_string()),
            kind: Some("a GitHub repository".to_string()),
            files: Some(1),
            ..read.clone()
        };
        assert_eq!(
            web_words(&repository, &clean),
            WebWords {
                lead: "Read".to_string(),
                title: "github.com/user1303836/kumi".to_string(),
                detail: "a GitHub repository · 1 file".to_string()
            }
        );
        assert_eq!(web_words(&WebEvent { url: None, title: String::new(), ..read.clone() }, &clean).title, "");
    }
}
