//! The little of Open Sound Control that Kumi's listening device speaks: messages of ints, floats and
//! strings, as Max's udpsend and udpreceive send and read them. Big-endian, every part padded to four bytes.

/// An argument to send: a number (whole ones go as ints), a number sent as a float, or text.
#[derive(Debug, Clone, PartialEq)]
pub enum OscArg {
    Number(f64),
    /// `{ float: 2 }`: 2 sent as a float.
    Float(f64),
    Text(String),
}

impl From<f64> for OscArg {
    fn from(value: f64) -> OscArg {
        OscArg::Number(value)
    }
}

impl From<i32> for OscArg {
    fn from(value: i32) -> OscArg {
        OscArg::Number(value as f64)
    }
}

impl From<u16> for OscArg {
    fn from(value: u16) -> OscArg {
        OscArg::Number(value as f64)
    }
}

impl From<&str> for OscArg {
    fn from(value: &str) -> OscArg {
        OscArg::Text(value.to_string())
    }
}

impl From<String> for OscArg {
    fn from(value: String) -> OscArg {
        OscArg::Text(value)
    }
}

/// An argument read back: a number or text.
#[derive(Debug, Clone, PartialEq)]
pub enum OscValue {
    Number(f64),
    Text(String),
}

impl OscValue {
    pub fn as_number(&self) -> Option<f64> {
        match self {
            OscValue::Number(value) => Some(*value),
            OscValue::Text(_) => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            OscValue::Number(_) => None,
            OscValue::Text(text) => Some(text),
        }
    }

    /// `String(value)`.
    pub fn to_js_string(&self) -> String {
        match self {
            OscValue::Number(value) => kumi_common::js::number::to_string(*value),
            OscValue::Text(text) => text.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OscMessage {
    pub address: String,
    pub args: Vec<OscValue>,
}

/// NULs up to a multiple of four (JavaScript's remainder: a negative length pads as its magnitude would).
fn pad(length: i64) -> i64 {
    (4 - (length % 4)) % 4
}

fn osc_string(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    // At least one NUL ends the string, then NULs up to a multiple of four.
    bytes.extend(std::iter::repeat_n(0u8, 1 + pad(bytes.len() as i64 + 1) as usize));
    bytes
}

/// A message: whole numbers go as ints, others as floats (`OscArg::Float(2.0)` sends 2 as a float), text as strings.
pub fn encode_osc(address: &str, args: &[OscArg]) -> Vec<u8> {
    let mut tags = String::from(",");
    let mut parts: Vec<u8> = Vec::new();
    for arg in args {
        match arg {
            OscArg::Text(text) => {
                tags.push('s');
                parts.extend(osc_string(text));
            }
            OscArg::Number(value) if value.fract() == 0.0 && *value >= -2147483648.0 && *value <= 2147483647.0 => {
                tags.push('i');
                parts.extend((*value as i32).to_be_bytes());
            }
            OscArg::Number(value) | OscArg::Float(value) => {
                tags.push('f');
                parts.extend((*value as f32).to_be_bytes());
            }
        }
    }
    let mut packet = osc_string(address);
    packet.extend(osc_string(&tags));
    packet.extend(parts);
    packet
}

/// A message read back; None for anything that isn't one (a bundle's messages come out one by one through decode_osc_packet).
pub fn decode_osc(packet: &[u8]) -> Option<OscMessage> {
    decode_osc_packet(packet).into_iter().next()
}

/// Every message in a packet: one, or a bundle's.
pub fn decode_osc_packet(packet: &[u8]) -> Vec<OscMessage> {
    if packet.len() >= 16 && &packet[0..8] == b"#bundle\0" {
        let mut messages: Vec<OscMessage> = Vec::new();
        let mut at = 16usize;
        while at + 4 <= packet.len() {
            let size = i32::from_be_bytes([packet[at], packet[at + 1], packet[at + 2], packet[at + 3]]);
            at += 4;
            if size <= 0 || at + size as usize > packet.len() {
                break;
            }
            messages.extend(decode_osc_packet(&packet[at..at + size as usize]));
            at += size as usize;
        }
        return messages;
    }
    // A read past the packet's end is a malformed packet: nothing (TS: the thrown RangeError, caught).
    decode_message(packet).unwrap_or_default()
}

fn decode_message(packet: &[u8]) -> Option<Vec<OscMessage>> {
    let length = packet.len() as i64;
    let (address, after_address) = read_string(packet, 0);
    if !address.starts_with('/') {
        return Some(Vec::new());
    }
    if after_address >= length {
        return Some(vec![OscMessage { address, args: Vec::new() }]);
    }
    let (tags, after_tags) = read_string(packet, after_address);
    if !tags.starts_with(',') {
        return Some(vec![OscMessage { address, args: Vec::new() }]);
    }
    let mut args: Vec<OscValue> = Vec::new();
    let mut at = after_tags;
    for tag in tags[1..].chars() {
        match tag {
            'i' => {
                args.push(OscValue::Number(i32::from_be_bytes(read(packet, at)?) as f64));
                at += 4;
            }
            'f' => {
                args.push(OscValue::Number(f32::from_be_bytes(read(packet, at)?) as f64));
                at += 4;
            }
            'h' => {
                args.push(OscValue::Number(i64::from_be_bytes(read(packet, at)?) as f64));
                at += 8;
            }
            'd' => {
                args.push(OscValue::Number(f64::from_be_bytes(read(packet, at)?)));
                at += 8;
            }
            's' | 'S' => {
                let (text, next) = read_string(packet, at);
                args.push(OscValue::Text(text));
                at = next;
            }
            'T' => args.push(OscValue::Number(1.0)),
            'F' | 'N' => args.push(OscValue::Number(0.0)),
            _ => return Some(vec![OscMessage { address, args }]),
        }
    }
    Some(vec![OscMessage { address, args }])
}

/// `N` bytes at `at`, or None past the packet's end.
fn read<const N: usize>(packet: &[u8], at: i64) -> Option<[u8; N]> {
    if at < 0 {
        return None;
    }
    packet.get(at as usize..at as usize + N)?.try_into().ok()
}

/// The string at `from` (up to a NUL, or the packet's end) and where the next part starts.
fn read_string(packet: &[u8], from: i64) -> (String, i64) {
    let length = packet.len() as i64;
    let start = from.clamp(0, length) as usize;
    let end = if from > length {
        length
    } else {
        packet[start..].iter().position(|byte| *byte == 0).map(|at| start as i64 + at as i64).unwrap_or(length)
    };
    let text = if from > end { String::new() } else { String::from_utf8_lossy(&packet[start..end as usize]).into_owned() };
    (text, end + 1 + pad(end + 1 - from))
}
