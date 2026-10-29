//! What a provider says of the producer's plan with every answer: how much of each window is used, and when
//! it resets. ChatGPT sends it in `x-codex-*` headers, a family each for its limits (`x-codex-primary-…` for
//! the five-hour window, `…-secondary-…` for the weekly one, other families named by `x-<id>-primary-…`).
//! API keys' per-minute request limits aren't a plan; a 429's Retry-After covers them.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

/// One of a plan's windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanWindow {
    pub used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<i64>,
    /// When it resets (epoch ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<f64>,
}

/// The latest windows each provider reported, for the whole process: answers come in on the session's thread, and the
/// screen reads them on its own.
static PLANS: Mutex<BTreeMap<String, Vec<PlanWindow>>> = Mutex::new(BTreeMap::new());

fn plans() -> MutexGuard<'static, BTreeMap<String, Vec<PlanWindow>>> {
    PLANS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Keeps what a response's headers say of the plan behind `provider`; a response that says nothing changes nothing.
pub fn record(provider: &str, headers: &BTreeMap<String, String>) {
    let windows = plan_windows(headers);
    if !windows.is_empty() {
        plans().insert(provider.to_string(), windows);
    }
}

/// The window of `provider`'s plan nearest its limit, when it's used `threshold` percent or more.
pub fn nearest(provider: &str, threshold: f64) -> Option<PlanWindow> {
    plans()
        .get(provider)?
        .iter()
        .filter(|window| window.used_percent >= threshold)
        .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
        .cloned()
}

/// Forgets every provider's windows (a new sign-in, tests).
pub fn forget() {
    plans().clear();
}

/// The plan's windows in ChatGPT's headers: each family's primary and secondary window (a window counts once it
/// says anything: some use, a length or a reset).
pub fn plan_windows(headers: &BTreeMap<String, String>) -> Vec<PlanWindow> {
    let mut families: Vec<String> = headers
        .keys()
        .filter_map(|name| name.to_ascii_lowercase().strip_suffix("-primary-used-percent").map(str::to_string))
        .filter(|prefix| prefix.starts_with("x-"))
        .collect();
    families.sort();
    families.dedup();
    let get = |name: &str| headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim());
    let mut windows = Vec::new();
    for family in families {
        for which in ["primary", "secondary"] {
            let Some(used) = get(&format!("{family}-{which}-used-percent")).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite())
            else {
                continue;
            };
            let minutes = get(&format!("{family}-{which}-window-minutes")).and_then(|v| v.parse::<i64>().ok());
            let resets = get(&format!("{family}-{which}-reset-at")).and_then(|v| v.parse::<i64>().ok());
            if used != 0.0 || minutes.is_some_and(|m| m != 0) || resets.is_some() {
                windows.push(PlanWindow {
                    used_percent: used,
                    window_minutes: minutes,
                    resets_at_ms: resets.map(|seconds| seconds as f64 * 1000.0),
                });
            }
        }
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
    }

    #[test]
    fn chatgpts_windows_are_read_from_each_family() {
        let read = plan_windows(&headers(&[
            ("x-codex-primary-used-percent", "82.5"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-at", "1791860000"),
            ("x-codex-secondary-used-percent", "41"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-at", "1792300000"),
            ("x-codex-other-primary-used-percent", "12"),
            ("content-type", "text/event-stream"),
        ]));
        assert_eq!(
            read,
            [
                PlanWindow { used_percent: 82.5, window_minutes: Some(300), resets_at_ms: Some(1_791_860_000_000.0) },
                PlanWindow { used_percent: 41.0, window_minutes: Some(10080), resets_at_ms: Some(1_792_300_000_000.0) },
                PlanWindow { used_percent: 12.0, window_minutes: None, resets_at_ms: None },
            ]
        );
        // A window that says nothing (no use, no length, no reset) isn't one; nor is a family without a number.
        assert!(plan_windows(&headers(&[("x-codex-primary-used-percent", "0"), ("x-codex-primary-window-minutes", "0")])).is_empty());
        assert!(plan_windows(&headers(&[("x-codex-primary-used-percent", "lots")])).is_empty());
    }

    #[test]
    fn the_window_nearest_its_limit_shows_past_the_threshold() {
        forget();
        record("openai-codex", &headers(&[("x-codex-primary-used-percent", "79"), ("x-codex-secondary-used-percent", "60")]));
        assert_eq!(nearest("openai-codex", 80.0), None);
        record("openai-codex", &headers(&[("x-codex-primary-used-percent", "84"), ("x-codex-secondary-used-percent", "91")]));
        assert_eq!(nearest("openai-codex", 80.0).map(|w| w.used_percent), Some(91.0));
        // A response without them leaves the last word standing.
        record("openai-codex", &headers(&[("content-type", "application/json")]));
        assert_eq!(nearest("openai-codex", 80.0).map(|w| w.used_percent), Some(91.0));
        assert_eq!(nearest("anthropic", 0.0), None);
        forget();
    }
}
