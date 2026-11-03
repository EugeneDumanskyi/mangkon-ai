//! API keys (ADR-0012, INV-EG-07): in the OS secret store, or for this session only in memory
//! when there is none. Never in `app.db`, a project, a config file or a log.

use std::collections::HashMap;
use std::sync::Mutex;

use secrecy::{ExposeSecret, SecretString};

use crate::error::{EgressError, ErrorCode};

/// The service name every key is stored under.
pub const SERVICE: &str = "mangkon-ai";

/// The secret-store account of an endpoint's key (`ENDPOINT.secretRef`).
pub fn account_for(endpoint_id: &str) -> String {
    format!("endpoint:{endpoint_id}")
}

/// Where keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretStoreKind {
    /// The OS keychain: macOS Keychain, Windows Credential Manager, Linux Secret Service.
    Keyring,
    /// No OS secret store: keys are kept in memory and lost on quit (ADR-0012).
    SessionOnly,
}

/// The secret-store port (ADR-0002 rule 2).
pub trait SecretStore: Send + Sync {
    fn kind(&self) -> SecretStoreKind;
    fn set(&self, account: &str, key: &SecretString) -> Result<(), EgressError>;
    /// `None` when there is no key for `account`.
    fn get(&self, account: &str) -> Result<Option<SecretString>, EgressError>;
    /// Removing a key that is not there is not an error.
    fn delete(&self, account: &str) -> Result<(), EgressError>;
}

/// The plain words shown when keys can only be kept for this session (ADR-0012).
pub const SESSION_ONLY_NOTICE: &str = "This computer has no system keychain (Secret Service) \
     running. API keys are kept only until Mangkon AI quits, and you will need to enter them \
     again next time. They are never written to disk.";

/// Keys in memory only, gone on quit.
#[derive(Default)]
pub struct SessionStore {
    keys: Mutex<HashMap<String, SecretString>>,
}

impl SessionStore {
    pub fn new() -> SessionStore {
        SessionStore::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SecretString>> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SecretStore for SessionStore {
    fn kind(&self) -> SecretStoreKind {
        SecretStoreKind::SessionOnly
    }

    fn set(&self, account: &str, key: &SecretString) -> Result<(), EgressError> {
        self.lock().insert(account.to_string(), key.clone());
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Option<SecretString>, EgressError> {
        Ok(self.lock().get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), EgressError> {
        self.lock().remove(account);
        Ok(())
    }
}

/// The OS secret store through `keyring`.
pub struct KeyringStore {
    service: String,
}

fn keyring_error(e: keyring::Error) -> EgressError {
    // The platform error names the store, never the secret.
    EgressError::new(
        ErrorCode::SecretStore,
        format!("The system keychain failed: {e}"),
    )
}

impl KeyringStore {
    /// Checks that the OS secret store answers, by reading an entry that is not there. Reading
    /// writes nothing and, for a missing entry, needs no unlock prompt.
    pub fn probe(service: &str) -> Result<KeyringStore, EgressError> {
        let entry = keyring::Entry::new(service, "probe").map_err(keyring_error)?;
        match entry.get_password() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(KeyringStore {
                service: service.to_string(),
            }),
            Err(e) => Err(keyring_error(e)),
        }
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry, EgressError> {
        keyring::Entry::new(&self.service, account).map_err(keyring_error)
    }
}

impl SecretStore for KeyringStore {
    fn kind(&self) -> SecretStoreKind {
        SecretStoreKind::Keyring
    }

    fn set(&self, account: &str, key: &SecretString) -> Result<(), EgressError> {
        self.entry(account)?
            .set_password(key.expose_secret())
            .map_err(keyring_error)
    }

    fn get(&self, account: &str) -> Result<Option<SecretString>, EgressError> {
        match self.entry(account)?.get_password() {
            Ok(k) => Ok(Some(SecretString::from(k))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_error(e)),
        }
    }

    fn delete(&self, account: &str) -> Result<(), EgressError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_error(e)),
        }
    }
}

/// The OS secret store if it answers, else a session store (ADR-0012).
pub fn system_secret_store() -> Box<dyn SecretStore> {
    match KeyringStore::probe(SERVICE) {
        Ok(k) => Box::new(k),
        Err(_) => Box::new(SessionStore::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_store_round_trip() {
        let s = SessionStore::new();
        let a = account_for("abc");
        assert_eq!(a, "endpoint:abc");
        assert!(s.get(&a).unwrap().is_none());
        s.set(&a, &SecretString::from("k-1")).unwrap();
        assert_eq!(s.get(&a).unwrap().unwrap().expose_secret(), "k-1");
        s.delete(&a).unwrap();
        s.delete(&a).unwrap();
        assert!(s.get(&a).unwrap().is_none());
        // The key never prints.
        let shown = format!("{:?}", SecretString::from("k-1"));
        assert!(!shown.contains("k-1"), "{shown}");
    }
}
