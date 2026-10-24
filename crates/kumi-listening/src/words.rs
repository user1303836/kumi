//! What words mean to the style model: LAION-CLAP's text side, which puts a text on the same map as the songs its audio
//! side heard, so "melancholic dub techno" finds the songs that sound like it. Its tokenizer is RoBERTa's byte-level BPE,
//! read from the vocabulary and merges the model came with. The model and its files are fetched the first time they're
//! needed, as the audio side is.

use fancy_regex::Regex;
use kumi_ai::models::{self, pinned, Ids, Say};
use kumi_common::abort::Signal;
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, LazyLock, Mutex},
};

/// A text's tokens at most, its first and last among them.
const MOST_TOKENS: usize = 512;
/// RoBERTa's `<s>` and `</s>`.
const FIRST: i64 = 0;
const LAST: i64 = 2;

/// How GPT-2 (and so RoBERTa) cuts a text before its pieces are merged: contractions, runs of letters, of digits, of
/// anything else, each with the space before it, and runs of spaces.
static PIECES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+").unwrap());

/// Byte-level BPE's alphabet: each byte as a printable character (the printable ASCII and Latin-1 ones as themselves,
/// the rest moved past 255).
fn byte_letters() -> [char; 256] {
    let printable = |byte: u32| (0x21..=0x7e).contains(&byte) || (0xa1..=0xac).contains(&byte) || (0xae..=0xff).contains(&byte);
    let mut letters = ['\0'; 256];
    let mut moved = 0;
    for byte in 0..256u32 {
        letters[byte as usize] = if printable(byte) {
            char::from_u32(byte).unwrap()
        } else {
            moved += 1;
            char::from_u32(255 + moved).unwrap()
        };
    }
    letters
}

/// RoBERTa's tokenizer: a text as the text side's token ids.
pub struct Tokenizer {
    ids: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    letters: [char; 256],
}

impl Tokenizer {
    /// From the vocabulary (JSON: token to id) and the merges (a line a merge, best first, under a version line).
    pub fn new(vocab: &str, merges: &str) -> Result<Tokenizer, String> {
        let ids: HashMap<String, i64> = serde_json::from_str(vocab).map_err(|error| format!("the tokenizer's vocabulary: {error}"))?;
        let ranks = merges
            .lines()
            .filter(|line| !line.starts_with("#version") && !line.trim().is_empty())
            .enumerate()
            .filter_map(|(rank, line)| line.split_once(' ').map(|(a, b)| ((a.to_string(), b.to_string()), rank)))
            .collect();
        Ok(Tokenizer { ids, ranks, letters: byte_letters() })
    }

    /// A text's token ids, `<s>` first and `</s>` last, at most `MOST_TOKENS`.
    pub fn encode(&self, text: &str) -> Vec<i64> {
        let mut encoded = vec![FIRST];
        for piece in PIECES.find_iter(text).filter_map(Result::ok) {
            let letters: String = piece.as_str().bytes().map(|byte| self.letters[byte as usize]).collect();
            encoded.extend(self.merged(&letters).iter().filter_map(|token| self.ids.get(token)));
        }
        encoded.truncate(MOST_TOKENS - 1);
        encoded.push(LAST);
        encoded
    }

    /// A piece's tokens: its letters, the best-ranked pair of neighbours merged again and again while any pair is a merge.
    fn merged(&self, piece: &str) -> Vec<String> {
        let mut tokens: Vec<String> = piece.chars().map(String::from).collect();
        while tokens.len() > 1 {
            let best = (0..tokens.len() - 1)
                .filter_map(|at| self.ranks.get(&(tokens[at].clone(), tokens[at + 1].clone())).map(|rank| (*rank, at)))
                .min();
            let Some((_, at)) = best else { break };
            let (left, right) = (tokens[at].clone(), tokens[at + 1].clone());
            // Every place that pair stands, left to right.
            let mut merged = Vec::with_capacity(tokens.len());
            let mut index = 0;
            while index < tokens.len() {
                if index + 1 < tokens.len() && tokens[index] == left && tokens[index + 1] == right {
                    merged.push(format!("{left}{right}"));
                    index += 2;
                } else {
                    merged.push(tokens[index].clone());
                    index += 1;
                }
            }
            tokens = merged;
        }
        tokens
    }
}

/// The tokenizer, read once.
static TOKENIZER: Mutex<Option<Arc<Tokenizer>>> = Mutex::new(None);

/// The text side and its tokenizer, ready: the runtime, the model and its files fetched the first time.
async fn model(say: Say<'_>, signal: &Signal) -> Result<(std::path::PathBuf, Arc<Tokenizer>), String> {
    models::runtime(say, signal).await?;
    let dir = models::dir();
    let model = dir.join(pinned::CLAP_TEXT.name);
    models::fetch(&pinned::CLAP_TEXT, &model, "its words model, LAION-CLAP's text side", say, signal).await?;
    if let Some(tokenizer) = TOKENIZER.lock().ok().and_then(|held| held.clone()) {
        return Ok((model, tokenizer));
    }
    let (vocab, merges) = (dir.join(pinned::CLAP_VOCAB.name), dir.join(pinned::CLAP_MERGES.name));
    models::fetch(&pinned::CLAP_VOCAB, &vocab, "its words model's vocabulary", say, signal).await?;
    models::fetch(&pinned::CLAP_MERGES, &merges, "its words model's merges", say, signal).await?;
    let read = |path: &Path| std::fs::read_to_string(path).map_err(|error| error.to_string());
    let tokenizer = Arc::new(Tokenizer::new(&read(&vocab)?, &read(&merges)?)?);
    if let Ok(mut held) = TOKENIZER.lock() {
        *held = Some(tokenizer.clone());
    }
    Ok((model, tokenizer))
}

/// What words mean to the style model (Kumi's own, the one the text side matches): a unit 512-number vector on the map
/// its audio side puts songs on.
pub async fn words(text: &str, say: Say<'_>, signal: &Signal) -> Result<Vec<f32>, String> {
    let (model, tokenizer) = model(say, signal).await?;
    let ids = tokenizer.encode(text);
    let count = ids.len();
    let inputs = vec![
        ("input_ids".to_string(), Ids { shape: vec![1, count], data: ids }),
        ("attention_mask".to_string(), Ids { shape: vec![1, count], data: vec![1; count] }),
    ];
    let out = models::run_ids(&model, inputs, vec!["text_embeds".into()]).await?;
    let vector = out.into_iter().next().map(|tensor| tensor.data).unwrap_or_default();
    if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
        return Err("the words model gave no vector".into());
    }
    Ok(crate::embed::normalized(vector))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_byte_has_a_letter_of_its_own() {
        let letters = byte_letters();
        assert_eq!(letters[b'a' as usize], 'a');
        assert_eq!(letters[b' ' as usize], 'Ġ', "a space reads as RoBERTa's Ġ");
        assert_eq!(letters.iter().collect::<std::collections::HashSet<_>>().len(), 256);
    }

    #[test]
    fn pieces_keep_the_space_before_them() {
        let pieces: Vec<&str> = PIECES.find_iter("it's  a test!!  ").filter_map(Result::ok).map(|piece| piece.as_str()).collect();
        assert_eq!(pieces, ["it", "'s", " ", " a", " test", "!!", "  "]);
    }
}
