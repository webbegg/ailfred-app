//! The login Keychain: each secret is a generic password with the store's service and the
//! secret's account, the item `security find-generic-password -s <service> -a <account>` finds.

use std::ptr::NonNull;

use objc2_core_foundation::{CFData, CFDictionary, CFRetained, CFString, CFType, kCFBooleanTrue};
use objc2_security::{
    SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate, errSecItemNotFound, errSecSuccess, kSecAttrAccount, kSecAttrService, kSecClass,
    kSecClassGenericPassword, kSecMatchLimit, kSecMatchLimitOne, kSecReturnData, kSecValueData,
};

use crate::SecretStore;

/// One app's secrets in the login Keychain, under its service (usually the app's id).
#[derive(Debug)]
pub struct Keychain {
    service: String,
}

impl Keychain {
    /// Fails on an empty service: empty attributes match every item in a Keychain query.
    pub fn new(service: &str) -> Result<Keychain, String> {
        if service.is_empty() {
            return Err("a Keychain service can't be empty".into());
        }
        Ok(Keychain { service: service.to_owned() })
    }

    /// The query for `account`'s item, plus `extra` attributes.
    fn query(&self, account: &str, extra: &[(&CFString, &CFType)]) -> Result<Dict, String> {
        if account.is_empty() {
            return Err("a Keychain account can't be empty".into());
        }
        let (service, account) = (CFString::from_str(&self.service), CFString::from_str(account));
        // SAFETY: constants of the Security framework.
        let mut pairs: Vec<(&CFString, &CFType)> =
            unsafe { vec![(kSecClass, &**kSecClassGenericPassword), (kSecAttrService, &*service), (kSecAttrAccount, &*account)] };
        pairs.extend_from_slice(extra);
        Ok(dict(&pairs))
    }
}

type Dict = CFRetained<CFDictionary<CFType, CFType>>;

fn dict(pairs: &[(&CFString, &CFType)]) -> Dict {
    let keys: Vec<&CFType> = pairs.iter().map(|(k, _)| &***k).collect();
    let values: Vec<&CFType> = pairs.iter().map(|(_, v)| *v).collect();
    CFDictionary::from_slices(&keys, &values)
}

/// Whether the item was there, or the Keychain's error.
fn found(status: i32) -> Result<bool, String> {
    match status {
        s if s == errSecSuccess => Ok(true),
        s if s == errSecItemNotFound => Ok(false),
        s => Err(format!("the Keychain said error {s}")),
    }
}

impl SecretStore for Keychain {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        // SAFETY: constants of the Security and CoreFoundation frameworks.
        let yes = unsafe { kCFBooleanTrue }.ok_or("CoreFoundation has no true")?;
        let q = unsafe { self.query(account, &[(kSecMatchLimit, &**kSecMatchLimitOne), (kSecReturnData, &**yes)]) }?;
        let mut result: *const CFType = std::ptr::null();
        // SAFETY: with kSecReturnData the result is a +1 CFData, which we take ownership of.
        let status = unsafe { SecItemCopyMatching(q.as_opaque(), &mut result) };
        if !found(status)? {
            return Ok(None);
        }
        let raw = NonNull::new(result.cast_mut().cast()).ok_or("the Keychain returned no data")?;
        let data: CFRetained<CFData> = unsafe { CFRetained::from_raw(raw) };
        String::from_utf8(data.to_vec()).map(Some).map_err(|_| "the saved secret isn't text".into())
    }

    fn exists(&self, account: &str) -> Result<bool, String> {
        // SAFETY: a plain query for one match; the result (and so the secret) isn't asked for.
        let q = unsafe { self.query(account, &[(kSecMatchLimit, &**kSecMatchLimitOne)]) }?;
        found(unsafe { SecItemCopyMatching(q.as_opaque(), std::ptr::null_mut()) })
    }

    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        let data = CFData::from_bytes(value.as_bytes());
        // SAFETY: constants and plain calls with live dictionaries.
        let value = (unsafe { kSecValueData }, &*data as &CFType);
        let update = dict(&[value]);
        // Update the item when there is one, else add it.
        if found(unsafe { SecItemUpdate(self.query(account, &[])?.as_opaque(), update.as_opaque()) })? {
            return Ok(());
        }
        let add = self.query(account, &[value])?;
        found(unsafe { SecItemAdd(add.as_opaque(), std::ptr::null_mut()) }).map(|_| ())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        // SAFETY: a plain call with a live dictionary.
        found(unsafe { SecItemDelete(self.query(account, &[])?.as_opaque()) }).map(|_| ())
    }
}
