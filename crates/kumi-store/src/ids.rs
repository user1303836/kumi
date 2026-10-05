//! Row ids that mean the same on every machine, so what Kumi keeps can move between them.

use std::{
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A new row's id: a ULID, the time in milliseconds then 80 random bits, so ids sort by when they were
/// made. Within one millisecond (or if the clock steps back) each id is the last one plus one, so this
/// process's ids keep their order.
pub fn new_id() -> String {
    static LAST: Mutex<u128> = Mutex::new(0);
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) & ((1 << 48) - 1);
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = if ms > *last >> 80 { ms << 80 | rand::random::<u128>() >> 48 } else { *last + 1 };
    encode(*last)
}

/// An id made from content: the same parts give the same id anywhere, so importing the same row again
/// (or on another machine) changes nothing.
pub fn content_id(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    let hash = hasher.finalize();
    encode(u128::from_be_bytes(hash.as_bytes()[..16].try_into().unwrap()))
}

/// 128 bits as 26 Crockford base-32 digits, as a ULID is written.
fn encode(value: u128) -> String {
    (0..26).map(|digit| CROCKFORD[(value >> (125 - 5 * digit) & 31) as usize] as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ids_are_ulids_that_sort_by_time() {
        let first = new_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = new_id();
        assert_eq!((first.len(), second.len()), (26, 26));
        assert!(first.bytes().all(|b| CROCKFORD.contains(&b)));
        assert!(first < second, "{first} then {second}");
        let many: Vec<String> = (0..1000).map(|_| new_id()).collect();
        assert!(many.windows(2).all(|pair| pair[0] < pair[1]), "ids made in the same millisecond keep their order");
    }

    #[test]
    fn content_ids_depend_on_every_part_and_nothing_else() {
        let id = content_id(&["note", "global", "p1", "1700000000000"]);
        assert_eq!(id, content_id(&["note", "global", "p1", "1700000000000"]));
        assert_ne!(id, content_id(&["note", "global", "p1", "1700000000001"]));
        assert_ne!(content_id(&["ab", "c"]), content_id(&["a", "bc"]), "parts are kept apart");
        assert_ne!(content_id(&["a\0b", "c"]), content_id(&["a", "b\0c"]), "whatever they hold");
        assert_eq!(id.len(), 26);
    }
}
