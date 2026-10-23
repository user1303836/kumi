//! Shared signed-wire primitives. Each caller applies its source channel's limits.
use crate::live::LiveError;
use crate::registry::{canonical_json, CanonicalError, CanonicalLimits, WIRE_CANONICAL_LIMITS};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
pub(super) const MAX_FRAME_BYTES: usize = 256 * 1_048_576;
pub(super) fn safe_integer(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| kumi_common::js::number::is_safe_integer(*n))
}
pub(super) fn canonical(value: &Value, extension: bool) -> Result<String, LiveError> {
    let extension_limits =
        CanonicalLimits { max_depth: 256, max_string_length: usize::MAX, max_array_length: usize::MAX, max_object_properties: usize::MAX };
    canonical_json(value, if extension { &extension_limits } else { &WIRE_CANONICAL_LIMITS }).map_err(|error| {
        LiveError::error(match error {
            CanonicalError::TooDeep => "wire payload is too deeply nested",
            CanonicalError::StringTooLarge => "wire string is too large",
            CanonicalError::ArrayTooLarge => "wire array is too large",
            CanonicalError::ObjectTooLarge => "wire object is too large",
        })
    })
}
pub(super) fn mac(secret: &str, value: &Value, extension: bool) -> Result<String, LiveError> {
    let encoded = canonical(value, extension)?;
    if !extension && encoded.len() > MAX_FRAME_BYTES {
        return Err(LiveError::error("wire payload is too large"));
    }
    // Every frame is hashed whole (megabytes for a large read): ring's HMAC-SHA256 uses the CPU's SHA
    // instructions (ARMv8, SHA-NI), where sha2 0.10 hashes in software on ARM.
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(ring::hmac::sign(&key, encoded.as_bytes()).as_ref()))
}
pub(super) fn signed(secret: &str, mut value: Value, extension: bool) -> Result<Value, LiveError> {
    value["mac"] = mac(secret, &value, extension)?.into();
    Ok(value)
}
/// Whether a received frame carries the right `mac`, which this takes out of it: nothing reads the mac
/// afterwards, and a frame can be megabytes (a snapshot), so it isn't copied to check it.
pub(super) fn verify(secret: &str, value: &mut Value, extension: bool) -> Result<bool, LiveError> {
    let Some(row) = value.as_object_mut() else {
        return Ok(false);
    };
    let Some(Value::String(received)) = row.remove("mac") else {
        return Ok(false);
    };
    Ok(mac(secret, value, extension)?.as_bytes().ct_eq(received.as_bytes()).into())
}
pub(super) fn random_id() -> String {
    let mut bytes = [0; 18];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
pub(super) fn digest(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical(value, false)?.as_bytes())))
}

/// JSON.parse accepts wire frames deeper than serde_json's default 128-level limit.
/// Scan nesting before disabling that default: canonical signing permits depth 256,
/// including an empty container at that depth, but never an unbounded Rust stack.
pub(super) fn parse(bytes: &[u8]) -> Result<Value, LiveError> {
    let text = String::from_utf8_lossy(bytes);
    let mut in_string = false;
    let mut escaped = false;
    let mut nesting = 0usize;
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'[' | b'{' => {
                    nesting += 1;
                    if nesting > 257 {
                        return Err(LiveError::error("wire payload is too deeply nested"));
                    }
                }
                b']' | b'}' => nesting = nesting.saturating_sub(1),
                _ => {}
            }
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    deserializer.disable_recursion_limit();
    let value = Value::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parsing_honors_the_256_level_wire_bound_beyond_serdes_default() {
        for depth in [128, 129, 255, 256] {
            let text = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            let value = parse(text.as_bytes()).unwrap();
            assert!(canonical(&value, false).is_ok(), "depth {depth}");
        }
        let empty_at_bound = format!("{}{}", "[".repeat(257), "]".repeat(257));
        assert!(canonical(&parse(empty_at_bound.as_bytes()).unwrap(), false).is_ok());
        let leaf_past_bound = format!("{}0{}", "[".repeat(257), "]".repeat(257));
        assert_eq!(
            canonical(&parse(leaf_past_bound.as_bytes()).unwrap(), false).unwrap_err().message(),
            "wire payload is too deeply nested"
        );
        let excessive = format!("{}0{}", "[".repeat(258), "]".repeat(258));
        assert_eq!(parse(excessive.as_bytes()).unwrap_err().message(), "wire payload is too deeply nested");
    }
    #[test]
    fn nesting_scan_ignores_escaped_string_contents_and_parser_still_checks_syntax() {
        let value = serde_json::json!({"text":format!("{}\"{{[\\]}}", "[".repeat(1000))});
        assert_eq!(parse(serde_json::to_string(&value).unwrap().as_bytes()).unwrap(), value);
        assert!(parse(b"[1,]").is_err());
        assert!(parse(b"{} {}").is_err());
        assert_eq!(parse(b"\"\xff\"").unwrap(), Value::String("\u{fffd}".into()));
    }
}
