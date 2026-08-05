use std::{
    convert::Infallible,
    fs::{File, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use fs2::FileExt;
use iota_stronghold::{
    engine::runtime::memories::buffer::Buffer,
    procedures::{
        FatalProcedureError, Procedure, ProcedureError, ProcedureOutput, Runner, UseSecret,
    },
    KeyProvider, Location, SnapshotPath, Stronghold,
};
use zeroize::Zeroizing;

pub type DatabaseKey = Zeroizing<Vec<u8>>;

const CLIENT_PATH: &[u8] = b"interview-assistant";
const VAULT_PATH: &[u8] = b"database-keys";
const RECORD_PATH: &[u8] = b"sqlcipher-v1";
const PENDING_SECRET_PREFIX: &[u8] = b"interview-assistant:stronghold-pending:v1\0";

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
        let _snapshot_lock = SnapshotInitializationLock::acquire(&self.snapshot_path)?;
        let has_snapshot = self.snapshot_path.exists();
        let unlock_secret = self.secrets.load()?.map(decode_unlock_secret).transpose()?;
        match (has_snapshot, unlock_secret) {
            (false, None) => self.create_database_key(),
            (true, Some(UnlockSecret::Committed(unlock_secret))) => {
                read_database_key(&self.snapshot_path, unlock_secret)
            }
            (true, Some(UnlockSecret::Pending(unlock_secret))) => {
                let database_key = read_database_key(&self.snapshot_path, unlock_secret.clone())?;
                self.secrets.store(&unlock_secret)?;
                Ok(database_key)
            }
            (false, Some(UnlockSecret::Pending(_))) => self.create_database_key(),
            (true, None) => Err(KeyManagerError::RecoveryRequired(
                "vault exists but its OS unlock secret is missing",
            )),
            (false, Some(UnlockSecret::Committed(_))) => Err(KeyManagerError::RecoveryRequired(
                "OS unlock secret exists but its vault is missing",
            )),
        }
    }

    fn create_database_key(&self) -> Result<DatabaseKey, KeyManagerError> {
        let mut database_key = Zeroizing::new(vec![0_u8; 32]);
        let mut unlock_secret = Zeroizing::new(vec![0_u8; 32]);
        getrandom::fill(&mut database_key).map_err(|_| KeyManagerError::Randomness)?;
        getrandom::fill(&mut unlock_secret).map_err(|_| KeyManagerError::Randomness)?;
        let pending_secret = encode_pending_unlock_secret(&unlock_secret);
        self.secrets.store(&pending_secret)?;
        let parent = self
            .snapshot_path
            .parent()
            .ok_or(KeyManagerError::Stronghold)?;
        let temporary_snapshot = tempfile::Builder::new()
            .prefix(".stronghold-init-")
            .tempfile_in(parent)
            .map_err(|_| KeyManagerError::Stronghold)?
            .into_temp_path();
        write_database_key(
            &temporary_snapshot,
            unlock_secret.clone(),
            database_key.clone(),
        )?;
        if let Err(error) = temporary_snapshot.persist_noclobber(&self.snapshot_path) {
            return if error.error.kind() == ErrorKind::AlreadyExists {
                Err(KeyManagerError::RecoveryRequired(
                    "vault appeared while secure initialization was in progress",
                ))
            } else {
                Err(KeyManagerError::Stronghold)
            };
        }
        self.secrets.store(&unlock_secret)?;
        Ok(database_key)
    }
}

enum UnlockSecret {
    Committed(DatabaseKey),
    Pending(DatabaseKey),
}

fn decode_unlock_secret(mut stored: DatabaseKey) -> Result<UnlockSecret, KeyManagerError> {
    if stored.len() == 32 {
        return Ok(UnlockSecret::Committed(stored));
    }
    if stored.len() == PENDING_SECRET_PREFIX.len() + 32 && stored.starts_with(PENDING_SECRET_PREFIX)
    {
        stored.drain(..PENDING_SECRET_PREFIX.len());
        return Ok(UnlockSecret::Pending(stored));
    }
    Err(KeyManagerError::RecoveryRequired(
        "OS unlock secret has an invalid format",
    ))
}

fn encode_pending_unlock_secret(unlock_secret: &[u8]) -> DatabaseKey {
    let mut pending = Zeroizing::new(Vec::with_capacity(
        PENDING_SECRET_PREFIX.len() + unlock_secret.len(),
    ));
    pending.extend_from_slice(PENDING_SECRET_PREFIX);
    pending.extend_from_slice(unlock_secret);
    pending
}

struct SnapshotInitializationLock {
    file: File,
}

impl SnapshotInitializationLock {
    fn acquire(snapshot_path: &Path) -> Result<Self, KeyManagerError> {
        let file_name = snapshot_path
            .file_name()
            .ok_or(KeyManagerError::InitializationLock)?;
        let mut lock_name = file_name.to_os_string();
        lock_name.push(".lock");
        let lock_path = snapshot_path.with_file_name(lock_name);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|_| KeyManagerError::InitializationLock)?;
        file.lock_exclusive()
            .map_err(|_| KeyManagerError::InitializationLock)?;
        Ok(Self { file })
    }
}

impl Drop for SnapshotInitializationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

struct ExportedDatabaseKey(DatabaseKey);

impl TryFrom<ProcedureOutput> for ExportedDatabaseKey {
    type Error = Infallible;

    fn try_from(output: ProcedureOutput) -> Result<Self, Self::Error> {
        Ok(Self(Zeroizing::new(Vec::<u8>::from(output))))
    }
}

struct ExportDatabaseKey {
    source: Location,
}

impl UseSecret<1> for ExportDatabaseKey {
    type Output = ExportedDatabaseKey;

    fn use_secret(
        self,
        [database_key]: [Buffer<u8>; 1],
    ) -> Result<Self::Output, FatalProcedureError> {
        let guarded_key = database_key.borrow();
        Ok(ExportedDatabaseKey(Zeroizing::new(guarded_key.to_vec())))
    }

    fn source(&self) -> [Location; 1] {
        [self.source.clone()]
    }
}

impl Procedure for ExportDatabaseKey {
    type Output = ExportedDatabaseKey;

    fn execute<R: Runner>(self, runner: &R) -> Result<Self::Output, ProcedureError> {
        <Self as UseSecret<1>>::exec(self, runner)
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
        .vault(VAULT_PATH)
        .write_secret(
            Location::generic(VAULT_PATH.to_vec(), RECORD_PATH.to_vec()),
            database_key,
        )
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
    let guarded_key = ExportDatabaseKey {
        source: Location::generic(VAULT_PATH.to_vec(), RECORD_PATH.to_vec()),
    }
    .execute(&client);
    match guarded_key {
        Ok(ExportedDatabaseKey(database_key)) => validate_database_key(database_key),
        Err(_) => migrate_legacy_database_key(&stronghold, &client, &provider, snapshot_path),
    }
}

fn migrate_legacy_database_key(
    stronghold: &Stronghold,
    client: &iota_stronghold::Client,
    provider: &KeyProvider,
    snapshot_path: &Path,
) -> Result<DatabaseKey, KeyManagerError> {
    let database_key = client
        .store()
        .get(RECORD_PATH)
        .map_err(|_| KeyManagerError::RecoveryRequired("database key record is unavailable"))?
        .map(Zeroizing::new)
        .ok_or(KeyManagerError::RecoveryRequired(
            "database key record is unavailable",
        ))?;
    let database_key = validate_database_key(database_key)?;
    client
        .vault(VAULT_PATH)
        .write_secret(
            Location::generic(VAULT_PATH.to_vec(), RECORD_PATH.to_vec()),
            database_key.clone(),
        )
        .map_err(|_| KeyManagerError::Stronghold)?;
    let removed = client
        .store()
        .delete(RECORD_PATH)
        .map_err(|_| KeyManagerError::Stronghold)?;
    let _removed = removed.map(Zeroizing::new);
    persist_migrated_snapshot(stronghold, provider, snapshot_path)?;
    Ok(database_key)
}

fn persist_migrated_snapshot(
    stronghold: &Stronghold,
    provider: &KeyProvider,
    snapshot_path: &Path,
) -> Result<(), KeyManagerError> {
    let parent = snapshot_path.parent().ok_or(KeyManagerError::Stronghold)?;
    let temporary_snapshot = tempfile::Builder::new()
        .prefix(".stronghold-migrate-")
        .tempfile_in(parent)
        .map_err(|_| KeyManagerError::Stronghold)?
        .into_temp_path();
    stronghold
        .commit_with_keyprovider(&SnapshotPath::from_path(&temporary_snapshot), provider)
        .map_err(|_| KeyManagerError::Stronghold)?;
    temporary_snapshot
        .persist(snapshot_path)
        .map_err(|_| KeyManagerError::Stronghold)
}

fn validate_database_key(database_key: DatabaseKey) -> Result<DatabaseKey, KeyManagerError> {
    if database_key.len() == 32 {
        Ok(database_key)
    } else {
        Err(KeyManagerError::RecoveryRequired(
            "database key record has an invalid length",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KeyManager, KeyManagerError, MemorySecretStore, SecretStore, CLIENT_PATH, RECORD_PATH,
    };
    use iota_stronghold::{KeyProvider, SnapshotPath, Stronghold};
    use std::{
        panic::{catch_unwind, AssertUnwindSafe},
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

    #[derive(Default)]
    struct CrashAfterFirstStore {
        inner: MemorySecretStore,
        should_crash: AtomicBool,
    }

    impl CrashAfterFirstStore {
        fn armed() -> Self {
            Self {
                inner: MemorySecretStore::default(),
                should_crash: AtomicBool::new(true),
            }
        }
    }

    impl SecretStore for CrashAfterFirstStore {
        fn load(&self) -> Result<Option<super::DatabaseKey>, KeyManagerError> {
            self.inner.load()
        }

        fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
            self.inner.store(value)?;
            if self.should_crash.swap(false, Ordering::SeqCst) {
                panic!("injected crash after first credential write");
            }
            Ok(())
        }

        fn delete(&self) -> Result<(), KeyManagerError> {
            self.inner.delete()
        }
    }

    #[derive(Default)]
    struct CrashBeforeSecondStore {
        inner: MemorySecretStore,
        store_calls: AtomicUsize,
    }

    impl SecretStore for CrashBeforeSecondStore {
        fn load(&self) -> Result<Option<super::DatabaseKey>, KeyManagerError> {
            self.inner.load()
        }

        fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
            if self.store_calls.fetch_add(1, Ordering::SeqCst) == 1 {
                panic!("injected crash before finalized credential write");
            }
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
    fn legacy_store_snapshot_migrates_to_vault_and_reopens() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("legacy.stronghold");
        let unlock_secret = super::DatabaseKey::new(vec![0x31; 32]);
        let database_key = super::DatabaseKey::new(vec![0x52; 32]);
        let stronghold = Stronghold::default();
        let client = stronghold.create_client(CLIENT_PATH).unwrap();
        client
            .store()
            .insert(RECORD_PATH.to_vec(), database_key.to_vec(), None)
            .unwrap();
        let provider = KeyProvider::try_from(unlock_secret.clone()).unwrap();
        stronghold
            .commit_with_keyprovider(&SnapshotPath::from_path(&snapshot), &provider)
            .unwrap();

        let secrets = Arc::new(MemorySecretStore::default());
        secrets.store(&unlock_secret).unwrap();
        let migrated = KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap();
        assert_eq!(migrated.as_slice(), database_key.as_slice());

        let migrated_snapshot = Stronghold::default();
        let provider = KeyProvider::try_from(unlock_secret.clone()).unwrap();
        migrated_snapshot
            .load_snapshot(&provider, &SnapshotPath::from_path(&snapshot))
            .unwrap();
        let migrated_client = migrated_snapshot.load_client(CLIENT_PATH).unwrap();
        assert!(migrated_client.store().get(RECORD_PATH).unwrap().is_none());

        let reopened = KeyManager::new(&snapshot, secrets).database_key().unwrap();
        assert_eq!(reopened.as_slice(), database_key.as_slice());
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
    fn unrelated_unlock_secret_for_existing_vault_requires_recovery() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("unrelated-secret.stronghold");
        let secrets = Arc::new(MemorySecretStore::default());
        KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap();
        secrets.store(&[0xa5; 32]).unwrap();

        let error = KeyManager::new(&snapshot, secrets)
            .database_key()
            .unwrap_err();

        assert!(matches!(error, KeyManagerError::RecoveryRequired(_)));
    }

    #[test]
    fn unrelated_snapshot_for_existing_unlock_secret_requires_recovery() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("original.stronghold");
        let secrets = Arc::new(MemorySecretStore::default());
        KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap();

        let unrelated_snapshot = temp.path().join("unrelated.stronghold");
        KeyManager::new(&unrelated_snapshot, Arc::new(MemorySecretStore::default()))
            .database_key()
            .unwrap();
        std::fs::copy(unrelated_snapshot, &snapshot).unwrap();

        let error = KeyManager::new(&snapshot, secrets)
            .database_key()
            .unwrap_err();

        assert!(matches!(error, KeyManagerError::RecoveryRequired(_)));
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
        assert_eq!(manager.secrets.store_attempts.load(Ordering::SeqCst), 3);
        assert!(snapshot.exists());
    }

    #[test]
    fn crash_after_pending_credential_write_restarts_bootstrap() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("pending-only.stronghold");
        let secrets = Arc::new(CrashAfterFirstStore::armed());

        let crashed = catch_unwind(AssertUnwindSafe(|| {
            KeyManager::new(&snapshot, secrets.clone()).database_key()
        }));
        assert!(crashed.is_err());
        assert!(!snapshot.exists());

        let recovered = KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap();
        let reopened = KeyManager::new(&snapshot, secrets).database_key().unwrap();
        assert_eq!(recovered.as_slice(), reopened.as_slice());
    }

    #[test]
    fn crash_after_snapshot_publish_finalizes_pending_credential() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("snapshot-with-pending.stronghold");
        let secrets = Arc::new(CrashBeforeSecondStore::default());

        let crashed = catch_unwind(AssertUnwindSafe(|| {
            KeyManager::new(&snapshot, secrets.clone()).database_key()
        }));
        assert!(crashed.is_err());
        assert!(snapshot.exists());

        let recovered = KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap();
        let reopened = KeyManager::new(&snapshot, secrets).database_key().unwrap();
        assert_eq!(recovered.as_slice(), reopened.as_slice());
    }

    #[test]
    fn concurrent_bootstrap_creates_one_key_and_one_unlock_secret() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("secrets.stronghold");
        let secrets = Arc::new(SlowEmptyLoadSecretStore::default());
        let managers = [
            Arc::new(KeyManager::new(&snapshot, secrets.clone())),
            Arc::new(KeyManager::new(&snapshot, secrets.clone())),
        ];
        let start = Arc::new(Barrier::new(3));

        let handles = managers
            .into_iter()
            .map(|manager| {
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
        assert_eq!(secrets.stores.load(Ordering::SeqCst), 2);
        assert_eq!(keys[0].as_slice(), keys[1].as_slice());
    }

    #[test]
    fn database_key_uses_the_zeroizing_stronghold_vault_api() {
        let production = include_str!("key_manager.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();

        assert!(production.contains(".write_secret("));
        assert!(production.contains("ExportDatabaseKey"));
        assert!(!production.contains(".store()\n        .insert("));
        assert!(!production.contains("database_key.to_vec()"));
    }
}
