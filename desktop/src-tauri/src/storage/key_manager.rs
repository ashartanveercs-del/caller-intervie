use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use iota_stronghold::{KeyProvider, SnapshotPath, Stronghold};
use zeroize::Zeroizing;

pub type DatabaseKey = Zeroizing<Vec<u8>>;

const CLIENT_PATH: &[u8] = b"interview-assistant";
const RECORD_PATH: &[u8] = b"sqlcipher-v1";

#[derive(Debug, thiserror::Error)]
pub enum KeyManagerError {
    #[error("secret recovery is required: {0}")]
    RecoveryRequired(&'static str),
    #[error("operating-system secret store is unavailable")]
    SecretStore,
    #[error("encrypted stronghold vault is unavailable")]
    Stronghold,
    #[error("secure randomness is unavailable")]
    Randomness,
}

pub trait SecretStore: Send + Sync {
    fn load(&self) -> Result<Option<DatabaseKey>, KeyManagerError>;
    fn store(&self, value: &[u8]) -> Result<(), KeyManagerError>;
    fn delete(&self) -> Result<(), KeyManagerError>;
}

#[derive(Default)]
pub struct MemorySecretStore {
    secret: Mutex<Option<Vec<u8>>>,
}

impl SecretStore for MemorySecretStore {
    fn load(&self) -> Result<Option<DatabaseKey>, KeyManagerError> {
        Ok(self
            .secret
            .lock()
            .map_err(|_| KeyManagerError::SecretStore)?
            .clone()
            .map(Zeroizing::new))
    }
    fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
        *self
            .secret
            .lock()
            .map_err(|_| KeyManagerError::SecretStore)? = Some(value.to_vec());
        Ok(())
    }
    fn delete(&self) -> Result<(), KeyManagerError> {
        *self
            .secret
            .lock()
            .map_err(|_| KeyManagerError::SecretStore)? = None;
        Ok(())
    }
}

/// OS credential-manager adapter for the Stronghold snapshot unlock secret.
pub struct KeyringSecretStore {
    service: String,
    account: String,
}

impl KeyringSecretStore {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }
    fn entry(&self) -> Result<keyring::Entry, KeyManagerError> {
        keyring::Entry::new(&self.service, &self.account).map_err(|_| KeyManagerError::SecretStore)
    }
}

impl SecretStore for KeyringSecretStore {
    fn load(&self) -> Result<Option<DatabaseKey>, KeyManagerError> {
        match self.entry()?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(KeyManagerError::SecretStore),
        }
    }
    fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
        self.entry()?
            .set_secret(value)
            .map_err(|_| KeyManagerError::SecretStore)
    }
    fn delete(&self) -> Result<(), KeyManagerError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(KeyManagerError::SecretStore),
        }
    }
}

pub struct KeyManager<S: SecretStore + ?Sized> {
    snapshot_path: PathBuf,
    secrets: Arc<S>,
}

impl<S: SecretStore + ?Sized> KeyManager<S> {
    pub fn new(snapshot_path: impl AsRef<Path>, secrets: Arc<S>) -> Self {
        Self {
            snapshot_path: snapshot_path.as_ref().to_path_buf(),
            secrets,
        }
    }

    pub fn database_key(&self) -> Result<DatabaseKey, KeyManagerError> {
        let has_snapshot = self.snapshot_path.exists();
        let unlock_secret = self.secrets.load()?;
        match (has_snapshot, unlock_secret) {
            (false, None) => self.create_database_key(),
            (true, Some(unlock_secret)) => read_database_key(&self.snapshot_path, unlock_secret),
            (true, None) => Err(KeyManagerError::RecoveryRequired(
                "vault exists but its OS unlock secret is missing",
            )),
            (false, Some(_)) => Err(KeyManagerError::RecoveryRequired(
                "OS unlock secret exists but its vault is missing",
            )),
        }
    }

    fn create_database_key(&self) -> Result<DatabaseKey, KeyManagerError> {
        let mut database_key = Zeroizing::new(vec![0_u8; 32]);
        let mut unlock_secret = Zeroizing::new(vec![0_u8; 32]);
        getrandom::fill(&mut database_key).map_err(|_| KeyManagerError::Randomness)?;
        getrandom::fill(&mut unlock_secret).map_err(|_| KeyManagerError::Randomness)?;
        write_database_key(
            &self.snapshot_path,
            unlock_secret.clone(),
            database_key.clone(),
        )?;
        self.secrets.store(&unlock_secret)?;
        Ok(database_key)
    }
}

fn write_database_key(
    snapshot_path: &Path,
    unlock_secret: DatabaseKey,
    database_key: DatabaseKey,
) -> Result<(), KeyManagerError> {
    let stronghold = Stronghold::default();
    let client = stronghold
        .create_client(CLIENT_PATH)
        .map_err(|_| KeyManagerError::Stronghold)?;
    client
        .store()
        .insert(RECORD_PATH.to_vec(), database_key.to_vec(), None)
        .map_err(|_| KeyManagerError::Stronghold)?;
    let provider = KeyProvider::try_from(unlock_secret).map_err(|_| KeyManagerError::Stronghold)?;
    stronghold
        .commit_with_keyprovider(&SnapshotPath::from_path(snapshot_path), &provider)
        .map_err(|_| KeyManagerError::Stronghold)
}

fn read_database_key(
    snapshot_path: &Path,
    unlock_secret: DatabaseKey,
) -> Result<DatabaseKey, KeyManagerError> {
    if unlock_secret.len() != 32 {
        return Err(KeyManagerError::RecoveryRequired(
            "OS unlock secret has an invalid length",
        ));
    }
    let stronghold = Stronghold::default();
    let provider = KeyProvider::try_from(unlock_secret).map_err(|_| KeyManagerError::Stronghold)?;
    let snapshot = SnapshotPath::from_path(snapshot_path);
    stronghold
        .load_snapshot(&provider, &snapshot)
        .map_err(|_| KeyManagerError::RecoveryRequired("vault could not be opened"))?;
    let client = stronghold
        .load_client(CLIENT_PATH)
        .map_err(|_| KeyManagerError::RecoveryRequired("vault client is unavailable"))?;
    client
        .store()
        .get(RECORD_PATH)
        .map_err(|_| KeyManagerError::RecoveryRequired("database key record is unavailable"))?
        .map(Zeroizing::new)
        .ok_or(KeyManagerError::RecoveryRequired(
            "database key record is unavailable",
        ))
}

#[cfg(test)]
mod tests {
    use super::{KeyManager, KeyManagerError, MemorySecretStore, SecretStore};
    use std::sync::Arc;
    use tempfile::tempdir;
    #[test]
    fn database_key_is_created_in_stronghold_and_reloads() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("secrets.stronghold");
        let secrets = Arc::new(MemorySecretStore::default());
        let manager = KeyManager::new(&snapshot, secrets.clone());
        let first = manager.database_key().unwrap();
        assert_eq!(first.len(), 32);
        assert!(snapshot.exists());
        assert!(!std::fs::read(&snapshot)
            .unwrap()
            .windows(first.len())
            .any(|window| window == first.as_slice()));
        let reopened = KeyManager::new(&snapshot, secrets);
        assert_eq!(
            reopened.database_key().unwrap().as_slice(),
            first.as_slice()
        );
    }
    #[test]
    fn existing_vault_without_unlock_secret_requires_recovery() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("secrets.stronghold");
        let secrets = Arc::new(MemorySecretStore::default());
        KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap();
        secrets.delete().unwrap();
        let error = KeyManager::new(&snapshot, secrets)
            .database_key()
            .unwrap_err();
        assert!(matches!(error, KeyManagerError::RecoveryRequired(_)));
    }
    #[test]
    fn unlock_secret_without_vault_requires_recovery() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("missing.stronghold");
        let secrets = Arc::new(MemorySecretStore::default());
        secrets.store(&[7; 32]).unwrap();
        let error = KeyManager::new(&snapshot, secrets)
            .database_key()
            .unwrap_err();
        assert!(matches!(error, KeyManagerError::RecoveryRequired(_)));
        assert!(!snapshot.exists());
    }
}
