//! The shared secret, connection nonces and the two HMAC-SHA256 proofs of the handshake.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub const MIN_SECRET_BYTES: usize = 32;
pub const MAX_SECRET_BYTES: usize = 512;
/// A nonce is 16 random bytes as lowercase hex.
pub const NONCE_HEX_LEN: usize = 32;
/// A proof is an HMAC-SHA256 as lowercase hex.
pub const PROOF_HEX_LEN: usize = 64;

/// Why a secret was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretError {
    TooShort,
    TooLong,
    BadByte,
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SecretError::TooShort => "the bridge secret is shorter than 32 characters",
            SecretError::TooLong => "the bridge secret is longer than 512 characters",
            SecretError::BadByte => "the bridge secret has a character outside printable ASCII",
        })
    }
}

impl std::error::Error for SecretError {}

/// The owner-only secret both ends prove they know: 32..=512 bytes, each printable ASCII without space
/// (0x21..=0x7E), so every language reads it as the same bytes. Never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(text: impl Into<String>) -> Result<Self, SecretError> {
        let text = text.into();
        if text.len() < MIN_SECRET_BYTES {
            return Err(SecretError::TooShort);
        }
        if text.len() > MAX_SECRET_BYTES {
            return Err(SecretError::TooLong);
        }
        if !text.bytes().all(|byte| (0x21..=0x7E).contains(&byte)) {
            return Err(SecretError::BadByte);
        }
        Ok(Self(text))
    }

    /// A secret file's contents: the secret and at most one line ending.
    pub fn from_file_text(text: &str) -> Result<Self, SecretError> {
        let line = text.strip_suffix('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).unwrap_or(text);
        Self::new(line)
    }

    /// 32 random bytes, base64url without padding (43 characters).
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// 16 random bytes as 32 lowercase hex characters.
pub fn random_hex16() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Whether `text` is a nonce as the wire spells one: exactly 32 lowercase hex characters.
pub fn valid_nonce(text: &str) -> bool {
    text.len() == NONCE_HEX_LEN && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Which end a proof comes from: the bridge proves first, on the host's nonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Server,
    Client,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Server => "server",
            Role::Client => "client",
        }
    }
}

/// The message a proof signs: `kumi-daw/1\n<role>\n<client nonce>\n<server nonce>`.
pub fn proof_message(role: Role, client_nonce: &str, server_nonce: &str) -> String {
    format!("{}\n{}\n{client_nonce}\n{server_nonce}", crate::wire::PROTOCOL, role.label())
}

/// HMAC-SHA256 keyed with the secret's bytes over [`proof_message`], as lowercase hex.
pub fn proof(secret: &Secret, role: Role, client_nonce: &str, server_nonce: &str) -> String {
    hmac_sha256_hex(secret.expose().as_bytes(), proof_message(role, client_nonce, server_nonce).as_bytes())
}

/// Whether `received` is the right proof, compared in constant time.
pub fn verify_proof(secret: &Secret, role: Role, client_nonce: &str, server_nonce: &str, received: &str) -> bool {
    let expected = proof(secret, role, client_nonce, server_nonce);
    received.len() == PROOF_HEX_LEN && bool::from(expected.as_bytes().ct_eq(received.as_bytes()))
}

pub fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(message);
    hex::encode(mac.finalize().into_bytes())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_printable_ascii_of_a_bounded_length() {
        assert_eq!(Secret::new("a".repeat(31)), Err(SecretError::TooShort));
        assert!(Secret::new("a".repeat(32)).is_ok());
        assert_eq!(Secret::new("a".repeat(513)), Err(SecretError::TooLong));
        assert_eq!(Secret::new(format!("{} ", "a".repeat(32))), Err(SecretError::BadByte));
        assert_eq!(Secret::new(format!("{}é", "a".repeat(32))), Err(SecretError::BadByte));
        assert_eq!(Secret::from_file_text(&format!("{}\r\n", "b".repeat(40))).unwrap().expose(), "b".repeat(40));
        assert_eq!(Secret::from_file_text(&format!("{}\n\n", "b".repeat(40))), Err(SecretError::BadByte));
        let generated = Secret::generate();
        assert_eq!(generated.expose().len(), 43);
        assert!(Secret::new(generated.expose()).is_ok());
        assert_eq!(format!("{generated:?}"), "Secret(..)");
    }

    #[test]
    fn proofs_differ_by_role_and_verify_only_exactly() {
        let secret = Secret::new("s".repeat(32)).unwrap();
        let (client, server) = (random_hex16(), random_hex16());
        assert!(valid_nonce(&client) && !valid_nonce(&client.to_uppercase()) && !valid_nonce(&client[1..]));
        let from_server = proof(&secret, Role::Server, &client, &server);
        assert_ne!(from_server, proof(&secret, Role::Client, &client, &server));
        assert!(verify_proof(&secret, Role::Server, &client, &server, &from_server));
        assert!(!verify_proof(&secret, Role::Client, &client, &server, &from_server));
        assert!(!verify_proof(&secret, Role::Server, &server, &client, &from_server));
        assert!(!verify_proof(&secret, Role::Server, &client, &server, &from_server.to_uppercase()));
    }
}
