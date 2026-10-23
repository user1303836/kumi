//! CBOR (RFC 8949), read and written back exactly. Serum 2 keeps a preset's body in it. Every value keeps how
//! it was written: the width of its head, a float's precision and bits, and whether a container has a definite
//! length. So decoding and encoding again gives the same bytes, and a later writer changes only what it means
//! to. An indefinite-length string's chunk boundaries are the one thing not kept (it is written back as one chunk).

use super::tree::Node;
use super::FormatError;

/// How the argument in an item's head was written: in the initial byte (values under 24), or in the 1, 2, 4
/// or 8 bytes after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    Inline,
    U8,
    U16,
    U32,
    U64,
}

impl Head {
    /// The narrowest head that holds `n`, as most encoders write it.
    pub fn shortest(n: u64) -> Head {
        match n {
            0..24 => Head::Inline,
            24..=0xff => Head::U8,
            0x100..=0xffff => Head::U16,
            0x1_0000..=0xffff_ffff => Head::U32,
            _ => Head::U64,
        }
    }

    fn holds(self, n: u64) -> bool {
        match self {
            Head::Inline => n < 24,
            Head::U8 => n <= 0xff,
            Head::U16 => n <= 0xffff,
            Head::U32 => n <= 0xffff_ffff,
            Head::U64 => true,
        }
    }
}

/// A string's or container's length: written in its head, or open and closed by a break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    Definite(Head),
    Indefinite,
}

impl Length {
    fn shortest(n: usize) -> Length {
        Length::Definite(Head::shortest(n as u64))
    }
}

/// A float as written: its precision and exact bits, so -0.0 and NaN payloads survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Float {
    Half(u16),
    Single(u32),
    Double(u64),
}

impl Float {
    pub fn value(self) -> f64 {
        match self {
            Float::Half(bits) => half_to_f64(bits),
            Float::Single(bits) => f32::from_bits(bits) as f64,
            Float::Double(bits) => f64::from_bits(bits),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Unsigned(u64, Head),
    /// The integer -1 - n.
    Negative(u64, Head),
    Bytes(Vec<u8>, Length),
    Text(String, Length),
    Array(Vec<Value>, Length),
    /// Keys in the order written.
    Map(Vec<(Value, Value)>, Length),
    Tag(u64, Head, Box<Value>),
    /// false (20), true (21), null (22), undefined (23) and the other simple values.
    Simple(u8, Head),
    Float(Float),
}

impl Value {
    /// A text value written the shortest way.
    pub fn text(text: impl Into<String>) -> Value {
        let text = text.into();
        let length = Length::shortest(text.len());
        Value::Text(text, length)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(text, _) => Some(text),
            _ => None,
        }
    }

    /// An integer or float as f64.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Unsigned(n, _) => Some(*n as f64),
            Value::Negative(n, _) => Some(-1.0 - *n as f64),
            Value::Float(float) => Some(float.value()),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Unsigned(n, _) => i64::try_from(*n).ok(),
            Value::Negative(n, _) => i64::try_from(*n).ok().map(|n| -1 - n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Simple(20, _) => Some(false),
            Value::Simple(21, _) => Some(true),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items, _) => Some(items),
            _ => None,
        }
    }

    /// A map's entries whose keys are text, in order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Value)> {
        let entries: &[(Value, Value)] = match self {
            Value::Map(entries, _) => entries,
            _ => &[],
        };
        entries.iter().filter_map(|(key, value)| key.as_str().map(|key| (key, value)))
    }

    /// A map's value under a text key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries().find(|(k, _)| *k == key).map(|(_, value)| value)
    }

    /// The value at a path of text keys and array indices ("Oscillator0/WTOsc0/plainParams").
    pub fn at(&self, path: &str) -> Option<&Value> {
        path.split('/').filter(|part| !part.is_empty()).try_fold(self, |value, part| match value {
            Value::Array(items, _) => part.parse::<usize>().ok().and_then(|index| items.get(index)),
            _ => value.get(part),
        })
    }

    /// The format-neutral tree surveys and descriptions read. Non-text map keys are written as CBOR
    /// diagnostic-like text; tags are dropped for their content.
    pub fn to_node(&self) -> Node {
        match self {
            Value::Unsigned(n, _) => Node::Int(*n as i128),
            Value::Negative(n, _) => Node::Int(-1 - *n as i128),
            Value::Bytes(bytes, _) => Node::Bytes(bytes.len()),
            Value::Text(text, _) => Node::Text(text.clone()),
            Value::Array(items, _) => Node::List(items.iter().map(Value::to_node).collect()),
            Value::Map(entries, _) => Node::Map(
                entries
                    .iter()
                    .map(|(key, value)| {
                        let key = match key {
                            Value::Text(text, _) => text.clone(),
                            other => match other.to_node() {
                                Node::Int(n) => n.to_string(),
                                node => format!("{node:?}"),
                            },
                        };
                        (key, value.to_node())
                    })
                    .collect(),
            ),
            Value::Tag(_, _, inner) => inner.to_node(),
            Value::Simple(20, _) => Node::Bool(false),
            Value::Simple(21, _) => Node::Bool(true),
            Value::Simple(_, _) => Node::Null,
            Value::Float(float) => Node::Float(float.value()),
        }
    }
}

/// How deep containers may nest before decoding gives up.
const MAX_DEPTH: usize = 256;

/// One CBOR item that fills `bytes` exactly.
pub fn decode(bytes: &[u8]) -> Result<Value, FormatError> {
    let mut reader = Reader { bytes, at: 0 };
    let value = reader.item(0)?;
    if reader.at != bytes.len() {
        return Err(FormatError::new(format!("CBOR: {} bytes after the item", bytes.len() - reader.at)));
    }
    Ok(value)
}

/// The bytes of `value`, written as it says it was written.
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out, value);
    out
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

enum Arg {
    Value(u64, Head),
    Indefinite,
}

impl Reader<'_> {
    fn fail<T>(&self, what: &str) -> Result<T, FormatError> {
        Err(FormatError::new(format!("CBOR at byte {}: {what}", self.at)))
    }

    fn take(&mut self, n: usize) -> Result<&[u8], FormatError> {
        if self.bytes.len() - self.at < n {
            return self.fail("ends early");
        }
        let slice = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(slice)
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn arg(&mut self, info: u8) -> Result<Arg, FormatError> {
        Ok(match info {
            0..24 => Arg::Value(info as u64, Head::Inline),
            24 => Arg::Value(self.take(1)?[0] as u64, Head::U8),
            25 => Arg::Value(u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64, Head::U16),
            26 => Arg::Value(u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64, Head::U32),
            27 => Arg::Value(u64::from_be_bytes(self.take(8)?.try_into().unwrap()), Head::U64),
            31 => Arg::Indefinite,
            _ => return self.fail("reserved additional information"),
        })
    }

    fn length(&self, n: u64, per_item: usize) -> Result<usize, FormatError> {
        match usize::try_from(n) {
            Ok(n) if n.saturating_mul(per_item) <= self.remaining() => Ok(n),
            _ => self.fail("a length longer than the data"),
        }
    }

    fn is_break(&mut self) -> Result<bool, FormatError> {
        match self.bytes.get(self.at) {
            Some(0xff) => {
                self.at += 1;
                Ok(true)
            }
            Some(_) => Ok(false),
            None => self.fail("ends inside an indefinite-length item"),
        }
    }

    fn item(&mut self, depth: usize) -> Result<Value, FormatError> {
        if depth > MAX_DEPTH {
            return self.fail("nests too deep");
        }
        let initial = self.take(1)?[0];
        let (major, info) = (initial >> 5, initial & 0x1f);
        if major == 7 {
            return self.simple_or_float(info);
        }
        let arg = self.arg(info)?;
        match (major, arg) {
            (0, Arg::Value(n, head)) => Ok(Value::Unsigned(n, head)),
            (1, Arg::Value(n, head)) => Ok(Value::Negative(n, head)),
            (2, Arg::Value(n, head)) => {
                let n = self.length(n, 1)?;
                Ok(Value::Bytes(self.take(n)?.to_vec(), Length::Definite(head)))
            }
            (2, Arg::Indefinite) => Ok(Value::Bytes(self.chunks(2)?, Length::Indefinite)),
            (3, Arg::Value(n, head)) => {
                let n = self.length(n, 1)?;
                let text = String::from_utf8(self.take(n)?.to_vec()).or_else(|_| self.fail("text that isn't UTF-8"))?;
                Ok(Value::Text(text, Length::Definite(head)))
            }
            (3, Arg::Indefinite) => {
                let bytes = self.chunks(3)?;
                let text = String::from_utf8(bytes).or_else(|_| self.fail("text that isn't UTF-8"))?;
                Ok(Value::Text(text, Length::Indefinite))
            }
            (4, Arg::Value(n, head)) => {
                let n = self.length(n, 1)?;
                let items = (0..n).map(|_| self.item(depth + 1)).collect::<Result<_, _>>()?;
                Ok(Value::Array(items, Length::Definite(head)))
            }
            (4, Arg::Indefinite) => {
                let mut items = Vec::new();
                while !self.is_break()? {
                    items.push(self.item(depth + 1)?);
                }
                Ok(Value::Array(items, Length::Indefinite))
            }
            (5, Arg::Value(n, head)) => {
                let n = self.length(n, 2)?;
                let entries = (0..n).map(|_| Ok((self.item(depth + 1)?, self.item(depth + 1)?))).collect::<Result<_, FormatError>>()?;
                Ok(Value::Map(entries, Length::Definite(head)))
            }
            (5, Arg::Indefinite) => {
                let mut entries = Vec::new();
                while !self.is_break()? {
                    entries.push((self.item(depth + 1)?, self.item(depth + 1)?));
                }
                Ok(Value::Map(entries, Length::Indefinite))
            }
            (6, Arg::Value(tag, head)) => Ok(Value::Tag(tag, head, Box::new(self.item(depth + 1)?))),
            _ => self.fail("an indefinite length where none is allowed"),
        }
    }

    /// An indefinite-length string's chunks, joined; each chunk is a definite string of the same major type.
    fn chunks(&mut self, major: u8) -> Result<Vec<u8>, FormatError> {
        let mut joined = Vec::new();
        while !self.is_break()? {
            let initial = self.take(1)?[0];
            if initial >> 5 != major {
                return self.fail("a chunk of another type inside an indefinite-length string");
            }
            match self.arg(initial & 0x1f)? {
                Arg::Value(n, _) => {
                    let n = self.length(n, 1)?;
                    joined.extend_from_slice(self.take(n)?);
                }
                Arg::Indefinite => return self.fail("an indefinite chunk inside an indefinite-length string"),
            }
        }
        Ok(joined)
    }

    fn simple_or_float(&mut self, info: u8) -> Result<Value, FormatError> {
        Ok(match info {
            0..24 => Value::Simple(info, Head::Inline),
            24 => {
                let n = self.take(1)?[0];
                if n < 32 {
                    return self.fail("a two-byte simple value under 32");
                }
                Value::Simple(n, Head::U8)
            }
            25 => Value::Float(Float::Half(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))),
            26 => Value::Float(Float::Single(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))),
            27 => Value::Float(Float::Double(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))),
            31 => return self.fail("a break outside an indefinite-length item"),
            _ => return self.fail("reserved additional information"),
        })
    }
}

fn write_head(out: &mut Vec<u8>, major: u8, n: u64, head: Head) {
    let head = if head.holds(n) { head } else { Head::shortest(n) };
    let major = major << 5;
    match head {
        Head::Inline => out.push(major | n as u8),
        Head::U8 => out.extend_from_slice(&[major | 24, n as u8]),
        Head::U16 => {
            out.push(major | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        Head::U32 => {
            out.push(major | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        Head::U64 => {
            out.push(major | 27);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

fn write_string(out: &mut Vec<u8>, major: u8, bytes: &[u8], length: Length) {
    match length {
        Length::Definite(head) => {
            write_head(out, major, bytes.len() as u64, head);
            out.extend_from_slice(bytes);
        }
        Length::Indefinite => {
            out.push(major << 5 | 31);
            if !bytes.is_empty() {
                write_head(out, major, bytes.len() as u64, Head::shortest(bytes.len() as u64));
                out.extend_from_slice(bytes);
            }
            out.push(0xff);
        }
    }
}

fn write(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Unsigned(n, head) => write_head(out, 0, *n, *head),
        Value::Negative(n, head) => write_head(out, 1, *n, *head),
        Value::Bytes(bytes, length) => write_string(out, 2, bytes, *length),
        Value::Text(text, length) => write_string(out, 3, text.as_bytes(), *length),
        Value::Array(items, length) => {
            match length {
                Length::Definite(head) => write_head(out, 4, items.len() as u64, *head),
                Length::Indefinite => out.push(4 << 5 | 31),
            }
            items.iter().for_each(|item| write(out, item));
            if *length == Length::Indefinite {
                out.push(0xff);
            }
        }
        Value::Map(entries, length) => {
            match length {
                Length::Definite(head) => write_head(out, 5, entries.len() as u64, *head),
                Length::Indefinite => out.push(5 << 5 | 31),
            }
            for (key, value) in entries {
                write(out, key);
                write(out, value);
            }
            if *length == Length::Indefinite {
                out.push(0xff);
            }
        }
        Value::Tag(tag, head, inner) => {
            write_head(out, 6, *tag, *head);
            write(out, inner);
        }
        Value::Simple(n, head) => match head {
            Head::Inline if *n < 24 => out.push(7 << 5 | n),
            _ => out.extend_from_slice(&[7 << 5 | 24, *n]),
        },
        Value::Float(Float::Half(bits)) => {
            out.push(7 << 5 | 25);
            out.extend_from_slice(&bits.to_be_bytes());
        }
        Value::Float(Float::Single(bits)) => {
            out.push(7 << 5 | 26);
            out.extend_from_slice(&bits.to_be_bytes());
        }
        Value::Float(Float::Double(bits)) => {
            out.push(7 << 5 | 27);
            out.extend_from_slice(&bits.to_be_bytes());
        }
    }
}

/// IEEE 754 half precision to f64.
fn half_to_f64(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (bits >> 10) & 0x1f;
    let fraction = (bits & 0x3ff) as f64;
    sign * match exponent {
        0 => fraction * 2f64.powi(-24),
        31 if fraction == 0.0 => f64::INFINITY,
        31 => f64::NAN,
        _ => (1.0 + fraction / 1024.0) * 2f64.powi(exponent as i32 - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(hex: &str) -> Value {
        let bytes = hex::decode(hex).unwrap();
        let value = decode(&bytes).unwrap();
        assert_eq!(hex::encode(encode(&value)), hex, "{value:?}");
        value
    }

    #[test]
    fn keeps_how_each_item_was_written() {
        // RFC 8949 appendix A, plus the non-shortest heads an encoder may choose.
        assert_eq!(round_trip("00"), Value::Unsigned(0, Head::Inline));
        assert_eq!(round_trip("1818"), Value::Unsigned(24, Head::U8));
        assert_eq!(round_trip("1900ff").as_i64(), Some(255));
        assert_eq!(round_trip("3863").as_i64(), Some(-100));
        assert_eq!(round_trip("f93c00").as_f64(), Some(1.0));
        assert_eq!(round_trip("fa47c35000").as_f64(), Some(100000.0));
        assert_eq!(round_trip("fb3ff199999999999a").as_f64(), Some(1.1));
        assert_eq!(round_trip("f98000").as_f64(), Some(-0.0));
        assert!(round_trip("f97e00").as_f64().unwrap().is_nan());
        assert_eq!(round_trip("f5").as_bool(), Some(true));
        round_trip("f6");
        round_trip("f820");
        assert_eq!(round_trip("6449455446").as_str(), Some("IETF"));
        assert_eq!(round_trip("780449455446").as_str(), Some("IETF"));
        round_trip("4401020304");
        round_trip("83010203");
        round_trip("9f018202039f0405ffff");
        let map = round_trip("a26161016162820203");
        assert_eq!(map.get("a").and_then(Value::as_i64), Some(1));
        assert_eq!(map.at("b/1").and_then(Value::as_i64), Some(3));
        round_trip("bf61610161629f0203ffff");
        round_trip("c074323031332d30332d32315432303a30343a30305a");
    }

    #[test]
    fn writes_an_indefinite_string_as_one_chunk() {
        let value = decode(&hex::decode("7f657374726561646d696e67ff").unwrap()).unwrap();
        assert_eq!(value.as_str(), Some("streaming"));
        assert_eq!(hex::encode(encode(&value)), "7f6973747265616d696e67ff");
        assert_eq!(decode(&encode(&value)).unwrap(), value);
    }

    #[test]
    fn refuses_what_is_not_one_whole_item() {
        for hex in ["", "18", "62ff", "9f01", "ff", "0000", "1c", "5f01ff", "9a7fffffff", "f818", "7f6161", "62c328"] {
            assert!(decode(&hex::decode(hex).unwrap()).is_err(), "{hex}");
        }
        let deep = "81".repeat(MAX_DEPTH + 2) + "00";
        assert!(decode(&hex::decode(deep).unwrap()).is_err());
    }

    #[test]
    fn a_head_too_narrow_for_its_value_is_widened() {
        let mut out = Vec::new();
        write(&mut out, &Value::Unsigned(300, Head::Inline));
        assert_eq!(hex::encode(&out), "19012c");
        assert_eq!(hex::encode(encode(&Value::text("a"))), "6161");
    }
}
