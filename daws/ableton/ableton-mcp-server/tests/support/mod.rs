//! Shared test helpers: `assert.throws(fn, /pattern/)` and the like.

#![allow(dead_code)]

use std::fmt::{Debug, Display};

/// `assert.throws(() => ..., /pattern/)`: the result is an error whose message matches `pattern`.
pub fn assert_throws<T: Debug, E: Display>(result: Result<T, E>, pattern: &str) {
    match result {
        Ok(value) => panic!("expected an error matching /{pattern}/, got {value:?}"),
        Err(error) => {
            let message = error.to_string();
            let regex = regex::Regex::new(pattern).expect("a valid pattern");
            assert!(regex.is_match(&message), "expected an error matching /{pattern}/, got {message:?}");
        }
    }
}

/// `assert.doesNotThrow` / a plain unwrap with the error's text in the failure.
pub fn assert_ok<T, E: Display>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected no error, got {error}"),
    }
}
