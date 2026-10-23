//! ICU collation for JavaScript's `String.localeCompare` without explicit options.
use icu_collator::{Collator, CollatorBorrowed};
use icu_locale_core::Locale;
use std::{cmp::Ordering, sync::LazyLock};
fn collator(locale: &str) -> CollatorBorrowed<'static> {
    let locale = locale.parse::<Locale>().unwrap_or_else(|_| "en-US".parse().unwrap());
    Collator::try_new(locale.into(), Default::default()).expect("compiled ICU collation data")
}
/// The process locale, using ICU environment precedence and native system defaults.
pub fn default_locale() -> String {
    // ICU's POSIX locale lookup, in priority order. A C locale maps to en-US in V8.
    let named = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        .or_else(system_locale)
        .unwrap_or_else(|| "en-US".into());
    let named = named.split(['.', '@']).next().unwrap_or("en-US");
    if matches!(named, "C" | "POSIX") {
        "en-US".into()
    } else {
        named.replace('_', "-")
    }
}
#[cfg(target_os = "macos")]
fn system_locale() -> Option<String> {
    use std::ffi::{c_char, c_void, CStr};
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyCurrent() -> *const c_void;
        fn CFLocaleGetIdentifier(locale: *const c_void) -> *const c_void;
        fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFRelease(object: *const c_void);
    }
    // Both CoreFoundation objects are read-only; only the copied locale is owned here.
    unsafe {
        let locale = CFLocaleCopyCurrent();
        if locale.is_null() {
            return None;
        }
        let name = CFLocaleGetIdentifier(locale);
        let mut buffer = [0 as c_char; 256];
        let copied = !name.is_null() && CFStringGetCString(name, buffer.as_mut_ptr(), buffer.len() as isize, 0x08000100);
        CFRelease(locale);
        copied.then(|| CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned())
    }
}
#[cfg(windows)]
fn system_locale() -> Option<String> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, count: i32) -> i32;
    }
    let mut name = [0_u16; 85];
    let count = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    (count > 1).then(|| String::from_utf16_lossy(&name[..count as usize - 1]))
}
#[cfg(not(any(target_os = "macos", windows)))]
fn system_locale() -> Option<String> {
    None
}
/// `a.localeCompare(b)`: default-locale ICU collation, with case and accents significant,
/// punctuation retained, and digit runs compared lexically. The locale is read once, as in V8.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    static COLLATOR: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| collator(&default_locale()));
    COLLATOR.compare(a, b)
}
/// Default-locale comparison with `{ numeric: true, sensitivity: "base" }`.
pub fn locale_compare_numeric_base(a: &str, b: &str) -> Ordering {
    use icu_collator::{
        options::{CollatorOptions, Strength},
        preferences::CollationNumericOrdering,
        CollatorPreferences,
    };
    static COLLATOR: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| {
        let locale = default_locale().parse::<Locale>().unwrap_or_else(|_| "en-US".parse().unwrap());
        let mut preferences: CollatorPreferences = locale.into();
        preferences.numeric_ordering = Some(CollationNumericOrdering::True);
        let mut options = CollatorOptions::default();
        options.strength = Some(Strength::Primary);
        Collator::try_new(preferences, options).expect("compiled ICU collation data")
    });
    COLLATOR.compare(a, b)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_and_explicit_collation_match_node_reference() {
        let fixture: serde_json::Value = serde_json::from_str(&kumi_testkit::oracle("kumi-common/locale-oracle.json")).unwrap();
        let values: Vec<&str> = fixture["values"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        for case in fixture["locales"].as_array().unwrap() {
            let locale = case["locale"].as_str().unwrap();
            let compare = collator(locale);
            let pairs = case["pairs"].as_array().unwrap();
            for (i, a) in values.iter().enumerate() {
                for (j, b) in values.iter().enumerate() {
                    let expected = pairs[i * values.len() + j].as_i64().unwrap().cmp(&0);
                    assert_eq!(compare.compare(a, b), expected, "{locale}: {a:?} vs {b:?}");
                }
            }
        }
    }
}
