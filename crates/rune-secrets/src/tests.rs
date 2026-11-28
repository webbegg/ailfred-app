use super::*;

/// What every store promises, run against the one given.
fn keeps_secrets(store: &dyn SecretStore) {
    assert_eq!(store.get("one").unwrap(), None, "nothing saved yet");
    assert!(!store.exists("one").unwrap());

    store.set("one", "first").unwrap();
    assert_eq!(store.get("one").unwrap().as_deref(), Some("first"));
    assert!(store.exists("one").unwrap());

    store.set("one", "second").unwrap();
    assert_eq!(store.get("one").unwrap().as_deref(), Some("second"), "a save replaces the old secret");

    store.set("two", "other").unwrap();
    assert_eq!(store.get("one").unwrap().as_deref(), Some("second"), "accounts keep their secrets apart");

    store.delete("one").unwrap();
    assert_eq!(store.get("one").unwrap(), None);
    assert!(!store.exists("one").unwrap());
    store.delete("one").unwrap();
    assert_eq!(store.get("two").unwrap().as_deref(), Some("other"), "deleting one account leaves the others");
    store.delete("two").unwrap();
}

#[test]
fn a_memory_store_saves_replaces_and_forgets_secrets_per_account() {
    keeps_secrets(&MemoryStore::default());
}

#[test]
fn deleting_a_secret_that_isnt_saved_is_no_error() {
    MemoryStore::default().delete("never").unwrap();
}

#[test]
fn a_memory_store_shows_how_many_secrets_it_holds_but_never_their_names_or_values() {
    let store = MemoryStore::default();
    store.set("api-key", "sk-secret").unwrap();
    let shown = format!("{store:?}");
    assert!(shown.contains("1 hidden"), "{shown}");
    assert!(!shown.contains("sk-secret") && !shown.contains("api-key"), "{shown}");
}

#[cfg(target_os = "macos")]
#[test]
fn the_keychain_refuses_an_empty_service_or_account_before_asking_the_system() {
    // Empty attributes match every item in a Keychain query, so they must never reach one.
    assert!(Keychain::new("").is_err());
    let keychain = Keychain::new("com.example.never-used").unwrap();
    assert!(keychain.get("").is_err());
    assert!(keychain.exists("").is_err());
    assert!(keychain.set("", "x").is_err());
    assert!(keychain.delete("").is_err());
}

/// Talks to the real login Keychain, so it isn't part of `scripts/check`: run it by hand with
/// `cargo test -p rune-secrets -- --ignored` on a Mac with an unlocked login Keychain.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "uses the real login Keychain"]
fn the_login_keychain_saves_replaces_and_forgets_secrets_per_account() {
    /// Deletes the test's items even when an assertion fails, so no run leaves any behind.
    struct Cleanup(Keychain);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.delete("one");
            let _ = self.0.delete("two");
        }
    }

    let service = format!("com.rune-secrets.test.{}", std::process::id());
    let keychain = Cleanup(Keychain::new(&service).unwrap());
    keeps_secrets(&keychain.0);
}
