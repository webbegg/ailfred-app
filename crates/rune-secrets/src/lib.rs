//! Secrets for Rune apps, kept in the OS credential store rather than in files: an API key, a
//! token, a project's env var values.
//!
//! A store holds one app's secrets, each under an account name (`anthropic-api-key`,
//! `<project>/<variable>`). [`Keychain`] is the login Keychain on macOS; [`MemoryStore`] keeps
//! them in memory, for tests and headless runs. Every call to the Keychain talks to the
//! system's security service and may prompt: call it from a user's action or off the UI
//! thread, and never at startup.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard};

#[cfg(target_os = "macos")]
mod keychain;

#[cfg(target_os = "macos")]
pub use keychain::Keychain;

/// Where an app keeps its secrets, each under an account name.
pub trait SecretStore: Send + Sync {
    /// The secret saved for `account`, if any.
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    /// Whether a secret is saved for `account`. A store that can tell without reading the
    /// secret does so, so asking never prompts for it.
    fn exists(&self, account: &str) -> Result<bool, String>;
    /// Saves `value` for `account`, replacing the old one.
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    /// Forgets the secret for `account`; deleting one that isn't saved is not an error.
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// Secrets held in memory, for tests and headless runs.
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    fn values(&self) -> MutexGuard<'_, HashMap<String, String>> {
        self.values.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self.values().get(account).cloned())
    }

    fn exists(&self, account: &str) -> Result<bool, String> {
        Ok(self.values().contains_key(account))
    }

    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        self.values().insert(account.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.values().remove(account);
        Ok(())
    }
}

/// Shows how many secrets are held, never their names or values.
impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore").field("values", &format_args!("{} hidden", self.values().len())).finish()
    }
}

#[cfg(test)]
mod tests;
