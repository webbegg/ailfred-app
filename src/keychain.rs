//! The Anthropic API key in the login Keychain: a generic password, service `com.ailfred`
//! (the installed app's id, shared by a dev run), account `anthropic-api-key`. Every call talks
//! to the system's security service and may prompt: only call from a user's action (Settings)
//! or at send time, off the UI thread, and never at startup. Tests get a fake in memory.

// Only the real Keychain uses them; tests get a fake.
#[cfg_attr(test, allow(dead_code))]
pub const SERVICE: &str = "com.ailfred";
#[cfg_attr(test, allow(dead_code))]
pub const ACCOUNT: &str = "anthropic-api-key";

/// The saved key, if any.
pub fn get() -> Option<String> {
    imp::get()
}

/// Whether a key is saved, without reading it.
pub fn exists() -> bool {
    imp::exists()
}

/// Saves `key`, replacing the old one.
pub fn set(key: &str) -> Result<(), String> {
    imp::set(key)
}

/// Forgets the key; no key saved is no error.
pub fn remove() -> Result<(), String> {
    imp::remove()
}

#[cfg(not(test))]
mod imp {
    use std::ptr::NonNull;

    use objc2_core_foundation::{CFData, CFDictionary, CFRetained, CFString, CFType, kCFBooleanTrue};
    use objc2_security::{
        SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate, errSecItemNotFound, errSecSuccess, kSecAttrAccount, kSecAttrService, kSecClass,
        kSecClassGenericPassword, kSecMatchLimit, kSecMatchLimitOne, kSecReturnData, kSecValueData,
    };

    use super::{ACCOUNT, SERVICE};

    type Dict = CFRetained<CFDictionary<CFType, CFType>>;

    fn dict(pairs: &[(&CFString, &CFType)]) -> Dict {
        let keys: Vec<&CFType> = pairs.iter().map(|(k, _)| &***k).collect();
        let values: Vec<&CFType> = pairs.iter().map(|(_, v)| *v).collect();
        CFDictionary::from_slices(&keys, &values)
    }

    /// Our item, plus `extra` attributes.
    fn query(extra: &[(&CFString, &CFType)]) -> Dict {
        let (service, account) = (CFString::from_str(SERVICE), CFString::from_str(ACCOUNT));
        // SAFETY: constants of the Security framework.
        let mut pairs: Vec<(&CFString, &CFType)> =
            unsafe { vec![(kSecClass, &**kSecClassGenericPassword), (kSecAttrService, &*service), (kSecAttrAccount, &*account)] };
        pairs.extend_from_slice(extra);
        dict(&pairs)
    }

    fn found(status: i32) -> Result<bool, String> {
        match status {
            s if s == errSecSuccess => Ok(true),
            s if s == errSecItemNotFound => Ok(false),
            s => Err(format!("the Keychain said error {s}")),
        }
    }

    pub fn exists() -> bool {
        // SAFETY: a plain query for one match; the result isn't asked for.
        let q = unsafe { query(&[(kSecMatchLimit, &**kSecMatchLimitOne)]) };
        let status = unsafe { SecItemCopyMatching(q.as_opaque(), std::ptr::null_mut()) };
        matches!(found(status), Ok(true))
    }

    pub fn get() -> Option<String> {
        // SAFETY: constants of the Security and CoreFoundation frameworks.
        let yes = unsafe { kCFBooleanTrue }?;
        let q = unsafe { query(&[(kSecMatchLimit, &**kSecMatchLimitOne), (kSecReturnData, &**yes)]) };
        let mut result: *const CFType = std::ptr::null();
        // SAFETY: with kSecReturnData the result is a +1 CFData, which we take ownership of.
        let status = unsafe { SecItemCopyMatching(q.as_opaque(), &mut result) };
        if status != errSecSuccess {
            return None;
        }
        let data: CFRetained<CFData> = unsafe { CFRetained::from_raw(NonNull::new(result.cast_mut().cast())?) };
        String::from_utf8(data.to_vec()).ok()
    }

    pub fn set(key: &str) -> Result<(), String> {
        let data = CFData::from_bytes(key.as_bytes());
        // SAFETY: constants and plain calls with live dictionaries.
        let value = (unsafe { kSecValueData }, &*data as &CFType);
        let update = dict(&[value]);
        // Update the item when there is one, else add it.
        let status = unsafe { SecItemUpdate(query(&[]).as_opaque(), update.as_opaque()) };
        if found(status)? {
            return Ok(());
        }
        let add = query(&[value]);
        found(unsafe { SecItemAdd(add.as_opaque(), std::ptr::null_mut()) }).map(|_| ())
    }

    pub fn remove() -> Result<(), String> {
        // SAFETY: a plain call with a live dictionary.
        found(unsafe { SecItemDelete(query(&[]).as_opaque()) }).map(|_| ())
    }
}

/// What tests see: a key in memory, never the Keychain.
#[cfg(test)]
mod imp {
    use std::sync::Mutex;

    static FAKE: Mutex<Option<String>> = Mutex::new(None);

    fn slot() -> std::sync::MutexGuard<'static, Option<String>> {
        FAKE.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get() -> Option<String> {
        slot().clone()
    }

    pub fn exists() -> bool {
        slot().is_some()
    }

    pub fn set(key: &str) -> Result<(), String> {
        *slot() = Some(key.to_owned());
        Ok(())
    }

    pub fn remove() -> Result<(), String> {
        *slot() = None;
        Ok(())
    }
}
