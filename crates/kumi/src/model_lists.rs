//! The cloud providers' model lists, kept between runs (`~/.kumi/cache/model-lists.json`), so a start doesn't wait for a
//! provider to list its models again: `/fast`'s tier, the context meter and `/model` read the kept list at once, and
//! Kumi asks the provider again in the background.
//!
//! A kept list is used only for the sign-in that listed it (a hash of the key, or of the ChatGPT account, never the
//! secret itself), by the Kumi version that kept it, and for at most [`MOST_AGE_MS`]. Model servers' lists aren't kept:
//! they're on this computer and answer at once. The file is a cache: one that can't be read or written is as if empty,
//! and an older Kumi never reads it.
use kumi_ai::{
    auth::store::Credential,
    providers::{api_key_from, models::ModelInfo, ProviderId, SignIn},
};
use kumi_types::{system::Env, version::KUMI_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// How long a kept list is used: older, Kumi waits for the provider's answer, as when there's none.
pub const MOST_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    lists: BTreeMap<String, KeptList>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeptList {
    /// Whose sign-in listed it: [`identity`].
    who: String,
    /// The Kumi that kept it: another reads its provider's list in its own way.
    kumi: String,
    /// When it was listed (ms since the epoch).
    at: i64,
    models: Vec<ModelInfo>,
}

/// Where the lists are kept.
#[derive(Debug, Clone)]
pub struct KeptLists {
    file: PathBuf,
}

impl KeptLists {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self { file: file.into() }
    }
    pub fn file(&self) -> &Path {
        &self.file
    }
    fn read_all(&self) -> Kept {
        std::fs::read(&self.file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }
    /// The list `provider` gave the sign-in `who`, kept by this Kumi within [`MOST_AGE_MS`] of `now`.
    pub fn read(&self, provider: &str, who: &str, now: i64) -> Option<Vec<ModelInfo>> {
        let kept = self.read_all().lists.remove(provider)?;
        let fresh = kept.at <= now && now - kept.at < MOST_AGE_MS;
        (kept.who == who && kept.kumi == KUMI_VERSION && fresh && !kept.models.is_empty()).then_some(kept.models)
    }
    /// Keeps the list `provider` just gave the sign-in `who`, in place of the one kept for it. A list that couldn't be
    /// kept is listed again next time, as before.
    pub fn keep(&self, provider: &str, who: &str, now: i64, models: &[ModelInfo]) {
        let mut kept = self.read_all();
        kept.lists.insert(provider.into(), KeptList { who: who.into(), kumi: KUMI_VERSION.into(), at: now, models: models.to_vec() });
        self.write(&kept);
    }
    /// Lets go of what's kept for these providers (a sign-in changed).
    pub fn forget(&self, providers: &[&str]) {
        let mut kept = self.read_all();
        let before = kept.lists.len();
        kept.lists.retain(|provider, _| !providers.contains(&provider.as_str()));
        if kept.lists.len() != before {
            self.write(&kept);
        }
    }
    fn write(&self, kept: &Kept) {
        let Some(folder) = self.file.parent() else { return };
        let Ok(text) = serde_json::to_vec(kept) else { return };
        let temporary = self.file.with_extension(format!("new-{}", std::process::id()));
        let written = std::fs::create_dir_all(folder)
            .and_then(|()| std::fs::write(&temporary, &text))
            .and_then(|()| std::fs::rename(&temporary, &self.file));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
    }
}

/// Whose sign-in lists `provider`'s models, as a hash: the API key Kumi would use (saved, else the environment's), or
/// the ChatGPT account. None without a sign-in.
pub fn identity(provider: ProviderId, saved: Option<&Credential>, env: &Env) -> Option<String> {
    let who = if provider.row().sign_in == SignIn::Chatgpt {
        match saved? {
            Credential::Oauth(credential) if !credential.account_id.is_empty() => format!("chatgpt\n{}", credential.account_id),
            _ => return None,
        }
    } else {
        format!("key\n{}", api_key_from(provider, saved, Some(env))?.key)
    };
    let digest = Sha256::digest(format!("{provider}\n{who}").as_bytes());
    Some(digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect())
}
