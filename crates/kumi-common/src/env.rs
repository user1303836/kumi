//! This process's environment and arguments, with a name, value or argument that isn't Unicode read as near as it
//! can be: `std::env::vars()` and `args()` panic on one.

/// `std::env::vars()`, lossily, into any collection of name and value.
pub fn vars<C: FromIterator<(String, String)>>() -> C {
    std::env::vars_os().map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned())).collect()
}

/// `std::env::args()`, lossily.
pub fn args() -> impl Iterator<Item = String> {
    std::env::args_os().map(|arg| arg.to_string_lossy().into_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use std::{collections::HashMap, ffi::OsStr, os::unix::ffi::OsStrExt};

    // The only test in this crate that touches the environment: std's own readers would panic on it meanwhile.
    #[test]
    fn a_value_that_isnt_unicode_is_read_lossily_instead_of_panicking() {
        let name = "KUMI_COMMON_TEST_NOT_UNICODE";
        std::env::set_var(name, OsStr::from_bytes(b"caf\xe9"));
        let read = std::panic::catch_unwind(super::vars::<HashMap<String, String>>);
        let std_panics = std::panic::catch_unwind(|| std::env::vars().count()).is_err();
        std::env::remove_var(name);
        assert!(std_panics, "std::env::vars() panics on it");
        assert_eq!(read.expect("read without panicking").get(name).map(String::as_str), Some("caf\u{fffd}"));
        assert!(super::args().count() > 0);
    }
}
