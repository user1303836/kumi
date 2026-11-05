//! The operations `kumi serve` offers, in the order `tools/list` gives them: one line per module. The runtime core's
//! are W1's; the library, map and file listening (W3) and track listening (W6) add their own modules here.

use super::change;
use super::core;
use super::ctx::Operation;

pub static OPERATIONS: &[&Operation] =
    &[&core::STATUS, &core::CONNECT, &core::OBSERVE, &change::CHANGE, &change::UNDO, &change::HISTORY, &core::JOB];

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
            assert_eq!(schema["additionalProperties"], false, "{}: every argument is named", operation.name);
            assert!(!operation.description.is_empty(), "{}", operation.name);
        }
    }
}
