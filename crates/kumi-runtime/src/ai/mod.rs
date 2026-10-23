//! What `@ai-sdk/provider` and the `@ai-sdk/*` provider packages gave the TypeScript: the
//! LanguageModelV4 types (`types`), `APICallError` (`error`), and the providers' wire bindings
//! (`anthropic`, `openai_responses`, `openai_compatible`), with SSE and HTTP helpers.

pub mod anthropic;
// A model call's error (kumi-net, which classifies why a call got no answer), as ai's own.
pub use kumi_net::error;
pub mod http;
pub mod openai_compatible;
pub mod openai_responses;
pub mod sse;
pub mod types;

/// Provider-utils' default source identifier: sixteen uniformly chosen alphanumeric characters.
fn generate_id() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut random = rand::rng();
    (0..16).map(|_| ALPHABET[random.random_range(0..ALPHABET.len())] as char).collect()
}
