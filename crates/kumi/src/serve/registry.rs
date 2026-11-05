//! The operations `kumi serve` offers, in the order `tools/list` gives them: one line per module. The runtime core's
//! are W1's; the library, map and file listening (W3) and track listening (W6) add their own modules here, and setup
//! (W5) its own.

use super::change;
use super::core;
use super::ctx::Operation;
use super::{generate, library, listen, map, read, setup};

pub static OPERATIONS: &[&Operation] = &[
    &core::STATUS,
    &core::CONNECT,
    &core::OBSERVE,
    &read::READ,
    &change::CHANGE,
    &change::UNDO,
    &change::HISTORY,
    &core::JOB,
    &setup::SETUP_DAW,
    &library::LIBRARY,
    &map::SOUNDS,
    &listen::LISTEN,
    &generate::GENERATE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_has_its_own_name_and_a_schema_for_an_object() {
        let mut names: Vec<&str> = OPERATIONS.iter().map(|operation| operation.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), OPERATIONS.len(), "{names:?}");
        for operation in OPERATIONS {
            assert!(!operation.name.starts_with("kumi_"), "{}: the host adds the prefix", operation.name);
            let schema = (operation.schema)();
            assert_eq!(schema["type"], "object", "{}", operation.name);
            assert!(!operation.description.is_empty(), "{}", operation.name);
        }
    }
}
