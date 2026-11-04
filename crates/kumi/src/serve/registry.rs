//! The operations `kumi serve` offers, in the order `tools/list` gives them: one line per module. The runtime core's
//! are W1's; the library, map and file listening (W3) and track listening (W6) add their own modules here.

use super::core;
use super::ctx::Operation;

pub static OPERATIONS: &[&Operation] = &[&core::STATUS];

/// The operation by its tool name.
pub fn find(name: &str) -> Option<&'static Operation> {
    OPERATIONS.iter().copied().find(|operation| operation.name == name)
}

/// The tool names offered, for `status`'s `capabilities`.
pub fn names() -> Vec<&'static str> {
    OPERATIONS.iter().map(|operation| operation.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_has_its_own_name_and_a_schema_for_an_object() {
        let names = names();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        for operation in OPERATIONS {
            assert!(!operation.name.starts_with("kumi_"), "{}: the host adds the prefix", operation.name);
            let schema = (operation.schema)();
            assert_eq!(schema["type"], "object", "{}", operation.name);
            assert_eq!(schema["additionalProperties"], false, "{}: every argument is named", operation.name);
            assert!(!operation.description.is_empty(), "{}", operation.name);
        }
    }
}
