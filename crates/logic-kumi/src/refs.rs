//! Refs and cursors. Logic gives nothing a stable id, so a ref is a position under an epoch, as Renoise's:
//! `<epoch>:<kind>:<n>` for a track (its place in the Tracks area, from 1), a region (its place through the project,
//! track by track) and a marker (its place in the Marker List); `<epoch>:<kind>:<parent>.<n>` for a note under its
//! region, a plug-in and a send under their track. The epoch moves on every track or region change the bridge sees
//! (a header appeared, vanished, was renamed or moved; a region added, removed or moved), and every ref, parent,
//! around or cursor from an earlier epoch is refused as `stale`, whatever it was for. A cursor is `<epoch>.<n>`, the
//! position a page goes on from.

use serde_json::Value;

/// A ref taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub epoch: u32,
    pub kind: String,
    pub numbers: Vec<usize>,
}

impl Ref {
    pub fn new(epoch: u32, kind: &str, numbers: &[usize]) -> Ref {
        Ref { epoch, kind: kind.into(), numbers: numbers.to_vec() }
    }

    /// `1:track:2`, `1:note:1.3`.
    pub fn parse(text: &str) -> Option<Ref> {
        let mut parts = text.splitn(3, ':');
        let (epoch, kind, numbers) = (parts.next()?, parts.next()?, parts.next()?);
        if kind.is_empty() || !kind.bytes().all(|b| b.is_ascii_lowercase()) {
            return None;
        }
        let epoch = parse_number(epoch)? as u32;
        let numbers: Option<Vec<usize>> = numbers.split('.').map(parse_number).collect();
        let numbers = numbers?;
        (!numbers.is_empty()).then_some(Ref { epoch, kind: kind.into(), numbers })
    }

    pub fn text(&self) -> String {
        let numbers: Vec<String> = self.numbers.iter().map(usize::to_string).collect();
        format!("{}:{}:{}", self.epoch, self.kind, numbers.join("."))
    }

    /// The first number: a track's, region's or marker's place; a note's region; a plug-in's or send's track.
    pub fn first(&self) -> usize {
        self.numbers[0]
    }

    /// The second number, for a ref under a parent.
    pub fn second(&self) -> Option<usize> {
        self.numbers.get(1).copied()
    }
}

fn parse_number(text: &str) -> Option<usize> {
    (!text.is_empty() && text.len() <= 9 && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

/// The epoch a ref (`<epoch>:<kind>:<n>`) or a cursor (`<epoch>.<n>`) names, if the text is one.
pub fn epoch_of(text: &str) -> Option<u32> {
    let end = text.find([':', '.'])?;
    parse_number(&text[..end]).map(|n| n as u32)
}

/// A page's cursor: where the next page starts.
pub fn cursor(epoch: u32, start: usize) -> String {
    format!("{epoch}.{start}")
}

/// Where a cursor points: its epoch and position, if the text is one.
pub fn parse_cursor(text: &str) -> Option<(u32, usize)> {
    let (epoch, position) = text.split_once('.')?;
    Some((parse_number(epoch)? as u32, parse_number(position)?))
}

/// An error as the wire carries it: a code and its message.
pub type Fault = (String, String);

pub fn bad(message: impl Into<String>) -> Fault {
    (daw_bridge::wire::code::BAD_PARAMS.into(), message.into())
}

pub fn failed(message: impl Into<String>) -> Fault {
    (daw_bridge::wire::code::FAILED.into(), message.into())
}

/// The refusal of a ref or cursor from another epoch, in the words the host's simulator uses.
pub fn stale(key: &str, from: u32, now: u32) -> Fault {
    let (what, then) = if key == "cursor" { ("cursor", "read again from the start") } else { ("ref", "read again") };
    (daw_bridge::wire::code::STALE.into(), format!("that {what} is from an earlier epoch ({from}; now {now}): {then}"))
}

/// The first of `ref`, `parent`, `around` and `cursor` in a request that names another epoch than `now`: refused
/// whatever the request was for.
pub fn check_epoch(params: &Value, now: u32) -> Result<(), Fault> {
    for key in ["ref", "parent", "around", "cursor"] {
        let Some(from) = params.get(key).and_then(Value::as_str).and_then(epoch_of).filter(|from| *from != now) else { continue };
        return Err(stale(key, from, now));
    }
    Ok(())
}

/// The row a ref names isn't there.
pub fn not_in_project(reference: &str) -> Fault {
    failed(format!("{reference} isn't in the project (deleted, or another project)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refs_are_positions_under_an_epoch_and_cursors_positions_in_a_page() {
        let note = Ref::parse("3:note:1.4").unwrap();
        assert_eq!((note.epoch, note.kind.as_str(), note.numbers.as_slice()), (3, "note", &[1, 4][..]));
        assert_eq!(note.text(), "3:note:1.4");
        assert_eq!((note.first(), note.second()), (1, Some(4)));
        assert_eq!(Ref::parse("1:track:2").unwrap().second(), None);
        assert_eq!(Ref::new(1, "track", &[2]).text(), "1:track:2");
        for bad in ["track:2", "1:track:", "1::2", "1:Track:2", "a:track:2", "1:track:x", "1:track:1.", "", "1:track:1234567890"] {
            assert_eq!(Ref::parse(bad), None, "{bad}");
        }
        assert_eq!(epoch_of("2:track:1"), Some(2));
        assert_eq!(epoch_of("2.17"), Some(2));
        assert_eq!(epoch_of("track:1"), None);
        assert_eq!(epoch_of("x"), None);
        assert_eq!(cursor(1, 50), "1.50");
        assert_eq!(parse_cursor("1.50"), Some((1, 50)));
        assert_eq!(parse_cursor("1"), None);
        assert_eq!(parse_cursor("1.x"), None);
    }

    #[test]
    fn a_ref_or_cursor_from_another_epoch_is_stale_in_the_simulators_words() {
        assert_eq!(
            check_epoch(&json!({"kind": "region", "parent": "0:track:2"}), 1),
            Err(("stale".into(), "that ref is from an earlier epoch (0; now 1): read again".into()))
        );
        assert_eq!(
            check_epoch(&json!({"cursor": "0.3", "ref": "1:track:1"}), 1),
            Err(("stale".into(), "that cursor is from an earlier epoch (0; now 1): read again from the start".into()))
        );
        assert_eq!(check_epoch(&json!({"ref": "1:track:1", "around": "1:track:2"}), 1), Ok(()));
        // Text that isn't a ref names no epoch, so it isn't stale (the read refuses it for what it is).
        assert_eq!(check_epoch(&json!({"ref": "bogus"}), 1), Ok(()));
        assert_eq!(not_in_project("1:track:9").1, "1:track:9 isn't in the project (deleted, or another project)");
        assert_eq!(bad("x").0, "bad-params");
        assert_eq!(failed("x").0, "failed");
    }
}
