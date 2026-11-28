//! The Anthropic API key in the login Keychain: a generic password, service `com.ailfred`
//! (the installed app's id, shared by a dev run), account `anthropic-api-key`. Every call talks
//! to the system's security service and may prompt: only call from a user's action (Settings)
//! or at send time, off the UI thread, and never at startup. Tests get a store in memory.

use std::sync::LazyLock;

use rune_secrets::SecretStore;

// Only the real Keychain uses it; tests get a store in memory.
#[cfg_attr(test, allow(dead_code))]
pub const SERVICE: &str = "com.ailfred";
pub const ACCOUNT: &str = "anthropic-api-key";

#[cfg(not(test))]
static STORE: LazyLock<rune_secrets::Keychain> = LazyLock::new(|| rune_secrets::Keychain::new(SERVICE).expect("the service isn't empty"));

#[cfg(test)]
static STORE: LazyLock<rune_secrets::MemoryStore> = LazyLock::new(rune_secrets::MemoryStore::default);

/// The saved key, if any.
pub fn get() -> Option<String> {
    STORE.get(ACCOUNT).ok().flatten()
}

/// Whether a key is saved, without reading it.
pub fn exists() -> bool {
    STORE.exists(ACCOUNT).unwrap_or(false)
}

/// Saves `key`, replacing the old one.
pub fn set(key: &str) -> Result<(), String> {
    STORE.set(ACCOUNT, key)
}

/// Forgets the key; no key saved is no error.
pub fn remove() -> Result<(), String> {
    STORE.delete(ACCOUNT)
}
