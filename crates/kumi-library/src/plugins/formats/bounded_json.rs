//! JSON decoding with a budget charged before allocating each value and key.

use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{FormatError, MAX_DECODED_BYTES, MAX_DECODED_DEPTH, MAX_DECODED_ITEMS};

pub fn parse(bytes: &[u8]) -> Result<Value, FormatError> {
    parse_with_limit(bytes, MAX_DECODED_BYTES)
}

/// Some established formats embed audio data in strings. Their historical byte
/// allowance can be retained without relaxing the tree's allocation budget.
pub(crate) fn parse_with_limit(bytes: &[u8], max_bytes: usize) -> Result<Value, FormatError> {
    if bytes.len() > max_bytes {
        return Err(FormatError::new("JSON exceeds the decoded byte budget"));
    }
    let mut remaining = MAX_DECODED_ITEMS;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = Seed { remaining: &mut remaining, depth: 0 }
        .deserialize(&mut decoder)
        .map_err(|error| FormatError::new(format!("JSON: {error}")))?;
    decoder.end().map_err(|error| FormatError::new(format!("JSON: {error}")))?;
    Ok(value)
}

struct Seed<'a> {
    remaining: &'a mut usize,
    depth: usize,
}

fn charge<E: de::Error>(remaining: &mut usize) -> Result<(), E> {
    *remaining = remaining.checked_sub(1).ok_or_else(|| E::custom("decoded item budget exceeded"))?;
    Ok(())
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        if self.depth > MAX_DECODED_DEPTH {
            return Err(de::Error::custom("JSON nesting budget exceeded"));
        }
        charge(self.remaining)?;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a bounded JSON value")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value).map(Value::Number).ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(Seed { remaining: self.remaining, depth: self.depth + 1 })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key_seed(Key(self.remaining))? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            let value = object.next_value_seed(Seed { remaining: self.remaining, depth: self.depth + 1 })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

struct Key<'a>(&'a mut usize);

impl<'de> DeserializeSeed<'de> for Key<'_> {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<String, D::Error> {
        charge(self.0)?;
        serde::Deserialize::deserialize(decoder)
    }
}
