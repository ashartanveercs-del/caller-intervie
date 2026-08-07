use std::{
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
    secret: Mutex<Option<DatabaseKey>>,
}

impl SecretStore for MemorySecretStore {
    fn load(&self) -> Result<Option<DatabaseKey>, KeyManagerError> {
        let secret = self
            .secret
            .lock()
            .map_err(|_| KeyManagerError::SecretStore)?;
        Ok(secret.clone())
    }
    fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
        *self
            .secret
            .lock()
            .map_err(|_| KeyManagerError::SecretStore)? = Some(Zeroizing::new(value.to_vec()));
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
        let snapshot_presence = snapshot_presence(&self.snapshot_path)?;
        let unlock_secret = self.secrets.load()?.map(decode_unlock_secret).transpose()?;
        match (snapshot_presence, unlock_secret) {
            (SnapshotPresence::Missing, None) => self.create_database_key(),
            (SnapshotPresence::Present, Some(UnlockSecret::Committed(unlock_secret))) => {
                read_database_key(&self.snapshot_path, unlock_secret)
            }
            (SnapshotPresence::Present, Some(UnlockSecret::Pending(unlock_secret))) => {
                let database_key = read_database_key(&self.snapshot_path, unlock_secret.clone())?;
                self.store_committed_unlock_secret(&unlock_secret)?;
                Ok(database_key)
            }
            (SnapshotPresence::Missing, Some(UnlockSecret::Pending(unlock_secret))) => {
                self.recover_database_key(unlock_secret)
            }
            (SnapshotPresence::Present, None) => Err(KeyManagerError::RecoveryRequired(
                "vault exists but its OS unlock secret is missing",
            )),
            (SnapshotPresence::Missing, Some(UnlockSecret::Committed(_))) => {
                Err(KeyManagerError::RecoveryRequired(
                    "OS unlock secret exists but its vault is missing",
                ))
            }
        }
    }

    fn create_database_key(&self) -> Result<DatabaseKey, KeyManagerError> {
        let mut database_key = Zeroizing::new(vec![0_u8; 32]);
        let mut unlock_secret = Zeroizing::new(vec![0_u8; 32]);
        getrandom::fill(&mut database_key).map_err(|_| KeyManagerError::Randomness)?;
        getrandom::fill(&mut unlock_secret).map_err(|_| KeyManagerError::Randomness)?;
        let pending_secret = encode_pending_unlock_secret(&unlock_secret);
        self.secrets.store(&pending_secret)?;
        self.publish_database_key(unlock_secret, database_key)
    }

    fn recover_database_key(
        &self,
        unlock_secret: DatabaseKey,
    ) -> Result<DatabaseKey, KeyManagerError> {
        let mut database_key = Zeroizing::new(vec![0_u8; 32]);
        getrandom::fill(&mut database_key).map_err(|_| KeyManagerError::Randomness)?;
        self.publish_database_key(unlock_secret, database_key)
    }

    fn publish_database_key(
        &self,
        unlock_secret: DatabaseKey,
        database_key: DatabaseKey,
    ) -> Result<DatabaseKey, KeyManagerError> {
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
        self.store_committed_unlock_secret(&unlock_secret)?;
        Ok(database_key)
    }

    fn store_committed_unlock_secret(&self, unlock_secret: &[u8]) -> Result<(), KeyManagerError> {
        if let Err(store_error) = self.secrets.store(unlock_secret) {
            let committed = self.secrets.load().ok().flatten().and_then(|stored| {
                match decode_unlock_secret(stored) {
                    Ok(UnlockSecret::Committed(visible)) => Some(visible),
                    Ok(UnlockSecret::Pending(_)) | Err(_) => None,
                }
            });
            if committed
                .as_deref()
                .is_some_and(|visible| visible == unlock_secret)
            {
                return Ok(());
            }
            return Err(store_error);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SnapshotPresence {
    Missing,
    Present,
}

fn snapshot_presence(snapshot_path: &Path) -> Result<SnapshotPresence, KeyManagerError> {
    #[cfg(test)]
    if let Some(error_kind) = tests::take_snapshot_metadata_failure() {
        return classify_snapshot_metadata(Err(std::io::Error::from(error_kind)));
    }
    classify_snapshot_metadata(std::fs::symlink_metadata(snapshot_path))
}

fn classify_snapshot_metadata(
    metadata: std::io::Result<std::fs::Metadata>,
) -> Result<SnapshotPresence, KeyManagerError> {
    match metadata {
        Ok(metadata) if metadata.file_type().is_file() => Ok(SnapshotPresence::Present),
        Ok(_) => Err(KeyManagerError::RecoveryRequired(
            "vault path is not a regular file",
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(SnapshotPresence::Missing),
        Err(_) => Err(KeyManagerError::RecoveryRequired(
            "vault path could not be inspected",
        )),
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
        match std::fs::symlink_metadata(&lock_path) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Ok(_) | Err(_) => return Err(KeyManagerError::InitializationLock),
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
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

impl From<ProcedureOutput> for ExportedDatabaseKey {
    fn from(output: ProcedureOutput) -> Self {
        Self(Zeroizing::new(Vec::<u8>::from(output)))
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
    #[cfg(test)]
    if tests::take_legacy_migration_failure(tests::LegacyMigrationFailure::Commit) {
        return Err(KeyManagerError::Stronghold);
    }
    stronghold
        .commit_with_keyprovider(&SnapshotPath::from_path(&temporary_snapshot), provider)
        .map_err(|_| KeyManagerError::Stronghold)?;
    #[cfg(test)]
    if tests::take_legacy_migration_failure(tests::LegacyMigrationFailure::Publish) {
        return Err(KeyManagerError::Stronghold);
    }
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
        cell::Cell,
        io::ErrorKind,
        panic::{catch_unwind, AssertUnwindSafe},
        path::Path,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Barrier, Mutex,
        },
        thread,
        time::Duration,
    };
    use tempfile::tempdir;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum LegacyMigrationFailure {
        Commit,
        Publish,
    }

    thread_local! {
        static LEGACY_MIGRATION_FAILURE: Cell<Option<LegacyMigrationFailure>> = const { Cell::new(None) };
        static SNAPSHOT_METADATA_FAILURE: Cell<Option<ErrorKind>> = const { Cell::new(None) };
    }

    fn fail_next_legacy_migration_at(failure: LegacyMigrationFailure) {
        LEGACY_MIGRATION_FAILURE.with(|pending| pending.set(Some(failure)));
    }

    pub(super) fn take_legacy_migration_failure(failure: LegacyMigrationFailure) -> bool {
        LEGACY_MIGRATION_FAILURE.with(|pending| {
            if pending.get() == Some(failure) {
                pending.set(None);
                true
            } else {
                false
            }
        })
    }

    fn fail_next_snapshot_metadata_with(error_kind: ErrorKind) {
        SNAPSHOT_METADATA_FAILURE.with(|pending| pending.set(Some(error_kind)));
    }

    pub(super) fn take_snapshot_metadata_failure() -> Option<ErrorKind> {
        SNAPSHOT_METADATA_FAILURE.with(|pending| pending.take())
    }

    #[derive(Default)]
    struct RecordingSecretStore {
        inner: MemorySecretStore,
        stores: Mutex<Vec<super::DatabaseKey>>,
    }

    impl RecordingSecretStore {
        fn seed(&self, value: &[u8]) {
            self.inner.store(value).unwrap();
        }

        fn stored_values(&self) -> Vec<super::DatabaseKey> {
            self.stores.lock().unwrap().clone()
        }
    }

    impl SecretStore for RecordingSecretStore {
        fn load(&self) -> Result<Option<super::DatabaseKey>, KeyManagerError> {
            self.inner.load()
        }

        fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
            self.stores
                .lock()
                .map_err(|_| KeyManagerError::SecretStore)?
                .push(super::DatabaseKey::new(value.to_vec()));
            self.inner.store(value)
        }

        fn delete(&self) -> Result<(), KeyManagerError> {
            self.inner.delete()
        }
    }

    const TEST_UNLOCK_SECRET: &[u8; 32] = b"0123456789ABCDEF0123456789ABCDEF";

    fn pending_test_secret() -> super::DatabaseKey {
        super::encode_pending_unlock_secret(TEST_UNLOCK_SECRET)
    }

    fn assert_error_has_no_sensitive_data(error: &KeyManagerError, sensitive_path: &Path) {
        let rendered = error.to_string();
        let path = sensitive_path.to_string_lossy();
        let file_name = sensitive_path.file_name().unwrap().to_string_lossy();
        let unlock_secret = String::from_utf8_lossy(TEST_UNLOCK_SECRET);
        assert!(!rendered.contains(path.as_ref()));
        assert!(!rendered.contains(file_name.as_ref()));
        assert!(!rendered.contains(unlock_secret.as_ref()));
    }

    fn assert_recovery_error_is_redacted(
        error: &KeyManagerError,
        expected_reason: &'static str,
        sensitive_path: &Path,
    ) {
        assert!(matches!(
            error,
            KeyManagerError::RecoveryRequired(reason) if *reason == expected_reason
        ));
        assert_error_has_no_sensitive_data(error, sensitive_path);
    }

    fn snapshot_lock_path(snapshot_path: &Path) -> std::path::PathBuf {
        let mut lock_name = snapshot_path.file_name().unwrap().to_os_string();
        lock_name.push(".lock");
        snapshot_path.with_file_name(lock_name)
    }

    #[cfg(unix)]
    fn create_test_file_symlink(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).unwrap();
        true
    }

    #[cfg(windows)]
    fn create_test_file_symlink(target: &Path, link: &Path) -> bool {
        const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
        match std::os::windows::fs::symlink_file(target, link) {
            Ok(()) => true,
            Err(error)
                if error.kind() == ErrorKind::PermissionDenied
                    || error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) =>
            {
                false
            }
            Err(error) => panic!("failed to create test symlink: {error}"),
        }
    }

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

    #[derive(Clone, Copy)]
    enum FinalWriteFailureMode {
        BeforeWrite,
        AfterWrite,
    }

    struct FailFinalWriteStore {
        inner: MemorySecretStore,
        mode: FinalWriteFailureMode,
        store_calls: AtomicUsize,
    }

    impl FailFinalWriteStore {
        fn new(mode: FinalWriteFailureMode) -> Self {
            Self {
                inner: MemorySecretStore::default(),
                mode,
                store_calls: AtomicUsize::new(0),
            }
        }
    }

    impl SecretStore for FailFinalWriteStore {
        fn load(&self) -> Result<Option<super::DatabaseKey>, KeyManagerError> {
            self.inner.load()
        }

        fn store(&self, value: &[u8]) -> Result<(), KeyManagerError> {
            if self.store_calls.fetch_add(1, Ordering::SeqCst) == 1 {
                if matches!(self.mode, FinalWriteFailureMode::AfterWrite) {
                    self.inner.store(value)?;
                }
                return Err(KeyManagerError::SecretStore);
            }
            self.inner.store(value)
        }

        fn delete(&self) -> Result<(), KeyManagerError> {
            self.inner.delete()
        }
    }

    fn create_legacy_snapshot(
        snapshot: &std::path::Path,
    ) -> (super::DatabaseKey, super::DatabaseKey) {
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
            .commit_with_keyprovider(&SnapshotPath::from_path(snapshot), &provider)
            .unwrap();
        (unlock_secret, database_key)
    }

    fn read_legacy_store_key(
        snapshot: &std::path::Path,
        unlock_secret: &super::DatabaseKey,
    ) -> super::DatabaseKey {
        let stronghold = Stronghold::default();
        let provider = KeyProvider::try_from(unlock_secret.clone()).unwrap();
        stronghold
            .load_snapshot(&provider, &SnapshotPath::from_path(snapshot))
            .unwrap();
        let client = stronghold.load_client(CLIENT_PATH).unwrap();
        super::DatabaseKey::new(client.store().get(RECORD_PATH).unwrap().unwrap())
    }

    fn assert_legacy_migration_failure_is_recoverable(failure: LegacyMigrationFailure) {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("legacy-failure.stronghold");
        let (unlock_secret, database_key) = create_legacy_snapshot(&snapshot);
        let secrets = Arc::new(MemorySecretStore::default());
        secrets.store(&unlock_secret).unwrap();
        let manager = KeyManager::new(&snapshot, secrets.clone());

        fail_next_legacy_migration_at(failure);
        assert!(matches!(
            manager.database_key(),
            Err(KeyManagerError::Stronghold)
        ));
        assert_eq!(
            read_legacy_store_key(&snapshot, &unlock_secret).as_slice(),
            database_key.as_slice()
        );

        let migrated = manager.database_key().unwrap();
        assert_eq!(migrated.as_slice(), database_key.as_slice());
        let reopened = KeyManager::new(&snapshot, secrets).database_key().unwrap();
        assert_eq!(reopened.as_slice(), database_key.as_slice());
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
    fn memory_secret_store_uses_zeroizing_backing() {
        fn require_zeroizing_backing(_: &Mutex<Option<super::DatabaseKey>>) {}

        let store = MemorySecretStore::default();
        require_zeroizing_backing(&store.secret);
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
    fn failure_path_legacy_commit_failure_preserves_snapshot_and_retries() {
        assert_legacy_migration_failure_is_recoverable(LegacyMigrationFailure::Commit);
    }

    #[test]
    fn failure_path_legacy_publication_failure_preserves_snapshot_and_retries() {
        assert_legacy_migration_failure_is_recoverable(LegacyMigrationFailure::Publish);
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
    fn permission_denied_snapshot_metadata_preserves_pending_secret_without_store() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("sensitive-metadata-path.stronghold");
        let pending = pending_test_secret();
        let secrets = Arc::new(RecordingSecretStore::default());
        secrets.seed(&pending);
        let manager = KeyManager::new(&snapshot, secrets.clone());

        fail_next_snapshot_metadata_with(ErrorKind::PermissionDenied);
        let result = manager.database_key();
        let _ = take_snapshot_metadata_failure();
        let error = result.unwrap_err();

        assert_recovery_error_is_redacted(&error, "vault path could not be inspected", &snapshot);
        assert!(secrets.stored_values().is_empty());
        assert_eq!(
            secrets.inner.load().unwrap().unwrap().as_slice(),
            pending.as_slice()
        );
    }

    #[test]
    fn broken_snapshot_symlink_is_rejected_without_credential_mutation() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("sensitive-broken-link.stronghold");
        let missing_target = temp.path().join("missing-snapshot-target");
        if !create_test_file_symlink(&missing_target, &snapshot) {
            return;
        }
        let pending = pending_test_secret();
        let secrets = Arc::new(RecordingSecretStore::default());
        secrets.seed(&pending);

        let error = KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap_err();

        assert_recovery_error_is_redacted(&error, "vault path is not a regular file", &snapshot);
        assert!(secrets.stored_values().is_empty());
        assert_eq!(
            secrets.inner.load().unwrap().unwrap().as_slice(),
            pending.as_slice()
        );
    }

    #[test]
    fn missing_snapshot_with_pending_secret_reuses_identical_unlock_secret() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("pending-recovery.stronghold");
        let pending = pending_test_secret();
        let secrets = Arc::new(RecordingSecretStore::default());
        secrets.seed(&pending);

        let database_key = KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap();

        let stored_values = secrets.stored_values();
        assert_eq!(stored_values.len(), 1);
        assert_eq!(stored_values[0].as_slice(), TEST_UNLOCK_SECRET);
        assert_eq!(
            secrets.inner.load().unwrap().unwrap().as_slice(),
            TEST_UNLOCK_SECRET
        );
        let reopened = super::read_database_key(
            &snapshot,
            super::DatabaseKey::new(TEST_UNLOCK_SECRET.to_vec()),
        )
        .unwrap();
        assert_eq!(reopened.as_slice(), database_key.as_slice());
    }

    #[test]
    fn snapshot_directory_is_rejected_as_non_regular() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("sensitive-snapshot-directory");
        std::fs::create_dir(&snapshot).unwrap();
        let pending = pending_test_secret();
        let secrets = Arc::new(RecordingSecretStore::default());
        secrets.seed(&pending);

        let error = KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap_err();

        assert_recovery_error_is_redacted(&error, "vault path is not a regular file", &snapshot);
        assert!(secrets.stored_values().is_empty());
        assert_eq!(
            secrets.inner.load().unwrap().unwrap().as_slice(),
            pending.as_slice()
        );
    }

    #[test]
    fn symlinked_initialization_lock_is_rejected() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("sensitive-lock-snapshot.stronghold");
        let lock_path = snapshot_lock_path(&snapshot);
        let lock_target = temp.path().join("sensitive-lock-target");
        std::fs::write(&lock_target, b"existing lock target").unwrap();
        if !create_test_file_symlink(&lock_target, &lock_path) {
            return;
        }
        let pending = pending_test_secret();
        let secrets = Arc::new(RecordingSecretStore::default());
        secrets.seed(&pending);

        let error = KeyManager::new(&snapshot, secrets.clone())
            .database_key()
            .unwrap_err();

        assert!(matches!(error, KeyManagerError::InitializationLock));
        assert_error_has_no_sensitive_data(&error, &lock_path);
        assert!(secrets.stored_values().is_empty());
        assert_eq!(
            secrets.inner.load().unwrap().unwrap().as_slice(),
            pending.as_slice()
        );
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
    fn failure_path_final_credential_error_preserves_pending_key_for_retry() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("final-write-error.stronghold");
        let secrets = Arc::new(FailFinalWriteStore::new(FinalWriteFailureMode::BeforeWrite));
        let manager = KeyManager::new(&snapshot, secrets.clone());

        assert!(matches!(
            manager.database_key(),
            Err(KeyManagerError::SecretStore)
        ));
        assert!(snapshot.exists());

        let pending = secrets.inner.load().unwrap().unwrap();
        let unlock_secret = match super::decode_unlock_secret(pending).unwrap() {
            super::UnlockSecret::Pending(unlock_secret) => unlock_secret,
            super::UnlockSecret::Committed(_) => panic!("credential must remain pending"),
        };
        let expected = super::read_database_key(&snapshot, unlock_secret).unwrap();
        let recovered = manager.database_key().unwrap();
        assert_eq!(recovered.as_slice(), expected.as_slice());
        let reopened = KeyManager::new(&snapshot, secrets).database_key().unwrap();
        assert_eq!(reopened.as_slice(), expected.as_slice());
    }

    #[test]
    fn failure_path_visible_final_credential_is_reconciled_without_rotation() {
        let temp = tempdir().unwrap();
        let snapshot = temp.path().join("visible-final-write.stronghold");
        let secrets = Arc::new(FailFinalWriteStore::new(FinalWriteFailureMode::AfterWrite));
        let manager = KeyManager::new(&snapshot, secrets.clone());

        let database_key = manager.database_key().unwrap();
        let visible_secret = secrets.inner.load().unwrap().unwrap();
        assert_eq!(visible_secret.len(), 32);

        let reopened = KeyManager::new(&snapshot, secrets).database_key().unwrap();
        assert_eq!(reopened.as_slice(), database_key.as_slice());
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
            .split("\nmod tests {")
            .next()
            .unwrap();

        assert!(production.contains(".write_secret("));
        assert!(production.contains("ExportDatabaseKey"));
        assert!(!production.contains(".store()\n        .insert("));
        assert!(!production.contains("database_key.to_vec()"));
    }
}
