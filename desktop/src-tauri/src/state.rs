use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex as AsyncMutex;

use crate::sidecar::{
    PersistenceAwareEventSink, SidecarSupervisor, StorageHealth, StorageHealthTracker,
    TauriEventSink, TauriSidecarLauncher, TauriStorageHealthReporter, PRODUCTION_HANDSHAKE_TIMEOUT,
};
use crate::storage::{KeyManager, KeyringSecretStore, SessionRepository};

pub const LOCAL_WORKSPACE_ID: &str = "018f0000-0000-7000-8000-000000000099";
const KEYRING_SERVICE: &str = "com.callerinterview.desktop";
const KEYRING_ACCOUNT: &str = "sqlcipher-unlock";

#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error("application data directory is unavailable")]
    AppDataUnavailable,
    #[error("encrypted storage key is unavailable")]
    KeyUnavailable,
    #[error("encrypted session storage is unavailable")]
    RepositoryUnavailable,
}

pub struct AppState {
    pub workspace_id: String,
    pub repository: Arc<SessionRepository>,
    pub storage_health: StorageHealthTracker,
    pub sidecar: SidecarSupervisor,
    pub session_operation_gate: Arc<AsyncMutex<()>>,
}

impl AppState {
    pub fn new(app: AppHandle) -> Result<Self, AppStateError> {
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|_| AppStateError::AppDataUnavailable)?;
        std::fs::create_dir_all(&app_data).map_err(|_| AppStateError::AppDataUnavailable)?;

        let secret_store = Arc::new(KeyringSecretStore::new(KEYRING_SERVICE, KEYRING_ACCOUNT));
        let key_manager = KeyManager::new(app_data.join("storage.stronghold"), secret_store);
        let database_key = key_manager
            .database_key()
            .map_err(|_| AppStateError::KeyUnavailable)?;
        let repository = Arc::new(
            SessionRepository::open(app_data.join("sessions.db"), database_key.as_slice())
                .map_err(|_| AppStateError::RepositoryUnavailable)?,
        );

        let storage_health = StorageHealthTracker::new(StorageHealth::ready());
        let health_reporter = Arc::new(TauriStorageHealthReporter::new(
            app.clone(),
            storage_health.clone(),
        ));
        let live_sink = Arc::new(TauriEventSink::new(app.clone()));
        let durable_sink = Arc::new(PersistenceAwareEventSink::new(
            LOCAL_WORKSPACE_ID,
            repository.clone(),
            live_sink,
            health_reporter,
        ));
        let launcher = Arc::new(TauriSidecarLauncher::new(app));
        let sidecar = SidecarSupervisor::with_launcher_and_sink(
            launcher,
            durable_sink,
            Duration::from_secs(1),
            Some(PRODUCTION_HANDSHAKE_TIMEOUT),
        );

        Ok(Self {
            workspace_id: LOCAL_WORKSPACE_ID.into(),
            repository,
            storage_health,
            sidecar,
            session_operation_gate: Arc::new(AsyncMutex::new(())),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::{mpsc, Mutex as AsyncMutex, Notify};

    use super::{KEYRING_ACCOUNT, KEYRING_SERVICE, LOCAL_WORKSPACE_ID};

    #[test]
    fn production_storage_identity_is_stable_and_non_secret() {
        assert!(uuid::Uuid::parse_str(LOCAL_WORKSPACE_ID).is_ok());
        assert_eq!(KEYRING_SERVICE, "com.callerinterview.desktop");
        assert_eq!(KEYRING_ACCOUNT, "sqlcipher-unlock");
        assert!(!KEYRING_ACCOUNT.contains("key"));
    }

    #[tokio::test]
    async fn complete_and_delete_session_operations_are_serialized() {
        let gate = Arc::new(AsyncMutex::new(()));
        let release_complete = Arc::new(Notify::new());
        let (entered_sender, mut entered_receiver) = mpsc::unbounded_channel();

        let complete = tokio::spawn({
            let gate = gate.clone();
            let release_complete = release_complete.clone();
            let entered_sender = entered_sender.clone();
            async move {
                let _session_operation = gate.lock().await;
                entered_sender.send("complete").unwrap();
                release_complete.notified().await;
            }
        });
        assert_eq!(entered_receiver.recv().await, Some("complete"));

        let delete = tokio::spawn({
            let gate = gate.clone();
            async move {
                let _session_operation = gate.lock().await;
                entered_sender.send("delete").unwrap();
            }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), entered_receiver.recv())
                .await
                .is_err()
        );

        release_complete.notify_waiters();
        complete.await.unwrap();
        delete.await.unwrap();
        assert_eq!(entered_receiver.recv().await, Some("delete"));
    }
}
