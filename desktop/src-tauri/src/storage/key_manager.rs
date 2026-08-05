use std::{
    fs,
    io::ErrorKind,
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
    #[error("secure key initialization lock is unavailable")]
    InitializationLock,
    #[error("OS secret storage failed and the new vault could not be removed")]
    InitializationRollback,
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
    initialization: Mutex<()>,
}

impl<S: SecretStore + ?Sized> KeyManager<S> {
    pub fn new(snapshot_path: impl AsRef<Path>, secrets: Arc<S>) -> Self {
        Self {
            snapshot_path: snapshot_path.as_ref().to_path_buf(),
            secrets,
            initialization: Mutex::new(()),
        }
    }

    pub fn database_key(&self) -> Result<DatabaseKey, KeyManagerError> {
        let _initialization = self
            .initialization
            .lock()
            .map_err(|_| KeyManagerError::InitializationLock)?;
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
        if let Err(error) = self.secrets.store(&unlock_secret) {
            match fs::remove_file(&self.snapshot_path) {
                Ok(()) => return Err(error),
                Err(cleanup) if cleanup.kind() == ErrorKind::NotFound => return Err(error),
                Err(_) => return Err(KeyManagerError::InitializationRollback),
            }
        }
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
    use std::{
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Barrier,
        },
        thread,
        time::Duration,
    };
    use tempfile::tempdir;

    struct FailOnceSecretStore {
        inner: MemorySecretStore,
        fail_store: AtomicBool,
        store_attempts: AtomicUsize,
    }

    impl Default for FailOnceSecretStore {
        fn default() -> Self {
            Self {
                inner: MemorySecretStore::default(),
                fail_store: AtomicBool::new(true),
                store_attempts: AtomicUsize::new(0),
            }
        }
    }

    impl SecretStore for FailOnceSecretStore {
        fn load(&self) -> Result<Option<super::DatabaseKey>, KeyManagerError> {
            self.inner.load()
        }

        fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
            self.store_attempts.fetch_add(1, Ordering::SeqCst);
            if self.fail_store.swap(false, Ordering::SeqCst) {
                return Err(KeyManagerError::SecretStore);
            }
            self.inner.store(value)
        }

        fn delete(&self) -> Result<(), KeyManagerError> {
            self.inner.delete()
        }
    }

    #[derive(Default)]
    struct SlowEmptyLoadSecretStore {
        inner: MemorySecretStore,
        empty_loads: AtomicUsize,
        stores: AtomicUsize,
    }

    impl SecretStore for SlowEmptyLoadSecretStore {
        fn load(&self) -> Result<Option<super::DatabaseKey>, KeyManagerError> {
            let value = self.inner.load()?;
            if value.is_none() {
                self.empty_loads.fetch_add(1, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(200));
            }
            Ok(value)
        }

        fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
            self.stores.fetch_add(1, Ordering::SeqCst);
            self.inner.store(value)
        }

        fn delete(&self) -> Result<(), KeyManagerError> {
            self.inner.delete()
        }
    }

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

    #[test]
    fn failed_secret_store_removes_new_snapshot_and_can_retry() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("secrets.stronghold");
        let secrets = Arc::new(FailOnceSecretStore::default());
        let manager = KeyManager::new(&snapshot, secrets);

        assert!(matches!(
            manager.database_key(),
            Err(KeyManagerError::SecretStore)
        ));
        assert_eq!(manager.secrets.store_attempts.load(Ordering::SeqCst), 1);
        assert!(!snapshot.exists());
        assert_eq!(manager.database_key().unwrap().len(), 32);
        assert_eq!(manager.secrets.store_attempts.load(Ordering::SeqCst), 2);
        assert!(snapshot.exists());
    }

    #[test]
    fn concurrent_bootstrap_creates_one_key_and_one_unlock_secret() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("secrets.stronghold");
        let secrets = Arc::new(SlowEmptyLoadSecretStore::default());
        let manager = Arc::new(KeyManager::new(&snapshot, secrets.clone()));
        let start = Arc::new(Barrier::new(3));

        let handles = (0..2)
            .map(|_| {
                let manager = manager.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    manager.database_key()
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        let keys = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(secrets.empty_loads.load(Ordering::SeqCst), 1);
        assert_eq!(secrets.stores.load(Ordering::SeqCst), 1);
        assert_eq!(keys[0].as_slice(), keys[1].as_slice());
    }
}
