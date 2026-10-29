//! Kumi's model clients. What `@ai-sdk/provider` and the `@ai-sdk/*` provider packages gave the TypeScript: the
//! LanguageModelV4 types (`types`), `APICallError` (`error`), and the providers' wire bindings
//! (`anthropic`, `openai_responses`, `openai_compatible`), with SSE and HTTP helpers. Beside them: what the agent asks
//! of a model (`binding`), the app's model controls (`control`), the providers and their models (`providers`), signing
//! in (`auth`), the models Kumi runs itself (`models`), where a turn's time went (`timing`), and how a failed call is
//! told and retried (`failure`).

pub mod anthropic;
pub mod auth;
pub mod binding;
pub mod control;
// A model call's error (kumi-net, which classifies why a call got no answer), as ai's own.
pub use kumi_net::error;
pub mod failure;
pub mod http;
pub mod models;
pub mod openai_compatible;
pub mod openai_responses;
pub mod providers;
pub mod sse;
pub mod timing;
pub mod types;

/// Provider-utils' default source identifier: sixteen uniformly chosen alphanumeric characters.
fn generate_id() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut random = rand::rng();
    (0..16).map(|_| ALPHABET[random.random_range(0..ALPHABET.len())] as char).collect()
}
