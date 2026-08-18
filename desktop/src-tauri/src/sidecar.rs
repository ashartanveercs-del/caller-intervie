use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::ShellExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command as TokioCommand};
use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex};

use crate::protocol::{
    encode_frame, validate_command, validate_event, CommandKind, Envelope, EventKind, FrameDecoder,
    ProtocolKind, MAX_FRAME_BYTES, MAX_SAFE_INTEGER, PROTOCOL_VERSION,
};
use crate::storage::{
    AppendEventResult, NewTimelineEvent, RepositoryError, RequestTurnAssociation,
    SessionRepository, SessionStatus, TimelineEventKind,
};

pub const SIDECAR_PROGRAM: &str = "callerinterview-sidecar";
pub const SIDECAR_EVENT: &str = "sidecar://event";
pub const STORAGE_HEALTH_EVENT: &str = "storage://health";
pub const PRODUCTION_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_DIAGNOSTICS: usize = 20;
const MAX_PENDING_DURABLE_EVENTS: usize = 1_024;
const MAX_QUARANTINED_DURABLE_EVENTS: usize = 64;
const MAX_PERMANENT_ATTEMPTS: u8 = 3;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const STORAGE_PERSIST_TIMEOUT: Duration = Duration::from_secs(5);
const STORAGE_RETRY_DELAY: Duration = Duration::from_millis(250);
const EVENT_SINK_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
const PERSISTENCE_WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(5);
const CAPTURE_PROTECTION_STOP_TIMEOUT: Duration = Duration::from_secs(1);

pub fn packaged_sidecar_path(host_executable: &std::path::Path) -> std::path::PathBuf {
    let mut path = host_executable
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""))
        .join(SIDECAR_PROGRAM);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarState {
    Starting,
    Ready,
    Restarting,
    Stopped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStatus {
    pub state: SidecarState,
    pub restart_count: u8,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SidecarError {
    code: &'static str,
    message: String,
}

impl SidecarError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: redact_diagnostic(&message.into()),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SidecarError {}

#[async_trait]
pub trait SidecarPort: Send + Sync {
    async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError>;
    async fn kill(&self) -> Result<(), SidecarError>;
    fn is_poisoned(&self) -> bool {
        false
    }
    fn observe(&self, _supervisor: SidecarSupervisor, _generation: u64) {}
}

#[async_trait]
pub trait SidecarLauncher: Send + Sync {
    async fn launch(&self) -> Result<Arc<dyn SidecarPort>, SidecarError>;
}

#[async_trait]
pub trait SidecarEventSink: Send + Sync {
    async fn emit(&self, event: &Envelope) -> Result<(), SidecarError>;

    async fn shutdown(&self) -> Result<(), SidecarError> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageHealthStatus {
    Pending,
    Ready,
    Degraded,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageHealth {
    pub status: StorageHealthStatus,
    pub code: Option<&'static str>,
    pub message: Option<&'static str>,
    pub recoverable: bool,
}

impl StorageHealth {
    pub fn ready() -> Self {
        Self {
            status: StorageHealthStatus::Ready,
            code: None,
            message: None,
            recoverable: false,
        }
    }

    fn degraded(code: &'static str, recoverable: bool) -> Self {
        Self {
            status: StorageHealthStatus::Degraded,
            code: Some(code),
            message: Some(
                "Session history could not be saved. The live session is still available.",
            ),
            recoverable,
        }
    }
}

#[derive(Clone)]
pub struct StorageHealthTracker {
    current: Arc<std::sync::Mutex<StorageHealth>>,
}

impl StorageHealthTracker {
    pub fn new(initial: StorageHealth) -> Self {
        Self {
            current: Arc::new(std::sync::Mutex::new(initial)),
        }
    }

    pub fn current(&self) -> Result<StorageHealth, SidecarError> {
        self.current
            .lock()
            .map(|health| health.clone())
            .map_err(|_| {
                SidecarError::new(
                    "storage_health_unavailable",
                    "storage health is unavailable",
                )
            })
    }

    fn set(&self, health: StorageHealth) -> Result<(), SidecarError> {
        *self.current.lock().map_err(|_| {
            SidecarError::new(
                "storage_health_unavailable",
                "storage health is unavailable",
            )
        })? = health;
        Ok(())
    }
}

pub trait StorageHealthReporter: Send + Sync {
    fn report(&self, health: &StorageHealth) -> Result<(), SidecarError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurableStoreError {
    Transient,
    DependencyMissing,
    EventCollision,
    InvalidSession,
    TerminalConflict,
    InvalidEvent,
}

impl DurableStoreError {
    fn code(self, event: &Envelope) -> &'static str {
        match self {
            Self::Transient => PersistenceAwareEventSink::storage_failure_code(event),
            Self::DependencyMissing => "storage_dependency_missing",
            Self::EventCollision => "storage_event_collision",
            Self::InvalidSession => "storage_invalid_session",
            Self::TerminalConflict => "storage_terminal_conflict",
            Self::InvalidEvent => "storage_invalid_event",
        }
    }

    fn is_permanent(self) -> bool {
        matches!(
            self,
            Self::EventCollision
                | Self::InvalidSession
                | Self::TerminalConflict
                | Self::InvalidEvent
        )
    }
}

fn classify_repository_error(error: RepositoryError) -> DurableStoreError {
    match error {
        RepositoryError::EventContentCollision { .. }
        | RepositoryError::RequestTurnAssociationConflict => DurableStoreError::EventCollision,
        RepositoryError::NotFound => DurableStoreError::InvalidSession,
        RepositoryError::SessionCompletionConflict => DurableStoreError::TerminalConflict,
        RepositoryError::NonDurableEvent(_)
        | RepositoryError::InvalidSessionLimit { .. }
        | RepositoryError::InvalidCompletionStatus
        | RepositoryError::InvalidTerminalEventKind
        | RepositoryError::InvalidTranscriptAssociationEventKind
        | RepositoryError::TranscriptAssociationMismatch
        | RepositoryError::BriefSerialization
        | RepositoryError::Model(_) => DurableStoreError::InvalidEvent,
        RepositoryError::CipherUnavailable
        | RepositoryError::ConnectionUnavailable
        | RepositoryError::Migration(_)
        | RepositoryError::Sql(_) => DurableStoreError::Transient,
    }
}

pub trait DurableEventStore: Send + Sync {
    fn append_event(
        &self,
        event: &NewTimelineEvent,
    ) -> Result<AppendEventResult, DurableStoreError>;
    fn append_terminal_event(
        &self,
        event: &NewTimelineEvent,
        status: SessionStatus,
        completed_at_ms: i64,
    ) -> Result<AppendEventResult, DurableStoreError>;
    fn append_transcript_event_with_association(
        &self,
        event: &NewTimelineEvent,
        association: &RequestTurnAssociation,
    ) -> Result<AppendEventResult, DurableStoreError>;
    fn resolve_request_turn(
        &self,
        workspace_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<String>, DurableStoreError>;

    fn interrupt(&self) {}
}

impl DurableEventStore for SessionRepository {
    fn append_event(
        &self,
        event: &NewTimelineEvent,
    ) -> Result<AppendEventResult, DurableStoreError> {
        SessionRepository::append_event(self, event).map_err(classify_repository_error)
    }

    fn append_terminal_event(
        &self,
        event: &NewTimelineEvent,
        status: SessionStatus,
        completed_at_ms: i64,
    ) -> Result<AppendEventResult, DurableStoreError> {
        SessionRepository::append_terminal_event(self, event, status, completed_at_ms)
            .map_err(classify_repository_error)
    }

    fn append_transcript_event_with_association(
        &self,
        event: &NewTimelineEvent,
        association: &RequestTurnAssociation,
    ) -> Result<AppendEventResult, DurableStoreError> {
        SessionRepository::append_transcript_event_with_association(self, event, association)
            .map_err(classify_repository_error)
    }

    fn resolve_request_turn(
        &self,
        workspace_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<String>, DurableStoreError> {
        SessionRepository::resolve_request_turn(self, workspace_id, session_id, request_id)
            .map_err(classify_repository_error)
    }

    fn interrupt(&self) {
        SessionRepository::interrupt(self);
    }
}

#[derive(Default)]
struct NoopEventSink;

#[async_trait]
impl SidecarEventSink for NoopEventSink {
    async fn emit(&self, _event: &Envelope) -> Result<(), SidecarError> {
        Ok(())
    }
}

struct SupervisorData {
    state: SidecarState,
    restart_count: u8,
    generation: u64,
    explicit_shutdown: bool,
    diagnostics: Vec<String>,
    active: Option<ActiveSidecar>,
    intentional_exit_generations: BTreeSet<u64>,
}

struct ActiveSidecar {
    generation: u64,
    port: Arc<dyn SidecarPort>,
    decoder: FrameDecoder,
    stderr: String,
    handshake_id: Option<String>,
    pending_runtime_session: Option<PendingRuntimeSession>,
    runtime_session_id: Option<String>,
    pending_capture_protection_stop: Option<PendingCaptureProtectionStop>,
}

struct PendingRuntimeSession {
    session_id: String,
    command_id: String,
}

struct PendingCaptureProtectionStop {
    session_id: String,
    command_id: String,
    acknowledgement: oneshot::Sender<Result<(), SidecarError>>,
}

enum CaptureProtectionStopPlan {
    None,
    Graceful(String),
    Emergency,
}

fn apply_runtime_session_event(active: &mut ActiveSidecar, event: &Envelope) {
    if apply_capture_protection_stop_event(active, event) {
        return;
    }
    if matches!(&event.kind, ProtocolKind::Event(EventKind::RuntimeError)) {
        let clears_pending_start = active
            .pending_runtime_session
            .as_ref()
            .is_some_and(|pending| {
                event.correlation_id.as_deref() == Some(pending.command_id.as_str())
            });
        if clears_pending_start {
            active.pending_runtime_session = None;
        }
        return;
    }
    if !matches!(&event.kind, ProtocolKind::Event(EventKind::SessionState)) {
        return;
    }
    let state = event.payload.get("state").and_then(Value::as_str);
    let session_id = event.session_id.as_deref();
    match state {
        Some("starting" | "listening" | "paused") => {
            let Some(session_id) = session_id else {
                return;
            };
            let confirms_pending = active
                .pending_runtime_session
                .as_ref()
                .is_some_and(|pending| {
                    pending.session_id == session_id
                        && event.correlation_id.as_deref() == Some(pending.command_id.as_str())
                });
            if confirms_pending || active.runtime_session_id.as_deref() == Some(session_id) {
                active.runtime_session_id = Some(session_id.to_owned());
                active.pending_runtime_session = None;
            }
        }
        Some("stopped" | "error") => {
            let Some(session_id) = session_id else {
                return;
            };
            if active
                .pending_runtime_session
                .as_ref()
                .is_some_and(|pending| pending.session_id == session_id)
                || active.runtime_session_id.as_deref() == Some(session_id)
            {
                active.pending_runtime_session = None;
                active.runtime_session_id = None;
            }
        }
        Some("idle") if session_id.is_none() => {
            active.runtime_session_id = None;
        }
        _ => {}
    }
}

fn apply_capture_protection_stop_event(active: &mut ActiveSidecar, event: &Envelope) -> bool {
    let Some(pending) = active.pending_capture_protection_stop.as_ref() else {
        return false;
    };
    let correlation_matches = event.correlation_id.as_deref() == Some(pending.command_id.as_str());
    if matches!(&event.kind, ProtocolKind::Event(EventKind::RuntimeError)) {
        if correlation_matches {
            let pending = active
                .pending_capture_protection_stop
                .take()
                .expect("capture stop acknowledgement must remain registered");
            let _ = pending.acknowledgement.send(Err(SidecarError::new(
                "sidecar_stop_failed",
                "The sidecar reported an error while stopping capture.",
            )));
            return true;
        }
        return false;
    }
    if !matches!(&event.kind, ProtocolKind::Event(EventKind::SessionState)) {
        return false;
    }

    let state = event.payload.get("state").and_then(Value::as_str);
    let session_matches = event.session_id.as_deref() == Some(pending.session_id.as_str());
    if session_matches && correlation_matches && matches!(state, Some("stopped" | "error")) {
        let pending = active
            .pending_capture_protection_stop
            .take()
            .expect("capture stop acknowledgement must remain registered");
        if state == Some("stopped") {
            active.pending_runtime_session = None;
            active.runtime_session_id = None;
            let _ = pending.acknowledgement.send(Ok(()));
        } else {
            let _ = pending.acknowledgement.send(Err(SidecarError::new(
                "sidecar_stop_failed",
                "The sidecar reported an error while stopping capture.",
            )));
        }
        return true;
    }

    session_matches && matches!(state, Some("stopping" | "stopped" | "error"))
        || state == Some("idle")
}

fn validate_runtime_command(
    active: &ActiveSidecar,
    command_kind: &ProtocolKind,
    command_session_id: Option<&str>,
) -> Result<(), SidecarError> {
    let ProtocolKind::Command(command_kind) = command_kind else {
        return Err(SidecarError::new(
            "invalid_command_kind",
            "sidecar command validation failed",
        ));
    };
    if matches!(
        command_kind,
        CommandKind::HandshakeRequest | CommandKind::SessionSnapshotRequest
    ) {
        return Ok(());
    }
    let requested = command_session_id.ok_or_else(|| {
        SidecarError::new("invalid_session_id", "sidecar command validation failed")
    })?;
    let active_session = active.runtime_session_id.as_deref();
    let pending_session = active
        .pending_runtime_session
        .as_ref()
        .map(|pending| pending.session_id.as_str());

    match command_kind {
        CommandKind::SessionStart => {
            if active_session.is_some() || pending_session.is_some() {
                Err(SidecarError::new(
                    "runtime_session_conflict",
                    "A runtime session is already active or starting.",
                ))
            } else {
                Ok(())
            }
        }
        CommandKind::QueryTrigger => match active_session {
            None => Err(SidecarError::new(
                "query_runtime_session_inactive",
                "No active runtime session can accept this query.",
            )),
            Some(current) if current != requested => Err(SidecarError::new(
                "query_runtime_session_mismatch",
                "The query does not target the active runtime session.",
            )),
            Some(_) => Ok(()),
        },
        CommandKind::SessionStop
        | CommandKind::ListeningSet
        | CommandKind::YouSourceSet
        | CommandKind::AudioSystemSet
        | CommandKind::AudioDeviceSet
        | CommandKind::KnowledgeIngest => {
            if active_session.is_none() && pending_session.is_none() {
                Err(SidecarError::new(
                    "runtime_session_inactive",
                    "No tracked runtime session can accept this command.",
                ))
            } else if active_session == Some(requested) || pending_session == Some(requested) {
                Ok(())
            } else {
                Err(SidecarError::new(
                    "runtime_session_mismatch",
                    "The command does not target the tracked runtime session.",
                ))
            }
        }
        CommandKind::HandshakeRequest | CommandKind::SessionSnapshotRequest => Ok(()),
    }
}

impl Default for SupervisorData {
    fn default() -> Self {
        Self {
            state: SidecarState::Starting,
            restart_count: 0,
            generation: 0,
            explicit_shutdown: false,
            diagnostics: Vec::new(),
            active: None,
            intentional_exit_generations: BTreeSet::new(),
        }
    }
}

struct SupervisorInner {
    data: AsyncMutex<SupervisorData>,
    lifecycle: AsyncMutex<()>,
    commands: AsyncMutex<()>,
    capture_protection_stop: AsyncMutex<()>,
    delivery: AsyncMutex<DeliveryState>,
    launcher: Option<Arc<dyn SidecarLauncher>>,
    sink: Arc<dyn SidecarEventSink>,
    restart_delay: Duration,
    handshake_timeout: Option<Duration>,
    authorization_timeout: Duration,
}

#[derive(Default)]
struct DeliveryState {
    tail: Option<oneshot::Receiver<()>>,
}

#[derive(Clone)]
pub struct SidecarSupervisor {
    inner: Arc<SupervisorInner>,
}

impl SidecarSupervisor {
    pub fn with_port(port: Arc<dyn SidecarPort>) -> Self {
        Self {
            inner: Arc::new(SupervisorInner {
                data: AsyncMutex::new(SupervisorData {
                    active: Some(ActiveSidecar {
                        generation: 0,
                        port,
                        decoder: FrameDecoder::new(MAX_FRAME_BYTES),
                        stderr: String::new(),
                        handshake_id: None,
                        pending_runtime_session: None,
                        runtime_session_id: None,
                        pending_capture_protection_stop: None,
                    }),
                    ..SupervisorData::default()
                }),
                lifecycle: AsyncMutex::new(()),
                commands: AsyncMutex::new(()),
                capture_protection_stop: AsyncMutex::new(()),
                delivery: AsyncMutex::new(DeliveryState::default()),
                launcher: None,
                sink: Arc::new(NoopEventSink),
                restart_delay: Duration::ZERO,
                handshake_timeout: None,
                authorization_timeout: AUTHORIZATION_TIMEOUT,
            }),
        }
    }

    pub fn with_launcher(launcher: Arc<dyn SidecarLauncher>, restart_delay: Duration) -> Self {
        Self::with_launcher_and_sink(launcher, Arc::new(NoopEventSink), restart_delay, None)
    }

    pub fn with_launcher_and_sink(
        launcher: Arc<dyn SidecarLauncher>,
        sink: Arc<dyn SidecarEventSink>,
        restart_delay: Duration,
        handshake_timeout: Option<Duration>,
    ) -> Self {
        Self::with_launcher_and_sink_and_authorization_timeout(
            launcher,
            sink,
            restart_delay,
            handshake_timeout,
            AUTHORIZATION_TIMEOUT,
        )
    }

    fn with_launcher_and_sink_and_authorization_timeout(
        launcher: Arc<dyn SidecarLauncher>,
        sink: Arc<dyn SidecarEventSink>,
        restart_delay: Duration,
        handshake_timeout: Option<Duration>,
        authorization_timeout: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(SupervisorInner {
                data: AsyncMutex::new(SupervisorData::default()),
                lifecycle: AsyncMutex::new(()),
                commands: AsyncMutex::new(()),
                capture_protection_stop: AsyncMutex::new(()),
                delivery: AsyncMutex::new(DeliveryState::default()),
                launcher: Some(launcher),
                sink,
                restart_delay,
                handshake_timeout,
                authorization_timeout,
            }),
        }
    }

    pub async fn start(&self) -> Result<SidecarStatus, SidecarError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let _commands = self.inner.commands.lock().await;
        let _delivery = self.inner.delivery.lock().await;
        if let Err(error) = self.launch_internal().await {
            self.fail(&error).await;
            return Err(error);
        }
        Ok(self.status().await)
    }

    pub async fn status(&self) -> SidecarStatus {
        let data = self.inner.data.lock().await;
        SidecarStatus {
            state: data.state.clone(),
            restart_count: data.restart_count,
            diagnostics: data.diagnostics.clone(),
        }
    }

    pub async fn current_runtime_session(&self) -> Option<String> {
        let data = self.inner.data.lock().await;
        if !matches!(data.state, SidecarState::Ready) {
            return None;
        }
        data.active
            .as_ref()
            .and_then(|active| active.runtime_session_id.clone())
    }

    async fn capture_protection_stop_plan(&self) -> CaptureProtectionStopPlan {
        let data = self.inner.data.lock().await;
        let Some(active) = data.active.as_ref() else {
            return CaptureProtectionStopPlan::None;
        };
        if !matches!(data.state, SidecarState::Ready) {
            return CaptureProtectionStopPlan::Emergency;
        }
        active
            .runtime_session_id
            .clone()
            .or_else(|| {
                active
                    .pending_runtime_session
                    .as_ref()
                    .map(|pending| pending.session_id.clone())
            })
            .map_or(
                CaptureProtectionStopPlan::None,
                CaptureProtectionStopPlan::Graceful,
            )
    }

    pub async fn send(&self, command: Envelope) -> Result<(), SidecarError> {
        self.send_with_authorization(command, async { Ok(()) })
            .await
    }

    pub async fn stop_for_capture_protection_loss(&self) -> Result<(), SidecarError> {
        let supervisor = self.clone();
        let (result_sender, result_receiver) = oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let result = supervisor.stop_for_capture_protection_loss_owned().await;
            let _ = result_sender.send(result);
        });
        result_receiver.await.map_err(|_| {
            SidecarError::new(
                "sidecar_stop_failed",
                "The sidecar capture cleanup task failed.",
            )
        })?
    }

    async fn stop_for_capture_protection_loss_owned(&self) -> Result<(), SidecarError> {
        let _stop = self.inner.capture_protection_stop.lock().await;
        let session_id = match self.capture_protection_stop_plan().await {
            CaptureProtectionStopPlan::None => return Ok(()),
            CaptureProtectionStopPlan::Graceful(session_id) => session_id,
            CaptureProtectionStopPlan::Emergency => {
                return self
                    .shutdown_after_capture_protection_loss(SidecarError::new(
                        "sidecar_stop_failed",
                        "The sidecar was not ready for protected session cleanup.",
                    ))
                    .await;
            }
        };
        let command = Envelope {
            version: PROTOCOL_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            session_id: Some(session_id),
            sequence: 0,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(MAX_SAFE_INTEGER),
            kind: CommandKind::SessionStop.into(),
            payload: Default::default(),
            correlation_id: None,
        };
        let (acknowledgement_sender, acknowledgement_receiver) = oneshot::channel();

        match tokio::time::timeout(CAPTURE_PROTECTION_STOP_TIMEOUT, async {
            self.dispatch_owned(
                command,
                None,
                async { Ok(()) },
                Some(acknowledgement_sender),
            )
            .await?;
            acknowledgement_receiver.await.map_err(|_| {
                SidecarError::new(
                    "sidecar_stop_failed",
                    "The sidecar stopped before acknowledging capture cleanup.",
                )
            })?
        })
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => self.shutdown_after_capture_protection_loss(error).await,
            Err(_) => {
                self.shutdown_after_capture_protection_loss(SidecarError::new(
                    "sidecar_stop_timeout",
                    "sidecar session stop timed out",
                ))
                .await
            }
        }
    }

    async fn shutdown_after_capture_protection_loss(
        &self,
        dispatch_error: SidecarError,
    ) -> Result<(), SidecarError> {
        match self.emergency_shutdown_for_capture_protection_loss().await {
            Ok(()) => Err(dispatch_error),
            Err(shutdown_error) => Err(shutdown_error),
        }
    }

    pub async fn send_with_authorization<A>(
        &self,
        command: Envelope,
        authorization: A,
    ) -> Result<(), SidecarError>
    where
        A: Future<Output = Result<(), SidecarError>> + Send + 'static,
    {
        self.spawn_owned_dispatch(command, None, authorization, ())
            .await
    }

    pub async fn send_with_authorization_and_gate<A>(
        &self,
        command: Envelope,
        session_operation_gate: Arc<AsyncMutex<()>>,
        authorization: A,
    ) -> Result<(), SidecarError>
    where
        A: Future<Output = Result<(), SidecarError>> + Send + 'static,
    {
        self.send_with_authorization_and_gate_with_keepalive(
            command,
            session_operation_gate,
            authorization,
            (),
        )
        .await
    }

    pub async fn send_with_authorization_and_gate_with_keepalive<A, K>(
        &self,
        command: Envelope,
        session_operation_gate: Arc<AsyncMutex<()>>,
        authorization: A,
        keepalive: K,
    ) -> Result<(), SidecarError>
    where
        A: Future<Output = Result<(), SidecarError>> + Send + 'static,
        K: Send + 'static,
    {
        self.spawn_owned_dispatch(
            command,
            Some(session_operation_gate),
            authorization,
            keepalive,
        )
        .await
    }

    async fn spawn_owned_dispatch<A, K>(
        &self,
        command: Envelope,
        session_operation_gate: Option<Arc<AsyncMutex<()>>>,
        authorization: A,
        keepalive: K,
    ) -> Result<(), SidecarError>
    where
        A: Future<Output = Result<(), SidecarError>> + Send + 'static,
        K: Send + 'static,
    {
        let supervisor = self.clone();
        let (result_sender, result_receiver) = oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let result = supervisor
                .dispatch_owned(command, session_operation_gate, authorization, None)
                .await;
            drop(keepalive);
            let _ = result_sender.send(result);
        });
        result_receiver.await.map_err(|_| {
            SidecarError::new(
                "sidecar_dispatch_failed",
                "The sidecar command dispatch task failed.",
            )
        })?
    }

    async fn dispatch_owned<A>(
        &self,
        command: Envelope,
        session_operation_gate: Option<Arc<AsyncMutex<()>>>,
        authorization: A,
        capture_stop_acknowledgement: Option<oneshot::Sender<Result<(), SidecarError>>>,
    ) -> Result<(), SidecarError>
    where
        A: Future<Output = Result<(), SidecarError>> + Send,
    {
        let _session_operation = match session_operation_gate {
            Some(gate) => Some(gate.lock_owned().await),
            None => None,
        };
        validate_command(&command).map_err(|error| {
            SidecarError::new(error.code(), "sidecar command validation failed")
        })?;
        let bytes = encode_frame(&command)
            .map_err(|error| SidecarError::new(error.code(), error.to_string()))?;
        let command_id = command.id.clone();
        let command_kind = command.kind.clone();
        let command_session_id = command.session_id.clone();
        let initial_generation = {
            let data = self.inner.data.lock().await;
            if !matches!(data.state, SidecarState::Ready) {
                return Err(SidecarError::new(
                    "sidecar_not_ready",
                    "sidecar has not completed its handshake",
                ));
            }
            let active = data.active.as_ref().ok_or_else(|| {
                SidecarError::new("sidecar_unavailable", "sidecar port is unavailable")
            })?;
            validate_runtime_command(active, &command_kind, command_session_id.as_deref())?;
            active.generation
        };

        tokio::time::timeout(self.inner.authorization_timeout, authorization)
            .await
            .map_err(|_| {
                SidecarError::new(
                    "sidecar_authorization_timeout",
                    "Sidecar command authorization timed out.",
                )
            })??;

        let _commands = self.inner.commands.lock().await;
        let (port, generation) = {
            let data = self.inner.data.lock().await;
            if !matches!(data.state, SidecarState::Ready) || data.generation != initial_generation {
                return Err(SidecarError::new(
                    "sidecar_runtime_changed",
                    "The sidecar runtime changed before command dispatch.",
                ));
            }
            let active = data.active.as_ref().ok_or_else(|| {
                SidecarError::new(
                    "sidecar_runtime_changed",
                    "The sidecar runtime changed before command dispatch.",
                )
            })?;
            if active.generation != initial_generation {
                return Err(SidecarError::new(
                    "sidecar_runtime_changed",
                    "The sidecar runtime changed before command dispatch.",
                ));
            }
            validate_runtime_command(active, &command_kind, command_session_id.as_deref())?;
            (active.port.clone(), active.generation)
        };

        let mut capture_stop_acknowledgement = capture_stop_acknowledgement;
        match port.write(bytes).await {
            Ok(()) => {
                let mut data = self.inner.data.lock().await;
                if data.generation != generation || !matches!(data.state, SidecarState::Ready) {
                    return Err(SidecarError::new(
                        "sidecar_runtime_changed",
                        "The sidecar runtime changed during command dispatch.",
                    ));
                }
                let active = data
                    .active
                    .as_mut()
                    .filter(|active| active.generation == generation)
                    .ok_or_else(|| {
                        SidecarError::new(
                            "sidecar_runtime_changed",
                            "The sidecar runtime changed during command dispatch.",
                        )
                    })?;
                match command_kind {
                    ProtocolKind::Command(CommandKind::SessionStart) => {
                        active.pending_runtime_session =
                            command_session_id.map(|session_id| PendingRuntimeSession {
                                session_id,
                                command_id,
                            });
                    }
                    ProtocolKind::Command(CommandKind::SessionStop) => {
                        if let Some(acknowledgement) = capture_stop_acknowledgement.take() {
                            let session_id = command_session_id.ok_or_else(|| {
                                SidecarError::new(
                                    "invalid_session_id",
                                    "sidecar command validation failed",
                                )
                            })?;
                            active.pending_capture_protection_stop =
                                Some(PendingCaptureProtectionStop {
                                    session_id,
                                    command_id,
                                    acknowledgement,
                                });
                        }
                    }
                    _ => {}
                }
                Ok(())
            }
            Err(error) => {
                if port.is_poisoned() {
                    self.handle_poisoned_write(generation, &error).await;
                }
                Err(error)
            }
        }
    }

    pub async fn accept_stdout(&self, generation: u64, chunk: &[u8]) -> Result<(), SidecarError> {
        let (delivery_result, mut first_error) = {
            let _commands = self.inner.commands.lock().await;
            let mut delivery = self.inner.delivery.lock().await;
            let envelopes = {
                let mut data = self.inner.data.lock().await;
                if data.generation != generation {
                    return Ok(());
                }
                let Some(active) = data.active.as_mut() else {
                    return Ok(());
                };
                if active.generation != generation {
                    return Ok(());
                }
                active
                    .decoder
                    .push(chunk)
                    .map_err(|error| SidecarError::new(error.code(), error.to_string()))?
            };
            let mut first_error = None;
            let mut accepted_events = Vec::new();
            for event in envelopes {
                let accepted_result = {
                    let mut data = self.inner.data.lock().await;
                    self.accept_event_locked(&mut data, generation, event)
                };
                let accepted = match accepted_result {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        first_error.get_or_insert(error);
                        continue;
                    }
                };
                if let Some(event) = accepted {
                    accepted_events.push(event);
                }
            }
            let delivery_result = (!accepted_events.is_empty())
                .then(|| self.queue_delivery_locked(&mut delivery, accepted_events));
            (delivery_result, first_error)
        };
        if let Some(delivery_result) = delivery_result {
            let result = delivery_result.await.unwrap_or_else(|_| {
                Err(SidecarError::new(
                    "sidecar_delivery_failed",
                    "Sidecar event delivery task failed.",
                ))
            });
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub async fn accept_event(&self, generation: u64, event: Envelope) -> Result<(), SidecarError> {
        let delivery_result = {
            let _commands = self.inner.commands.lock().await;
            let mut delivery = self.inner.delivery.lock().await;
            let accepted = {
                let mut data = self.inner.data.lock().await;
                self.accept_event_locked(&mut data, generation, event)?
            };
            accepted.map(|event| self.queue_delivery_locked(&mut delivery, vec![event]))
        };
        if let Some(delivery_result) = delivery_result {
            return delivery_result.await.map_err(|_| {
                SidecarError::new(
                    "sidecar_delivery_failed",
                    "Sidecar event delivery task failed.",
                )
            })?;
        }
        Ok(())
    }

    fn queue_delivery_locked(
        &self,
        delivery: &mut DeliveryState,
        events: Vec<Envelope>,
    ) -> oneshot::Receiver<Result<(), SidecarError>> {
        let previous = delivery.tail.take();
        let (completion_sender, completion_receiver) = oneshot::channel();
        let (result_sender, result_receiver) = oneshot::channel();
        delivery.tail = Some(completion_receiver);
        let sink = self.inner.sink.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            let mut first_error = None;
            for event in events {
                if let Err(error) = sink.emit(&event).await {
                    first_error.get_or_insert(error);
                }
            }
            let result = first_error.map_or(Ok(()), Err);
            let _ = result_sender.send(result);
            let _ = completion_sender.send(());
        });
        result_receiver
    }

    fn accept_event_locked(
        &self,
        data: &mut SupervisorData,
        generation: u64,
        event: Envelope,
    ) -> Result<Option<Envelope>, SidecarError> {
        let Some(active) = data.active.as_ref() else {
            return Ok(None);
        };
        if active.generation != generation || data.generation != generation {
            return Ok(None);
        }
        validate_event(&event)
            .map_err(|error| SidecarError::new(error.code(), "sidecar event validation failed"))?;
        let is_ready = matches!(
            &event.kind,
            crate::protocol::ProtocolKind::Event(EventKind::SidecarReady)
        );
        if is_ready {
            if active.handshake_id.as_deref() != event.correlation_id.as_deref() {
                return Ok(None);
            }
            if matches!(data.state, SidecarState::Ready) {
                return Ok(None);
            }
            data.state = SidecarState::Ready;
        } else if !matches!(data.state, SidecarState::Ready) {
            return Err(SidecarError::new(
                "sidecar_not_ready",
                "sidecar emitted an event before sidecar.ready",
            ));
        }
        if let Some(active) = data.active.as_mut() {
            apply_runtime_session_event(active, &event);
        }
        Ok(Some(event))
    }

    pub async fn accept_stderr(&self, generation: u64, stderr: &[u8]) {
        let mut data = self.inner.data.lock().await;
        if data.generation != generation {
            return;
        }
        let Some(active) = data.active.as_mut() else {
            return;
        };
        if active.generation != generation {
            return;
        }
        active.stderr.push_str(&String::from_utf8_lossy(stderr));
        let mut lines = Vec::new();
        while let Some(end) = active.stderr.find(['\n', '\r']) {
            lines.push(active.stderr.drain(..=end).collect::<String>());
        }
        for _line in lines {
            self.push_diagnostic_locked(&mut data, "sidecar stderr output received");
        }
    }

    pub async fn handle_unexpected_exit(&self, generation: u64) {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let _commands = self.inner.commands.lock().await;
        let _delivery = self.inner.delivery.lock().await;
        let restart = {
            let mut data = self.inner.data.lock().await;
            if data.intentional_exit_generations.remove(&generation) {
                return;
            }
            let Some(active) = data.active.as_ref() else {
                return;
            };
            if active.generation != generation || data.generation != generation {
                return;
            }
            if data.explicit_shutdown {
                data.state = SidecarState::Stopped;
                return;
            }
            self.flush_stderr_locked(&mut data, generation);
            data.active = None;
            true
        };
        if restart {
            self.restart_after_unexpected_exit_internal().await;
        }
    }

    async fn handle_poisoned_write(&self, generation: u64, error: &SidecarError) {
        let mut data = self.inner.data.lock().await;
        let Some(active) = data.active.as_ref() else {
            return;
        };
        if active.generation != generation || data.generation != generation {
            return;
        }
        data.state = SidecarState::Failed;
        self.push_diagnostic_locked(&mut data, &error.to_string());
        if error.code() != "sidecar_cleanup_failed" {
            data.active = None;
        }
    }

    async fn handle_poisoned_exit(&self, generation: u64) {
        let _commands = self.inner.commands.lock().await;
        let _delivery = self.inner.delivery.lock().await;
        let mut data = self.inner.data.lock().await;
        let Some(active) = data.active.as_ref() else {
            return;
        };
        if active.generation != generation || data.generation != generation {
            return;
        }
        self.flush_stderr_locked(&mut data, generation);
        data.active = None;
        data.state = SidecarState::Failed;
        self.push_diagnostic_locked(
            &mut data,
            "sidecar stdin write failed; child was terminated",
        );
    }

    async fn restart_after_unexpected_exit_internal(&self) {
        {
            let mut data = self.inner.data.lock().await;
            if data.restart_count >= 1 {
                data.state = SidecarState::Failed;
                self.push_diagnostic_locked(&mut data, "sidecar exited after automatic restart");
                return;
            }
            data.restart_count = 1;
            data.state = SidecarState::Restarting;
            self.push_diagnostic_locked(&mut data, "sidecar exited unexpectedly; restarting once");
        }

        if !self.inner.restart_delay.is_zero() {
            tokio::time::sleep(self.inner.restart_delay).await;
        }
        if let Err(error) = self.launch_internal().await {
            let mut data = self.inner.data.lock().await;
            data.state = SidecarState::Failed;
            self.push_diagnostic_locked(&mut data, &error.to_string());
        }
    }

    pub async fn restart(&self) -> Result<SidecarStatus, SidecarError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let _commands = self.inner.commands.lock().await;
        let _delivery = self.inner.delivery.lock().await;
        self.stop_current_for_restart_internal().await?;
        {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = false;
            data.state = SidecarState::Starting;
            data.restart_count = 0;
        }
        if let Err(error) = self.launch_internal().await {
            self.fail(&error).await;
            return Err(error);
        }
        Ok(self.status().await)
    }

    pub async fn shutdown(&self) -> Result<(), SidecarError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let _commands = self.inner.commands.lock().await;
        let _delivery = self.inner.delivery.lock().await;
        let generation = {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = true;
            data.state = SidecarState::Stopped;
            data.active.as_ref().map(|active| active.generation)
        };
        let sink_result =
            match tokio::time::timeout(EVENT_SINK_SHUTDOWN_TIMEOUT, self.inner.sink.shutdown())
                .await
            {
                Ok(result) => result,
                Err(_) => Err(SidecarError::new(
                    "sidecar_sink_shutdown_timeout",
                    "sidecar event delivery did not stop in time",
                )),
            };
        let cleanup_result = match generation {
            Some(generation) => self.cleanup_generation(generation).await,
            None => Ok(()),
        };
        sink_result?;
        cleanup_result
    }

    async fn emergency_shutdown_for_capture_protection_loss(&self) -> Result<(), SidecarError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let _commands = self.inner.commands.lock().await;
        let _delivery = self.inner.delivery.lock().await;
        let active = {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = true;
            data.state = SidecarState::Stopped;
            let active = data.active.take();
            if let Some(active) = active.as_ref() {
                data.intentional_exit_generations.insert(active.generation);
            }
            active
        };
        let sink_result =
            match tokio::time::timeout(EVENT_SINK_SHUTDOWN_TIMEOUT, self.inner.sink.shutdown())
                .await
            {
                Ok(result) => result,
                Err(_) => Err(SidecarError::new(
                    "sidecar_sink_shutdown_timeout",
                    "sidecar event delivery did not stop in time",
                )),
            };
        let cleanup_result = match active {
            Some(active) => self.emergency_detach_generation(active).await,
            None => Ok(()),
        };
        sink_result?;
        cleanup_result
    }

    async fn launch_internal(&self) -> Result<(), SidecarError> {
        let launcher = self.inner.launcher.as_ref().ok_or_else(|| {
            SidecarError::new("sidecar_unavailable", "sidecar launcher is unavailable")
        })?;
        let port = launcher.launch().await?;
        let handshake_id = uuid::Uuid::new_v4().to_string();
        let generation = {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = false;
            data.state = SidecarState::Starting;
            data.generation += 1;
            let generation = data.generation;
            data.active = Some(ActiveSidecar {
                generation,
                port: port.clone(),
                decoder: FrameDecoder::new(MAX_FRAME_BYTES),
                stderr: String::new(),
                handshake_id: Some(handshake_id.clone()),
                pending_runtime_session: None,
                runtime_session_id: None,
                pending_capture_protection_stop: None,
            });
            generation
        };
        port.observe(self.clone(), generation);
        if let Err(error) = self.send_handshake(port.clone(), handshake_id).await {
            self.cleanup_generation(generation).await?;
            return Err(error);
        }
        self.arm_handshake_timeout(generation);
        Ok(())
    }

    async fn send_handshake(
        &self,
        port: Arc<dyn SidecarPort>,
        handshake_id: String,
    ) -> Result<(), SidecarError> {
        let command = Envelope {
            version: crate::protocol::PROTOCOL_VERSION,
            id: handshake_id,
            session_id: None,
            sequence: 0,
            timestamp_ms: 0,
            kind: CommandKind::HandshakeRequest.into(),
            payload: Default::default(),
            correlation_id: None,
        };
        let bytes = encode_frame(&command)
            .map_err(|error| SidecarError::new(error.code(), error.to_string()))?;
        port.write(bytes).await
    }

    fn arm_handshake_timeout(&self, generation: u64) {
        let Some(timeout) = self.inner.handshake_timeout else {
            return;
        };
        let supervisor = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(timeout).await;
            supervisor.handle_handshake_timeout(generation).await;
        });
    }

    async fn handle_handshake_timeout(&self, generation: u64) {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let _commands = self.inner.commands.lock().await;
        let _delivery = self.inner.delivery.lock().await;
        let active = {
            let mut data = self.inner.data.lock().await;
            let Some(active) = data.active.as_ref() else {
                return;
            };
            if active.generation != generation
                || data.generation != generation
                || !matches!(data.state, SidecarState::Starting)
            {
                return;
            }
            data.state = SidecarState::Restarting;
            self.push_diagnostic_locked(&mut data, "sidecar handshake timed out");
            data.active.as_ref().map(|active| active.generation)
        };
        if let Some(generation) = active {
            if self.cleanup_generation(generation).await.is_err() {
                return;
            }
        }
        self.restart_after_unexpected_exit_internal().await;
    }

    async fn stop_current_for_restart_internal(&self) -> Result<(), SidecarError> {
        let generation = {
            let data = self.inner.data.lock().await;
            data.active.as_ref().map(|active| active.generation)
        };
        if let Some(generation) = generation {
            self.cleanup_generation(generation).await?;
        }
        Ok(())
    }

    async fn cleanup_generation(&self, generation: u64) -> Result<(), SidecarError> {
        let active = {
            let mut data = self.inner.data.lock().await;
            let Some(active) = data.active.take() else {
                return Ok(());
            };
            if active.generation != generation || data.generation != generation {
                data.active = Some(active);
                return Ok(());
            }
            data.intentional_exit_generations.insert(generation);
            active
        };
        match active.port.kill().await {
            Ok(()) => Ok(()),
            Err(error) => {
                let cleanup_error = SidecarError::new(
                    "sidecar_cleanup_failed",
                    format!("sidecar cleanup failed: {error}"),
                );
                let mut data = self.inner.data.lock().await;
                if data.generation == generation && data.active.is_none() {
                    data.active = Some(active);
                }
                data.state = SidecarState::Failed;
                self.push_diagnostic_locked(&mut data, &cleanup_error.to_string());
                Err(cleanup_error)
            }
        }
    }

    async fn emergency_detach_generation(&self, active: ActiveSidecar) -> Result<(), SidecarError> {
        match active.port.kill().await {
            Ok(()) => Ok(()),
            Err(error) => {
                let cleanup_error = SidecarError::new(
                    "sidecar_cleanup_failed",
                    format!("sidecar cleanup failed: {error}"),
                );
                let mut data = self.inner.data.lock().await;
                data.explicit_shutdown = true;
                data.state = SidecarState::Stopped;
                self.push_diagnostic_locked(&mut data, &cleanup_error.to_string());
                Err(cleanup_error)
            }
        }
    }

    async fn push_diagnostic(&self, diagnostic: impl AsRef<str>) {
        let mut data = self.inner.data.lock().await;
        self.push_diagnostic_locked(&mut data, diagnostic.as_ref());
    }

    fn flush_stderr_locked(&self, data: &mut SupervisorData, generation: u64) {
        let Some(active) = data.active.as_mut() else {
            return;
        };
        if active.generation != generation || data.generation != generation {
            return;
        }
        let line = std::mem::take(&mut active.stderr);
        if !line.is_empty() {
            self.push_diagnostic_locked(data, "sidecar stderr output received");
        }
    }

    async fn fail(&self, error: &SidecarError) {
        let mut data = self.inner.data.lock().await;
        data.state = SidecarState::Failed;
        self.push_diagnostic_locked(&mut data, &error.to_string());
    }

    fn push_diagnostic_locked(&self, data: &mut SupervisorData, diagnostic: &str) {
        data.diagnostics.push(redact_diagnostic(diagnostic));
        if data.diagnostics.len() > MAX_DIAGNOSTICS {
            data.diagnostics.remove(0);
        }
    }
}

pub fn redact_diagnostic(value: &str) -> String {
    let lowercase = value.to_ascii_lowercase();
    if [
        "api_key",
        "api-key",
        "authorization",
        "bearer ",
        "password",
        "secret",
        "token",
        "sk-",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
    {
        return "<redacted>".to_owned();
    }
    value.to_owned()
}

pub struct TauriSidecarLauncher {
    app: AppHandle,
}

impl TauriSidecarLauncher {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl SidecarLauncher for TauriSidecarLauncher {
    async fn launch(&self) -> Result<Arc<dyn SidecarPort>, SidecarError> {
        // Keep Tauri's sidecar identity/path resolver, then use Tokio's split pipe and child API.
        let std_command: std::process::Command = self
            .app
            .shell()
            .sidecar(SIDECAR_PROGRAM)
            .map_err(|error| SidecarError::new("sidecar_launch_failed", error.to_string()))?
            .into();
        TokioSidecarPort::spawn(TokioCommand::from(std_command), WRITE_TIMEOUT)
            .map(|port| port as Arc<dyn SidecarPort>)
    }
}

struct WriterRequest {
    bytes: Vec<u8>,
    result: oneshot::Sender<Result<(), SidecarError>>,
}

struct WriterTask {
    sender: mpsc::Sender<WriterRequest>,
    join: tokio::task::JoinHandle<()>,
}

enum ProcessRequest {
    Terminate(oneshot::Sender<Result<(), SidecarError>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProcessExit {
    Pending,
    Reaped,
}

struct TokioSidecarPort {
    writer: AsyncMutex<Option<WriterTask>>,
    control: mpsc::Sender<ProcessRequest>,
    stdout: Arc<AsyncMutex<Option<ChildStdout>>>,
    stderr: Arc<AsyncMutex<Option<ChildStderr>>>,
    exit: watch::Receiver<ProcessExit>,
    poisoned: Arc<AtomicBool>,
    write_timeout: Duration,
}

impl TokioSidecarPort {
    fn spawn(
        mut command: TokioCommand,
        write_timeout: Duration,
    ) -> Result<Arc<Self>, SidecarError> {
        command.kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| SidecarError::new("sidecar_launch_failed", error.to_string()))?;
        let stdin = child.stdin.take().ok_or_else(|| {
            SidecarError::new(
                "sidecar_launch_failed",
                "sidecar stdin pipe was unavailable",
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SidecarError::new(
                "sidecar_launch_failed",
                "sidecar stdout pipe was unavailable",
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            SidecarError::new(
                "sidecar_launch_failed",
                "sidecar stderr pipe was unavailable",
            )
        })?;

        let (writer_sender, writer_receiver) = mpsc::channel(1);
        let writer_join = tokio::spawn(writer_task(stdin, writer_receiver));
        let (control_sender, control_receiver) = mpsc::channel(2);
        let (exit_sender, exit_receiver) = watch::channel(ProcessExit::Pending);
        tokio::spawn(process_control_task(child, control_receiver, exit_sender));

        Ok(Arc::new(Self {
            writer: AsyncMutex::new(Some(WriterTask {
                sender: writer_sender,
                join: writer_join,
            })),
            control: control_sender,
            stdout: Arc::new(AsyncMutex::new(Some(stdout))),
            stderr: Arc::new(AsyncMutex::new(Some(stderr))),
            exit: exit_receiver,
            poisoned: Arc::new(AtomicBool::new(false)),
            write_timeout,
        }))
    }

    async fn stop_writer(&self) -> Result<(), SidecarError> {
        let task = self.writer.lock().await.take();
        let Some(mut task) = task else {
            return Ok(());
        };
        task.join.abort();
        match tokio::time::timeout(CLEANUP_TIMEOUT, &mut task.join).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.is_cancelled() => Ok(()),
            Ok(Err(error)) => Err(SidecarError::new(
                "sidecar_cleanup_failed",
                error.to_string(),
            )),
            Err(_) => Err(SidecarError::new(
                "sidecar_cleanup_failed",
                "sidecar stdin task did not stop before cleanup deadline",
            )),
        }
    }

    async fn request_termination(&self) -> Result<(), SidecarError> {
        let mut exit = self.exit.clone();
        if matches!(*exit.borrow_and_update(), ProcessExit::Reaped) {
            return Ok(());
        }

        let (result_sender, result_receiver) = oneshot::channel();
        let send_error = match tokio::time::timeout(
            CLEANUP_TIMEOUT,
            self.control.send(ProcessRequest::Terminate(result_sender)),
        )
        .await
        {
            Ok(Ok(())) => None,
            Ok(Err(_)) => Some(SidecarError::new(
                "sidecar_cleanup_failed",
                "sidecar control task stopped",
            )),
            Err(_) => Some(SidecarError::new(
                "sidecar_cleanup_failed",
                "sidecar control task is busy",
            )),
        };
        if let Some(error) = send_error {
            return if matches!(*exit.borrow_and_update(), ProcessExit::Reaped) {
                Ok(())
            } else {
                Err(error)
            };
        }

        let mut exit_wait = exit.clone();
        let termination = tokio::time::timeout(CLEANUP_TIMEOUT, async move {
            tokio::select! {
                result = result_receiver => result.map_err(|_| {
                    SidecarError::new("sidecar_cleanup_failed", "sidecar control task stopped")
                })?,
                reaped = exit_wait.wait_for(|state| matches!(state, ProcessExit::Reaped)) => {
                    reaped
                        .map(|_| ())
                        .map_err(|_| SidecarError::new(
                            "sidecar_cleanup_failed",
                            "sidecar exit observer stopped",
                        ))
                }
            }
        })
        .await;
        match termination {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                if matches!(*exit.borrow_and_update(), ProcessExit::Reaped) {
                    Ok(())
                } else {
                    Err(error)
                }
            }
            Err(_) if matches!(*exit.borrow_and_update(), ProcessExit::Reaped) => Ok(()),
            Err(_) => Err(SidecarError::new(
                "sidecar_cleanup_failed",
                "sidecar did not exit before cleanup deadline",
            )),
        }
    }

    async fn poison_and_terminate(&self) -> Result<(), SidecarError> {
        self.poisoned.store(true, Ordering::Release);
        let writer_result = self.stop_writer().await;
        let process_result = self.request_termination().await;
        writer_result.and(process_result)
    }
}

async fn writer_task(mut stdin: ChildStdin, mut receiver: mpsc::Receiver<WriterRequest>) {
    while let Some(request) = receiver.recv().await {
        let result = stdin
            .write_all(&request.bytes)
            .await
            .map_err(|error| SidecarError::new("sidecar_write_failed", error.to_string()));
        let _ = request.result.send(result);
    }
}

async fn process_control_task(
    mut child: Child,
    mut receiver: mpsc::Receiver<ProcessRequest>,
    exit: watch::Sender<ProcessExit>,
) {
    loop {
        tokio::select! {
            status = child.wait() => {
                if status.is_ok() {
                    let _ = exit.send(ProcessExit::Reaped);
                    return;
                }
            }
            request = receiver.recv() => match request {
                Some(ProcessRequest::Terminate(result)) => {
                    let termination = match child.start_kill() {
                        Ok(()) => match tokio::time::timeout(CLEANUP_TIMEOUT, child.wait()).await {
                            Ok(Ok(_)) => {
                                let _ = exit.send(ProcessExit::Reaped);
                                Ok(())
                            }
                            Ok(Err(error)) => Err(SidecarError::new("sidecar_cleanup_failed", error.to_string())),
                            Err(_) => Err(SidecarError::new("sidecar_cleanup_failed", "sidecar did not exit before cleanup deadline")),
                        },
                        Err(error) => Err(SidecarError::new("sidecar_cleanup_failed", error.to_string())),
                    };
                    let completed = termination.is_ok();
                    let _ = result.send(termination);
                    if completed {
                        return;
                    }
                }
                None => {
                    let _ = child.start_kill();
                    if matches!(
                        tokio::time::timeout(CLEANUP_TIMEOUT, child.wait()).await,
                        Ok(Ok(_))
                    ) {
                        let _ = exit.send(ProcessExit::Reaped);
                    }
                    return;
                }
            }
        }
    }
}

#[async_trait]
impl SidecarPort for TokioSidecarPort {
    async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(SidecarError::new(
                "sidecar_unavailable",
                "sidecar stdin is poisoned",
            ));
        }
        let sender = self
            .writer
            .lock()
            .await
            .as_ref()
            .map(|task| task.sender.clone())
            .ok_or_else(|| {
                SidecarError::new("sidecar_unavailable", "sidecar stdin is unavailable")
            })?;
        let (result_sender, result_receiver) = oneshot::channel();
        sender
            .try_send(WriterRequest {
                bytes,
                result: result_sender,
            })
            .map_err(|_| SidecarError::new("sidecar_write_failed", "sidecar stdin is busy"))?;
        match tokio::time::timeout(self.write_timeout, result_receiver).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => {
                let cleanup = self.poison_and_terminate().await;
                cleanup?;
                Err(error)
            }
            Ok(Err(_)) => Err(SidecarError::new(
                "sidecar_write_failed",
                "sidecar stdin task stopped",
            )),
            Err(_) => match self.poison_and_terminate().await {
                Ok(()) => Err(SidecarError::new(
                    "sidecar_write_timeout",
                    "sidecar stdin write timed out",
                )),
                Err(error) => Err(error),
            },
        }
    }

    async fn kill(&self) -> Result<(), SidecarError> {
        self.poisoned.store(true, Ordering::Release);
        let writer_result = self.stop_writer().await;
        let process_result = self.request_termination().await;
        writer_result.and(process_result)
    }

    fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    fn observe(&self, supervisor: SidecarSupervisor, generation: u64) {
        let stdout = self.stdout.clone();
        let stdout_supervisor = supervisor.clone();
        tauri::async_runtime::spawn(async move {
            let Some(stdout) = stdout.lock().await.take() else {
                return;
            };
            relay_stdout(stdout, stdout_supervisor, generation).await;
        });

        let stderr = self.stderr.clone();
        let stderr_supervisor = supervisor.clone();
        tauri::async_runtime::spawn(async move {
            let Some(stderr) = stderr.lock().await.take() else {
                return;
            };
            relay_stderr(stderr, stderr_supervisor, generation).await;
        });

        let mut exit = self.exit.clone();
        let poisoned = self.poisoned.clone();
        tauri::async_runtime::spawn(async move {
            while *exit.borrow() == ProcessExit::Pending {
                if exit.changed().await.is_err() {
                    return;
                }
            }
            if poisoned.load(Ordering::Acquire) {
                supervisor.handle_poisoned_exit(generation).await;
            } else {
                supervisor.handle_unexpected_exit(generation).await;
            }
        });
    }
}

async fn relay_stdout(mut stdout: ChildStdout, supervisor: SidecarSupervisor, generation: u64) {
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match stdout.read(&mut buffer).await {
            Ok(0) => return,
            Ok(read) => {
                if let Err(error) = supervisor.accept_stdout(generation, &buffer[..read]).await {
                    supervisor.push_diagnostic(error.to_string()).await;
                }
            }
            Err(error) => {
                supervisor
                    .push_diagnostic(format!("sidecar stdout read failed: {error}"))
                    .await;
                return;
            }
        }
    }
}

async fn relay_stderr(mut stderr: ChildStderr, supervisor: SidecarSupervisor, generation: u64) {
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => return,
            Ok(read) => supervisor.accept_stderr(generation, &buffer[..read]).await,
            Err(error) => {
                supervisor
                    .push_diagnostic(format!("sidecar stderr read failed: {error}"))
                    .await;
                return;
            }
        }
    }
}

pub struct TauriEventSink {
    app: AppHandle,
}

impl TauriEventSink {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl SidecarEventSink for TauriEventSink {
    async fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
        self.app
            .emit(SIDECAR_EVENT, event)
            .map_err(|error| SidecarError::new("sidecar_event_emit_failed", error.to_string()))
    }
}

pub struct TauriStorageHealthReporter {
    app: AppHandle,
    tracker: StorageHealthTracker,
}

impl TauriStorageHealthReporter {
    pub fn new(app: AppHandle, tracker: StorageHealthTracker) -> Self {
        Self { app, tracker }
    }
}

impl StorageHealthReporter for TauriStorageHealthReporter {
    fn report(&self, health: &StorageHealth) -> Result<(), SidecarError> {
        self.tracker.set(health.clone())?;
        self.app
            .emit(STORAGE_HEALTH_EVENT, health)
            .map_err(|error| SidecarError::new("storage_health_emit_failed", error.to_string()))
    }
}

pub struct PersistenceAwareEventSink {
    workspace_id: String,
    store: Arc<dyn DurableEventStore>,
    downstream: Arc<dyn SidecarEventSink>,
    health: Arc<dyn StorageHealthReporter>,
    source_generation: AtomicU64,
    queue: Arc<AsyncMutex<DurableEventQueue>>,
    health_delivery: Arc<AsyncMutex<()>>,
    shutdown: watch::Sender<bool>,
    persistence_timeout: Duration,
    retry_delay: Duration,
}

#[derive(Clone)]
struct PendingDurableEvent {
    source_generation: u64,
    event: Envelope,
    acknowledgement: watch::Sender<PersistenceAcknowledgement>,
}

#[derive(Clone)]
struct QuarantinedDurableEvent {
    event: Envelope,
    code: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PersistenceAcknowledgement {
    Pending,
    Persisted,
    Degraded(&'static str),
    Rejected(&'static str),
}

#[derive(Default)]
struct DurableEventQueue {
    pending: VecDeque<PendingDurableEvent>,
    deferred: VecDeque<PendingDurableEvent>,
    quarantined: VecDeque<QuarantinedDurableEvent>,
    worker_running: bool,
    worker: Option<tokio::task::JoinHandle<()>>,
    shutting_down: bool,
    degraded_code: Option<&'static str>,
    degraded_recoverable: bool,
    sticky_degraded: bool,
}

enum PersistenceEnqueue {
    Await(watch::Receiver<PersistenceAcknowledgement>),
    LiveOnly,
    Collision,
}

impl PersistenceAwareEventSink {
    pub fn new(
        workspace_id: impl Into<String>,
        store: Arc<dyn DurableEventStore>,
        downstream: Arc<dyn SidecarEventSink>,
        health: Arc<dyn StorageHealthReporter>,
    ) -> Self {
        Self::with_persistence_timeout(
            workspace_id,
            store,
            downstream,
            health,
            STORAGE_PERSIST_TIMEOUT,
        )
    }

    fn with_persistence_timeout(
        workspace_id: impl Into<String>,
        store: Arc<dyn DurableEventStore>,
        downstream: Arc<dyn SidecarEventSink>,
        health: Arc<dyn StorageHealthReporter>,
        persistence_timeout: Duration,
    ) -> Self {
        Self::with_persistence_options(
            workspace_id,
            store,
            downstream,
            health,
            persistence_timeout,
            STORAGE_RETRY_DELAY,
        )
    }

    fn with_persistence_options(
        workspace_id: impl Into<String>,
        store: Arc<dyn DurableEventStore>,
        downstream: Arc<dyn SidecarEventSink>,
        health: Arc<dyn StorageHealthReporter>,
        persistence_timeout: Duration,
        retry_delay: Duration,
    ) -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            workspace_id: workspace_id.into(),
            store,
            downstream,
            health,
            source_generation: AtomicU64::new(0),
            queue: Arc::new(AsyncMutex::new(DurableEventQueue::default())),
            health_delivery: Arc::new(AsyncMutex::new(())),
            shutdown,
            persistence_timeout,
            retry_delay,
        }
    }

    fn is_persistence_candidate(event: &Envelope) -> bool {
        matches!(&event.kind, ProtocolKind::Event(EventKind::SessionState))
            && event.session_id.is_some()
            || matches!(
                &event.kind,
                ProtocolKind::Event(EventKind::SuggestionCompleted)
            )
            || matches!(
                &event.kind,
                ProtocolKind::Event(EventKind::TranscriptUpdated)
            ) && event.payload.get("is_final").and_then(Value::as_bool) == Some(true)
    }

    fn storage_failure_code(event: &Envelope) -> &'static str {
        if matches!(
            &event.kind,
            ProtocolKind::Event(EventKind::SuggestionCompleted)
        ) {
            "storage_association_or_write_failed"
        } else {
            "storage_write_failed"
        }
    }

    fn spawn_persistence_worker(&self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(run_persistence_worker(
            self.workspace_id.clone(),
            self.store.clone(),
            self.health.clone(),
            self.queue.clone(),
            self.health_delivery.clone(),
            self.persistence_timeout,
            self.retry_delay,
            self.shutdown.subscribe(),
        ))
    }
}

#[async_trait]
impl SidecarEventSink for PersistenceAwareEventSink {
    async fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
        if matches!(&event.kind, ProtocolKind::Event(EventKind::SidecarReady)) {
            self.source_generation.fetch_add(1, Ordering::SeqCst);
        } else if Self::is_persistence_candidate(event) {
            let mut degraded_report = None;
            let enqueue = {
                let mut queue = self.queue.lock().await;
                let queued = queue
                    .pending
                    .iter()
                    .chain(queue.deferred.iter())
                    .find(|item| item.event.id == event.id)
                    .map(|item| (item.event == *event, item.acknowledgement.subscribe()));
                let quarantined = queue
                    .quarantined
                    .iter()
                    .find(|item| item.event.id == event.id)
                    .map(|item| (item.event == *event, item.code));
                if let Some((is_exact, receiver)) = queued {
                    if is_exact {
                        PersistenceEnqueue::Await(receiver)
                    } else {
                        let code = "storage_event_collision";
                        let recoverable = false;
                        let should_report =
                            mark_degraded_locked(&mut queue, code, recoverable, true);
                        degraded_report = should_report.then_some((code, recoverable));
                        PersistenceEnqueue::Collision
                    }
                } else if let Some((is_exact, quarantine_code)) = quarantined {
                    if is_exact {
                        let recoverable = false;
                        let should_report =
                            mark_degraded_locked(&mut queue, quarantine_code, recoverable, true);
                        degraded_report = should_report.then_some((quarantine_code, recoverable));
                        if quarantine_code == "storage_event_collision" {
                            PersistenceEnqueue::Collision
                        } else {
                            PersistenceEnqueue::LiveOnly
                        }
                    } else {
                        let code = "storage_event_collision";
                        let recoverable = false;
                        let should_report =
                            mark_degraded_locked(&mut queue, code, recoverable, true);
                        degraded_report = should_report.then_some((code, recoverable));
                        PersistenceEnqueue::Collision
                    }
                } else if queue.shutting_down {
                    let code = "storage_shutdown";
                    let recoverable = false;
                    let should_report = mark_degraded_locked(&mut queue, code, recoverable, true);
                    degraded_report = should_report.then_some((code, recoverable));
                    PersistenceEnqueue::LiveOnly
                } else if queue.pending.len() + queue.deferred.len() >= MAX_PENDING_DURABLE_EVENTS
                    && queue.deferred.is_empty()
                {
                    let code = "storage_backlog_full";
                    let recoverable = false;
                    let should_report = mark_degraded_locked(&mut queue, code, recoverable, true);
                    degraded_report = should_report.then_some((code, recoverable));
                    PersistenceEnqueue::LiveOnly
                } else {
                    if queue.pending.len() + queue.deferred.len() >= MAX_PENDING_DURABLE_EVENTS {
                        let code = "storage_deferred_evicted";
                        let evicted = queue
                            .deferred
                            .pop_front()
                            .expect("a full recoverable backlog must contain a deferred event");
                        evicted
                            .acknowledgement
                            .send_replace(PersistenceAcknowledgement::Degraded(code));
                        quarantine_event_locked(&mut queue, evicted, code);
                        let recoverable = false;
                        let should_report =
                            mark_degraded_locked(&mut queue, code, recoverable, true);
                        degraded_report = should_report.then_some((code, recoverable));
                    }
                    let (sender, receiver) = watch::channel(PersistenceAcknowledgement::Pending);
                    queue.pending.push_back(PendingDurableEvent {
                        source_generation: self.source_generation.load(Ordering::SeqCst),
                        event: event.clone(),
                        acknowledgement: sender,
                    });
                    if !queue.worker_running {
                        queue.worker_running = true;
                        queue.worker = Some(self.spawn_persistence_worker());
                    }
                    PersistenceEnqueue::Await(receiver)
                }
            };

            if let Some((code, recoverable)) = degraded_report {
                report_degraded_if_current(
                    &self.queue,
                    &self.health_delivery,
                    self.health.as_ref(),
                    code,
                    recoverable,
                )
                .await;
            }
            match enqueue {
                PersistenceEnqueue::Await(receiver) => {
                    match await_initial_persistence(receiver, self.persistence_timeout).await {
                        Some(PersistenceAcknowledgement::Rejected(code)) => {
                            return Err(SidecarError::new(
                                code,
                                "durable event id conflicts with existing content",
                            ));
                        }
                        Some(_) => {}
                        None => {
                            report_timeout_if_event_pending(
                                &self.queue,
                                &self.health_delivery,
                                self.health.as_ref(),
                                event,
                                Self::storage_failure_code(event),
                            )
                            .await;
                        }
                    }
                }
                PersistenceEnqueue::Collision => {
                    return Err(SidecarError::new(
                        "storage_event_collision",
                        "durable event id conflicts with existing content",
                    ));
                }
                PersistenceEnqueue::LiveOnly => {}
            }
        }
        self.downstream.emit(event).await
    }

    async fn shutdown(&self) -> Result<(), SidecarError> {
        let mut worker = {
            let mut queue = self.queue.lock().await;
            queue.shutting_down = true;
            self.shutdown.send_replace(true);
            queue.worker.take()
        };
        self.store.interrupt();
        if let Some(mut worker_handle) = worker.take() {
            match tokio::time::timeout(PERSISTENCE_WORKER_SHUTDOWN_TIMEOUT, &mut worker_handle)
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    self.queue.lock().await.worker_running = false;
                    return Err(SidecarError::new(
                        "storage_worker_join_failed",
                        "durable storage worker stopped unexpectedly",
                    ));
                }
                Err(_) => {
                    self.queue.lock().await.worker = Some(worker_handle);
                    return Err(SidecarError::new(
                        "storage_shutdown_timeout",
                        "durable storage worker did not stop before the shutdown deadline",
                    ));
                }
            }
        }
        self.queue.lock().await.worker_running = false;
        self.downstream.shutdown().await
    }
}

async fn await_initial_persistence(
    mut receiver: watch::Receiver<PersistenceAcknowledgement>,
    timeout: Duration,
) -> Option<PersistenceAcknowledgement> {
    let acknowledgement = async {
        loop {
            let current = receiver.borrow().clone();
            if current != PersistenceAcknowledgement::Pending {
                return current;
            }
            if receiver.changed().await.is_err() {
                return PersistenceAcknowledgement::Degraded("storage_write_failed");
            }
        }
    };
    tokio::time::timeout(timeout, acknowledgement).await.ok()
}

fn mark_degraded_locked(
    queue: &mut DurableEventQueue,
    code: &'static str,
    recoverable: bool,
    sticky: bool,
) -> bool {
    debug_assert!(!sticky || !recoverable);
    if queue.sticky_degraded && !sticky {
        return false;
    }
    let newly_sticky = sticky && !queue.sticky_degraded;
    let should_report = queue.degraded_code != Some(code)
        || queue.degraded_recoverable != recoverable
        || newly_sticky;
    queue.degraded_code = Some(code);
    queue.degraded_recoverable = recoverable;
    queue.sticky_degraded |= sticky;
    should_report
}

async fn report_storage_degraded(
    queue: &Arc<AsyncMutex<DurableEventQueue>>,
    health_delivery: &Arc<AsyncMutex<()>>,
    health: &dyn StorageHealthReporter,
    code: &'static str,
    recoverable: bool,
    sticky: bool,
) {
    let should_report = {
        let mut queue = queue.lock().await;
        mark_degraded_locked(&mut queue, code, recoverable, sticky)
    };
    if should_report {
        report_degraded_if_current(queue, health_delivery, health, code, recoverable).await;
    }
}

async fn report_degraded_if_current(
    queue: &Arc<AsyncMutex<DurableEventQueue>>,
    health_delivery: &Arc<AsyncMutex<()>>,
    health: &dyn StorageHealthReporter,
    code: &'static str,
    recoverable: bool,
) {
    let _delivery = health_delivery.lock().await;
    let is_current = {
        let queue = queue.lock().await;
        queue.degraded_code == Some(code) && queue.degraded_recoverable == recoverable
    };
    if is_current {
        let _ = health.report(&StorageHealth::degraded(code, recoverable));
    }
}

async fn report_ready_if_current(
    queue: &Arc<AsyncMutex<DurableEventQueue>>,
    health_delivery: &Arc<AsyncMutex<()>>,
    health: &dyn StorageHealthReporter,
) {
    let _delivery = health_delivery.lock().await;
    let is_current = {
        let queue = queue.lock().await;
        queue.pending.is_empty()
            && queue.deferred.is_empty()
            && !queue.sticky_degraded
            && queue.degraded_code.is_none()
    };
    if is_current {
        let _ = health.report(&StorageHealth::ready());
    }
}

async fn report_timeout_if_event_pending(
    queue: &Arc<AsyncMutex<DurableEventQueue>>,
    health_delivery: &Arc<AsyncMutex<()>>,
    health: &dyn StorageHealthReporter,
    event: &Envelope,
    code: &'static str,
) {
    let should_report = {
        let mut queue = queue.lock().await;
        let acknowledgement = queue
            .pending
            .iter()
            .chain(queue.deferred.iter())
            .find(|item| item.event == *event)
            .filter(|item| *item.acknowledgement.borrow() == PersistenceAcknowledgement::Pending)
            .map(|item| item.acknowledgement.clone());
        if let Some(acknowledgement) = acknowledgement {
            let should_report = mark_degraded_locked(&mut queue, code, true, false);
            acknowledgement.send_replace(PersistenceAcknowledgement::Degraded(code));
            should_report
        } else {
            false
        }
    };
    if should_report {
        report_degraded_if_current(queue, health_delivery, health, code, true).await;
    }
}

fn spawn_persistence_attempt(
    store: Arc<dyn DurableEventStore>,
    workspace_id: String,
    item: PendingDurableEvent,
) -> tokio::task::JoinHandle<Result<(), DurableStoreError>> {
    tokio::task::spawn_blocking(move || {
        persist_durable_event(
            store.as_ref(),
            &workspace_id,
            item.source_generation,
            &item.event,
        )
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_persistence_worker(
    workspace_id: String,
    store: Arc<dyn DurableEventStore>,
    health: Arc<dyn StorageHealthReporter>,
    queue: Arc<AsyncMutex<DurableEventQueue>>,
    health_delivery: Arc<AsyncMutex<()>>,
    persistence_timeout: Duration,
    retry_delay: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let item = {
            let mut queue_state = queue.lock().await;
            if queue_state.shutting_down {
                queue_state.worker_running = false;
                return;
            }
            let Some(item) = queue_state.pending.front().cloned() else {
                queue_state.worker_running = false;
                return;
            };
            item
        };
        let Some(mut attempt) = spawn_persistence_attempt_if_active(
            &queue,
            store.clone(),
            workspace_id.clone(),
            item.clone(),
        )
        .await
        else {
            return;
        };
        let mut permanent_attempts = 0_u8;

        loop {
            let result = tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        let _ = attempt.await;
                        mark_worker_stopped(&queue).await;
                        return;
                    }
                    continue;
                }
                result = tokio::time::timeout(persistence_timeout, &mut attempt) => result,
            };
            match result {
                Ok(Ok(Ok(()))) => {
                    let report_ready = {
                        let mut queue_state = queue.lock().await;
                        let persisted = queue_state
                            .pending
                            .pop_front()
                            .expect("persistence worker must own the FIFO head");
                        debug_assert_eq!(persisted.event, item.event);
                        let deferred = std::mem::take(&mut queue_state.deferred);
                        queue_state.pending.extend(deferred);
                        if queue_state.pending.is_empty() && !queue_state.sticky_degraded {
                            queue_state.degraded_code = None;
                            queue_state.degraded_recoverable = false;
                            true
                        } else {
                            false
                        }
                    };
                    item.acknowledgement
                        .send_replace(PersistenceAcknowledgement::Persisted);
                    if report_ready {
                        report_ready_if_current(&queue, &health_delivery, health.as_ref()).await;
                    }
                    break;
                }
                Ok(Ok(Err(DurableStoreError::DependencyMissing))) => {
                    let code = DurableStoreError::DependencyMissing.code(&item.event);
                    item.acknowledgement
                        .send_replace(PersistenceAcknowledgement::Degraded(code));
                    defer_fifo_head(&queue, &item).await;
                    report_storage_degraded(
                        &queue,
                        &health_delivery,
                        health.as_ref(),
                        code,
                        true,
                        false,
                    )
                    .await;
                    break;
                }
                Ok(Ok(Err(error))) if error.is_permanent() => {
                    let code = error.code(&item.event);
                    permanent_attempts += 1;
                    if error == DurableStoreError::EventCollision {
                        item.acknowledgement
                            .send_replace(PersistenceAcknowledgement::Rejected(code));
                    } else {
                        item.acknowledgement
                            .send_replace(PersistenceAcknowledgement::Degraded(code));
                    }
                    report_storage_degraded(
                        &queue,
                        &health_delivery,
                        health.as_ref(),
                        code,
                        true,
                        false,
                    )
                    .await;
                    if permanent_attempts >= MAX_PERMANENT_ATTEMPTS {
                        quarantine_fifo_head(&queue, &item, code).await;
                        report_storage_degraded(
                            &queue,
                            &health_delivery,
                            health.as_ref(),
                            code,
                            false,
                            true,
                        )
                        .await;
                        break;
                    }
                    if sleep_or_shutdown(&mut shutdown, retry_delay).await {
                        mark_worker_stopped(&queue).await;
                        return;
                    }
                    let Some(retry) = spawn_persistence_attempt_if_active(
                        &queue,
                        store.clone(),
                        workspace_id.clone(),
                        item.clone(),
                    )
                    .await
                    else {
                        return;
                    };
                    attempt = retry;
                }
                Ok(Ok(Err(error))) => {
                    let code = error.code(&item.event);
                    item.acknowledgement
                        .send_replace(PersistenceAcknowledgement::Degraded(code));
                    report_storage_degraded(
                        &queue,
                        &health_delivery,
                        health.as_ref(),
                        code,
                        true,
                        false,
                    )
                    .await;
                    if sleep_or_shutdown(&mut shutdown, retry_delay).await {
                        mark_worker_stopped(&queue).await;
                        return;
                    }
                    let Some(retry) = spawn_persistence_attempt_if_active(
                        &queue,
                        store.clone(),
                        workspace_id.clone(),
                        item.clone(),
                    )
                    .await
                    else {
                        return;
                    };
                    attempt = retry;
                }
                Err(_) => {
                    let code = PersistenceAwareEventSink::storage_failure_code(&item.event);
                    item.acknowledgement
                        .send_replace(PersistenceAcknowledgement::Degraded(code));
                    report_storage_degraded(
                        &queue,
                        &health_delivery,
                        health.as_ref(),
                        code,
                        true,
                        false,
                    )
                    .await;
                    if sleep_or_shutdown(&mut shutdown, retry_delay).await {
                        let _ = attempt.await;
                        mark_worker_stopped(&queue).await;
                        return;
                    }
                }
                Ok(Err(_)) => {
                    let code = PersistenceAwareEventSink::storage_failure_code(&item.event);
                    item.acknowledgement
                        .send_replace(PersistenceAcknowledgement::Degraded(code));
                    report_storage_degraded(
                        &queue,
                        &health_delivery,
                        health.as_ref(),
                        code,
                        true,
                        false,
                    )
                    .await;
                    if sleep_or_shutdown(&mut shutdown, retry_delay).await {
                        mark_worker_stopped(&queue).await;
                        return;
                    }
                    let Some(retry) = spawn_persistence_attempt_if_active(
                        &queue,
                        store.clone(),
                        workspace_id.clone(),
                        item.clone(),
                    )
                    .await
                    else {
                        return;
                    };
                    attempt = retry;
                }
            }
        }
    }
}

async fn spawn_persistence_attempt_if_active(
    queue: &Arc<AsyncMutex<DurableEventQueue>>,
    store: Arc<dyn DurableEventStore>,
    workspace_id: String,
    item: PendingDurableEvent,
) -> Option<tokio::task::JoinHandle<Result<(), DurableStoreError>>> {
    let queue = queue.lock().await;
    if queue.shutting_down {
        return None;
    }
    Some(spawn_persistence_attempt(store, workspace_id, item))
}

async fn sleep_or_shutdown(shutdown: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
        _ = tokio::time::sleep(delay) => *shutdown.borrow(),
    }
}

async fn mark_worker_stopped(queue: &Arc<AsyncMutex<DurableEventQueue>>) {
    queue.lock().await.worker_running = false;
}

async fn defer_fifo_head(
    queue: &Arc<AsyncMutex<DurableEventQueue>>,
    expected: &PendingDurableEvent,
) {
    let mut queue = queue.lock().await;
    let item = queue
        .pending
        .pop_front()
        .expect("persistence worker must own the FIFO head");
    debug_assert_eq!(item.event, expected.event);
    queue.deferred.push_back(item);
}

async fn quarantine_fifo_head(
    queue: &Arc<AsyncMutex<DurableEventQueue>>,
    expected: &PendingDurableEvent,
    code: &'static str,
) {
    let mut queue = queue.lock().await;
    let item = queue
        .pending
        .pop_front()
        .expect("persistence worker must own the FIFO head");
    debug_assert_eq!(item.event, expected.event);
    quarantine_event_locked(&mut queue, item, code);
}

fn quarantine_event_locked(
    queue: &mut DurableEventQueue,
    item: PendingDurableEvent,
    code: &'static str,
) {
    if queue.quarantined.len() >= MAX_QUARANTINED_DURABLE_EVENTS {
        queue.quarantined.pop_front();
    }
    queue.quarantined.push_back(QuarantinedDurableEvent {
        event: item.event,
        code,
    });
}

fn persist_durable_event(
    store: &dyn DurableEventStore,
    workspace_id: &str,
    source_generation: u64,
    event: &Envelope,
) -> Result<(), DurableStoreError> {
    let ProtocolKind::Event(kind) = &event.kind else {
        return Err(DurableStoreError::InvalidEvent);
    };
    let session_id = event
        .session_id
        .as_deref()
        .ok_or(DurableStoreError::InvalidEvent)?;
    let timeline_kind = match kind {
        EventKind::SessionState => TimelineEventKind::SessionState,
        EventKind::TranscriptUpdated
            if event.payload.get("is_final").and_then(Value::as_bool) == Some(true) =>
        {
            TimelineEventKind::TranscriptFinal
        }
        EventKind::SuggestionCompleted => TimelineEventKind::SuggestionCompleted,
        _ => return Err(DurableStoreError::InvalidEvent),
    };
    let timestamp_ms =
        i64::try_from(event.timestamp_ms).map_err(|_| DurableStoreError::InvalidEvent)?;
    let request_id = matches!(kind, EventKind::SuggestionCompleted)
        .then(|| event.correlation_id.clone())
        .flatten();
    let turn_id = match kind {
        EventKind::TranscriptUpdated => event
            .payload
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        EventKind::SuggestionCompleted => {
            let request_id = request_id
                .as_deref()
                .ok_or(DurableStoreError::InvalidEvent)?;
            Some(
                store
                    .resolve_request_turn(workspace_id, session_id, request_id)?
                    .ok_or(DurableStoreError::DependencyMissing)?,
            )
        }
        _ => None,
    };
    let durable_event = NewTimelineEvent {
        workspace_id: workspace_id.to_owned(),
        session_id: session_id.to_owned(),
        event_id: event.id.clone(),
        source_generation: i64::try_from(source_generation)
            .map_err(|_| DurableStoreError::InvalidEvent)?,
        source_sequence: i64::try_from(event.sequence)
            .map_err(|_| DurableStoreError::InvalidEvent)?,
        timestamp_ms,
        kind: timeline_kind,
        correlation_id: event.correlation_id.clone(),
        request_id,
        turn_id,
        payload: Value::Object(event.payload.clone()),
    };
    if matches!(kind, EventKind::TranscriptUpdated) {
        let association = RequestTurnAssociation {
            workspace_id: workspace_id.to_owned(),
            session_id: session_id.to_owned(),
            request_id: event.id.clone(),
            turn_id: durable_event
                .turn_id
                .clone()
                .ok_or(DurableStoreError::InvalidEvent)?,
            created_at_ms: timestamp_ms,
        };
        return store
            .append_transcript_event_with_association(&durable_event, &association)
            .map(|_| ());
    }
    let terminal_status = matches!(kind, EventKind::SessionState)
        .then(|| event.payload.get("state").and_then(Value::as_str))
        .flatten()
        .and_then(|state| match state {
            "stopped" => Some(SessionStatus::Completed),
            "error" => Some(SessionStatus::Interrupted),
            _ => None,
        });
    match terminal_status {
        Some(status) => store
            .append_terminal_event(&durable_event, status, timestamp_ms)
            .map(|_| ()),
        None => store.append_event(&durable_event).map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::{json, Map, Value};
    use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex, Notify};

    use crate::protocol::{
        encode_frame, CommandKind, Envelope, EventKind, FrameDecoder, ProtocolKind,
        MAX_FRAME_BYTES, PROTOCOL_VERSION,
    };
    use crate::storage::{
        AppendEventResult, NewTimelineEvent, RequestTurnAssociation, SessionStatus,
    };

    use super::{
        packaged_sidecar_path, persist_durable_event, process_control_task, redact_diagnostic,
        DurableEventQueue, DurableEventStore, DurableStoreError, PendingDurableEvent,
        PersistenceAcknowledgement, PersistenceAwareEventSink, ProcessExit, ProcessRequest,
        SidecarError, SidecarEventSink, SidecarLauncher, SidecarPort, SidecarState,
        SidecarSupervisor, StorageHealth, StorageHealthReporter, StorageHealthStatus, TokioCommand,
        TokioSidecarPort, MAX_PENDING_DURABLE_EVENTS, SIDECAR_PROGRAM,
    };

    const SESSION_ID: &str = "018f0000-0000-7000-8000-000000000003";
    const SESSION_START_COMMAND_ID: &str = "018f0000-0000-7000-8000-000000000020";

    #[derive(Clone, Default)]
    struct FakeSidecarPort {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    #[async_trait]
    impl SidecarPort for FakeSidecarPort {
        async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
            self.writes.lock().unwrap().push(bytes);
            Ok(())
        }

        async fn kill(&self) -> Result<(), SidecarError> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct EarlyStartAckPort {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        observer: Arc<Mutex<Option<(SidecarSupervisor, u64)>>>,
        delivered: Arc<Notify>,
    }

    #[async_trait]
    impl SidecarPort for EarlyStartAckPort {
        async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
            let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
            let command = decoder.push(&bytes).unwrap().pop().unwrap();
            self.writes.lock().unwrap().push(bytes);
            if matches!(
                command.kind,
                ProtocolKind::Command(CommandKind::SessionStart)
            ) {
                let (supervisor, generation) = self.observer.lock().unwrap().clone().unwrap();
                let session_id = command.session_id.unwrap();
                let mut event = runtime_session_event(&session_id, "listening");
                event.correlation_id = Some(command.id);
                let event = encode_frame(&event).unwrap();
                let delivered = self.delivered.clone();
                let mut delivery = tokio::spawn(async move {
                    supervisor.accept_stdout(generation, &event).await.unwrap();
                    delivered.notify_waiters();
                });
                let _ = tokio::time::timeout(Duration::from_millis(25), &mut delivery).await;
            }
            Ok(())
        }

        async fn kill(&self) -> Result<(), SidecarError> {
            Ok(())
        }

        fn observe(&self, supervisor: SidecarSupervisor, generation: u64) {
            *self.observer.lock().unwrap() = Some((supervisor, generation));
        }
    }

    #[derive(Clone)]
    struct ToggleWriteFailurePort {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        fail_writes: Arc<AtomicBool>,
        poisoned: bool,
    }

    impl ToggleWriteFailurePort {
        fn new(poisoned: bool) -> Self {
            Self {
                writes: Arc::new(Mutex::new(Vec::new())),
                fail_writes: Arc::new(AtomicBool::new(false)),
                poisoned,
            }
        }

        fn set_fail_writes(&self, fail: bool) {
            self.fail_writes
                .store(fail, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl SidecarPort for ToggleWriteFailurePort {
        async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
            if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(SidecarError::new(
                    "sidecar_write_failed",
                    "sidecar command write failed",
                ));
            }
            self.writes.lock().unwrap().push(bytes);
            Ok(())
        }

        async fn kill(&self) -> Result<(), SidecarError> {
            Ok(())
        }

        fn is_poisoned(&self) -> bool {
            self.poisoned
        }
    }

    #[derive(Clone, Default)]
    struct HangingStopPort {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        kills: Arc<AtomicUsize>,
        stop_started: Arc<Notify>,
    }

    #[async_trait]
    impl SidecarPort for HangingStopPort {
        async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
            let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
            let command = decoder.push(&bytes).unwrap().pop().unwrap();
            self.writes.lock().unwrap().push(bytes);
            if matches!(
                command.kind,
                ProtocolKind::Command(CommandKind::SessionStop)
            ) {
                self.stop_started.notify_waiters();
                std::future::pending::<()>().await;
            }
            Ok(())
        }

        async fn kill(&self) -> Result<(), SidecarError> {
            self.kills.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct BlockingStopPort {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        kills: Arc<AtomicUsize>,
        stop_started: Arc<Notify>,
        stop_written: Arc<Notify>,
        release_stop: Arc<Notify>,
        killed: Arc<Notify>,
    }

    #[async_trait]
    impl SidecarPort for BlockingStopPort {
        async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
            let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
            let command = decoder.push(&bytes).unwrap().pop().unwrap();
            self.writes.lock().unwrap().push(bytes);
            if matches!(
                command.kind,
                ProtocolKind::Command(CommandKind::SessionStop)
            ) {
                self.stop_started.notify_waiters();
                self.release_stop.notified().await;
                self.stop_written.notify_waiters();
            }
            Ok(())
        }

        async fn kill(&self) -> Result<(), SidecarError> {
            self.kills.fetch_add(1, AtomicOrdering::SeqCst);
            self.killed.notify_waiters();
            Ok(())
        }
    }

    struct KillFailureDropPort {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        kills: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl Drop for KillFailureDropPort {
        fn drop(&mut self) {
            self.drops.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    #[async_trait]
    impl SidecarPort for KillFailureDropPort {
        async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
            let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
            let command = decoder.push(&bytes).unwrap().pop().unwrap();
            self.writes.lock().unwrap().push(bytes);
            if matches!(
                command.kind,
                ProtocolKind::Command(CommandKind::SessionStop)
            ) {
                return Err(SidecarError::new(
                    "sidecar_write_failed",
                    "sidecar stop write failed",
                ));
            }
            Ok(())
        }

        async fn kill(&self) -> Result<(), SidecarError> {
            self.kills.fetch_add(1, AtomicOrdering::SeqCst);
            Err(SidecarError::new(
                "sidecar_kill_failed",
                "sidecar kill failed",
            ))
        }
    }

    #[derive(Clone)]
    struct BlockingCommandPort {
        writes: Arc<Mutex<Vec<Vec<u8>>>>,
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail_writes: Arc<AtomicBool>,
    }

    impl BlockingCommandPort {
        fn new(fail_writes: bool) -> Self {
            Self {
                writes: Arc::new(Mutex::new(Vec::new())),
                started: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
                fail_writes: Arc::new(AtomicBool::new(fail_writes)),
            }
        }

        fn set_fail_writes(&self, fail: bool) {
            self.fail_writes
                .store(fail, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl SidecarPort for BlockingCommandPort {
        async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
            let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
            let command = decoder.push(&bytes).unwrap().pop().unwrap();
            self.writes.lock().unwrap().push(bytes);
            if matches!(
                command.kind,
                ProtocolKind::Command(CommandKind::HandshakeRequest)
            ) {
                return Ok(());
            }
            self.started.notify_waiters();
            self.release.notified().await;
            if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
                Err(SidecarError::new(
                    "sidecar_write_failed",
                    "sidecar command write failed",
                ))
            } else {
                Ok(())
            }
        }

        async fn kill(&self) -> Result<(), SidecarError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FakeSidecarLauncher {
        port: Arc<dyn SidecarPort>,
        launches: Arc<Mutex<usize>>,
    }

    #[derive(Default)]
    struct CollectingEventSink {
        events: Mutex<Vec<Envelope>>,
    }

    #[async_trait]
    impl SidecarEventSink for CollectingEventSink {
        async fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailNextEventSink {
        fail_next: AtomicBool,
        events: Mutex<Vec<Envelope>>,
    }

    #[async_trait]
    impl SidecarEventSink for FailNextEventSink {
        async fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
            if self
                .fail_next
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(SidecarError::new(
                    "test_sink_failed",
                    "test sink rejected one event",
                ));
            }
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct BlockingEventSink {
        block_next: AtomicBool,
        started: Notify,
        release: Notify,
        event_ids: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SidecarEventSink for BlockingEventSink {
        async fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
            if self
                .block_next
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.started.notify_waiters();
                self.release.notified().await;
            }
            self.event_ids.lock().unwrap().push(event.id.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeDurableEventStore {
        events: Mutex<Vec<NewTimelineEvent>>,
        terminal_transitions: Mutex<Vec<(SessionStatus, i64)>>,
        associations: Mutex<HashMap<String, String>>,
        permanent_failures: Mutex<HashMap<String, DurableStoreError>>,
        fail_writes: AtomicBool,
        order: Arc<Mutex<Vec<String>>>,
        writer_threads: Mutex<Vec<std::thread::ThreadId>>,
        persisted: tokio::sync::Notify,
    }

    impl DurableEventStore for FakeDurableEventStore {
        fn append_event(
            &self,
            event: &NewTimelineEvent,
        ) -> Result<AppendEventResult, DurableStoreError> {
            self.writer_threads
                .lock()
                .unwrap()
                .push(std::thread::current().id());
            self.order.lock().unwrap().push("persist".into());
            if let Some(error) = self
                .permanent_failures
                .lock()
                .unwrap()
                .get(&event.event_id)
                .copied()
            {
                return Err(error);
            }
            if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(DurableStoreError::Transient);
            }
            self.events.lock().unwrap().push(event.clone());
            self.persisted.notify_one();
            Ok(AppendEventResult::Inserted { host_sequence: 1 })
        }

        fn append_terminal_event(
            &self,
            event: &NewTimelineEvent,
            status: SessionStatus,
            completed_at_ms: i64,
        ) -> Result<AppendEventResult, DurableStoreError> {
            let result = self.append_event(event)?;
            self.terminal_transitions
                .lock()
                .unwrap()
                .push((status, completed_at_ms));
            Ok(result)
        }

        fn append_transcript_event_with_association(
            &self,
            event: &NewTimelineEvent,
            association: &RequestTurnAssociation,
        ) -> Result<AppendEventResult, DurableStoreError> {
            self.order.lock().unwrap().push("associate".into());
            let mut associations = self.associations.lock().unwrap();
            match associations.get(&association.request_id) {
                Some(turn_id) if turn_id == &association.turn_id => {}
                Some(_) => return Err(DurableStoreError::EventCollision),
                None => {
                    associations
                        .insert(association.request_id.clone(), association.turn_id.clone());
                }
            }
            drop(associations);
            self.append_event(event)
        }

        fn resolve_request_turn(
            &self,
            _workspace_id: &str,
            _session_id: &str,
            request_id: &str,
        ) -> Result<Option<String>, DurableStoreError> {
            Ok(self.associations.lock().unwrap().get(request_id).cloned())
        }
    }

    struct SlowDurableEventStore {
        delay: Duration,
    }

    impl DurableEventStore for SlowDurableEventStore {
        fn append_event(
            &self,
            _event: &NewTimelineEvent,
        ) -> Result<AppendEventResult, DurableStoreError> {
            std::thread::sleep(self.delay);
            Ok(AppendEventResult::Inserted { host_sequence: 1 })
        }

        fn append_terminal_event(
            &self,
            event: &NewTimelineEvent,
            _status: SessionStatus,
            _completed_at_ms: i64,
        ) -> Result<AppendEventResult, DurableStoreError> {
            self.append_event(event)
        }

        fn append_transcript_event_with_association(
            &self,
            event: &NewTimelineEvent,
            _association: &RequestTurnAssociation,
        ) -> Result<AppendEventResult, DurableStoreError> {
            self.append_event(event)
        }

        fn resolve_request_turn(
            &self,
            _workspace_id: &str,
            _session_id: &str,
            _request_id: &str,
        ) -> Result<Option<String>, DurableStoreError> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct BlockingDurableEventStore {
        gate: (Mutex<bool>, Condvar),
        interrupted: AtomicBool,
        events: Mutex<Vec<NewTimelineEvent>>,
        associations: Mutex<HashMap<String, String>>,
        attempts: AtomicUsize,
        active_writes: AtomicUsize,
        max_active_writes: AtomicUsize,
        started: tokio::sync::Notify,
        persisted: tokio::sync::Notify,
    }

    impl BlockingDurableEventStore {
        fn release(&self) {
            let mut released = self.gate.0.lock().unwrap();
            *released = true;
            self.gate.1.notify_all();
        }
    }

    impl DurableEventStore for BlockingDurableEventStore {
        fn append_event(
            &self,
            event: &NewTimelineEvent,
        ) -> Result<AppendEventResult, DurableStoreError> {
            self.attempts.fetch_add(1, AtomicOrdering::SeqCst);
            let active = self.active_writes.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            self.max_active_writes
                .fetch_max(active, AtomicOrdering::SeqCst);
            self.started.notify_one();

            let mut released = self.gate.0.lock().unwrap();
            while !*released && !self.interrupted.load(AtomicOrdering::SeqCst) {
                released = self.gate.1.wait(released).unwrap();
            }
            let interrupted = self.interrupted.load(AtomicOrdering::SeqCst);
            drop(released);

            if interrupted {
                self.active_writes.fetch_sub(1, AtomicOrdering::SeqCst);
                return Err(DurableStoreError::Transient);
            }

            self.events.lock().unwrap().push(event.clone());
            self.active_writes.fetch_sub(1, AtomicOrdering::SeqCst);
            self.persisted.notify_one();
            Ok(AppendEventResult::Inserted { host_sequence: 1 })
        }

        fn append_terminal_event(
            &self,
            event: &NewTimelineEvent,
            _status: SessionStatus,
            _completed_at_ms: i64,
        ) -> Result<AppendEventResult, DurableStoreError> {
            self.append_event(event)
        }

        fn append_transcript_event_with_association(
            &self,
            event: &NewTimelineEvent,
            association: &RequestTurnAssociation,
        ) -> Result<AppendEventResult, DurableStoreError> {
            let mut associations = self.associations.lock().unwrap();
            match associations.get(&association.request_id) {
                Some(turn_id) if turn_id == &association.turn_id => {}
                Some(_) => return Err(DurableStoreError::EventCollision),
                None => {
                    associations
                        .insert(association.request_id.clone(), association.turn_id.clone());
                }
            }
            drop(associations);
            self.append_event(event)
        }

        fn resolve_request_turn(
            &self,
            _workspace_id: &str,
            _session_id: &str,
            request_id: &str,
        ) -> Result<Option<String>, DurableStoreError> {
            Ok(self.associations.lock().unwrap().get(request_id).cloned())
        }

        fn interrupt(&self) {
            self.interrupted.store(true, AtomicOrdering::SeqCst);
            self.gate.1.notify_all();
        }
    }

    #[derive(Default)]
    struct FailOnceDurableEventStore {
        attempts: AtomicUsize,
        events: Mutex<Vec<NewTimelineEvent>>,
        associations: Mutex<HashMap<String, String>>,
        persisted: tokio::sync::Notify,
    }

    impl DurableEventStore for FailOnceDurableEventStore {
        fn append_event(
            &self,
            event: &NewTimelineEvent,
        ) -> Result<AppendEventResult, DurableStoreError> {
            if self.attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                return Err(DurableStoreError::Transient);
            }
            self.events.lock().unwrap().push(event.clone());
            self.persisted.notify_one();
            Ok(AppendEventResult::Inserted { host_sequence: 1 })
        }

        fn append_terminal_event(
            &self,
            event: &NewTimelineEvent,
            _status: SessionStatus,
            _completed_at_ms: i64,
        ) -> Result<AppendEventResult, DurableStoreError> {
            self.append_event(event)
        }

        fn append_transcript_event_with_association(
            &self,
            event: &NewTimelineEvent,
            association: &RequestTurnAssociation,
        ) -> Result<AppendEventResult, DurableStoreError> {
            let mut associations = self.associations.lock().unwrap();
            match associations.get(&association.request_id) {
                Some(turn_id) if turn_id == &association.turn_id => {}
                Some(_) => return Err(DurableStoreError::EventCollision),
                None => {
                    associations
                        .insert(association.request_id.clone(), association.turn_id.clone());
                }
            }
            drop(associations);
            self.append_event(event)
        }

        fn resolve_request_turn(
            &self,
            _workspace_id: &str,
            _session_id: &str,
            request_id: &str,
        ) -> Result<Option<String>, DurableStoreError> {
            Ok(self.associations.lock().unwrap().get(request_id).cloned())
        }
    }

    struct OrderedEventSink {
        order: Arc<Mutex<Vec<String>>>,
        events: Mutex<Vec<Envelope>>,
    }

    #[async_trait]
    impl SidecarEventSink for OrderedEventSink {
        async fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
            self.order.lock().unwrap().push("emit".into());
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }
    }

    struct RecordingStorageHealthReporter {
        order: Arc<Mutex<Vec<String>>>,
        reports: Mutex<Vec<StorageHealth>>,
    }

    impl StorageHealthReporter for RecordingStorageHealthReporter {
        fn report(&self, health: &StorageHealth) -> Result<(), SidecarError> {
            self.order
                .lock()
                .unwrap()
                .push(format!("health:{:?}", health.status).to_lowercase());
            self.reports.lock().unwrap().push(health.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct BlockingReadyStorageHealthReporter {
        gate: (Mutex<bool>, Condvar),
        ready_started: tokio::sync::Notify,
        reports: Mutex<Vec<StorageHealth>>,
    }

    impl BlockingReadyStorageHealthReporter {
        fn release_ready(&self) {
            let mut released = self.gate.0.lock().unwrap();
            *released = true;
            self.gate.1.notify_all();
        }
    }

    impl StorageHealthReporter for BlockingReadyStorageHealthReporter {
        fn report(&self, health: &StorageHealth) -> Result<(), SidecarError> {
            if health.status == StorageHealthStatus::Ready {
                self.ready_started.notify_one();
                let mut released = self.gate.0.lock().unwrap();
                while !*released {
                    released = self.gate.1.wait(released).unwrap();
                }
            }
            self.reports.lock().unwrap().push(health.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct ReentrantStorageHealthReporter {
        queue: Mutex<Option<Arc<AsyncMutex<DurableEventQueue>>>>,
        observed_locked_queue: AtomicBool,
        reports: Mutex<Vec<StorageHealth>>,
    }

    impl StorageHealthReporter for ReentrantStorageHealthReporter {
        fn report(&self, health: &StorageHealth) -> Result<(), SidecarError> {
            if let Some(queue) = self.queue.lock().unwrap().clone() {
                if queue.try_lock().is_err() {
                    self.observed_locked_queue
                        .store(true, AtomicOrdering::SeqCst);
                }
            }
            self.reports.lock().unwrap().push(health.clone());
            Ok(())
        }
    }

    #[async_trait]
    impl SidecarLauncher for FakeSidecarLauncher {
        async fn launch(&self) -> Result<Arc<dyn SidecarPort>, SidecarError> {
            *self.launches.lock().unwrap() += 1;
            Ok(self.port.clone())
        }
    }

    #[derive(Clone)]
    struct SequencedSidecarLauncher {
        ports: Arc<Mutex<VecDeque<Arc<dyn SidecarPort>>>>,
    }

    #[async_trait]
    impl SidecarLauncher for SequencedSidecarLauncher {
        async fn launch(&self) -> Result<Arc<dyn SidecarPort>, SidecarError> {
            self.ports.lock().unwrap().pop_front().ok_or_else(|| {
                SidecarError::new("sidecar_launch_failed", "no sidecar port is available")
            })
        }
    }

    #[derive(Clone)]
    struct ReadyControlPort {
        inner: Arc<TokioSidecarPort>,
    }

    #[async_trait]
    impl SidecarPort for ReadyControlPort {
        async fn write(&self, _bytes: Vec<u8>) -> Result<(), SidecarError> {
            Ok(())
        }

        async fn kill(&self) -> Result<(), SidecarError> {
            self.inner.kill().await
        }

        fn observe(&self, supervisor: SidecarSupervisor, generation: u64) {
            self.inner.observe(supervisor, generation);
        }
    }

    fn controlled_tokio_port(
        control: mpsc::Sender<ProcessRequest>,
        exit: watch::Receiver<ProcessExit>,
    ) -> Arc<TokioSidecarPort> {
        Arc::new(TokioSidecarPort {
            writer: AsyncMutex::new(None),
            control,
            stdout: Arc::new(AsyncMutex::new(None)),
            stderr: Arc::new(AsyncMutex::new(None)),
            exit,
            poisoned: Arc::new(AtomicBool::new(false)),
            write_timeout: Duration::from_secs(1),
        })
    }

    fn fixture_envelope() -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: "018f0000-0000-7000-8000-000000000001".into(),
            session_id: None,
            sequence: 0,
            timestamp_ms: 1,
            kind: ProtocolKind::Event(EventKind::SidecarReady),
            payload: Map::from_iter([(String::from("status"), Value::String("ready".into()))]),
            correlation_id: None,
        }
    }

    fn ready_for(correlation_id: Option<&str>) -> Envelope {
        Envelope {
            correlation_id: correlation_id.map(str::to_owned),
            ..fixture_envelope()
        }
    }

    fn last_handshake_id(port: &FakeSidecarPort) -> String {
        let writes = port.writes.lock().unwrap();
        let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
        let command = decoder.push(writes.last().unwrap()).unwrap().pop().unwrap();
        assert_eq!(
            command.kind,
            ProtocolKind::Command(CommandKind::HandshakeRequest)
        );
        command.id
    }

    fn fixture_command() -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: "018f0000-0000-7000-8000-000000000002".into(),
            session_id: Some(SESSION_ID.into()),
            sequence: 1,
            timestamp_ms: 2,
            kind: ProtocolKind::Command(CommandKind::ListeningSet),
            payload: Map::from_iter([(String::from("enabled"), Value::Bool(true))]),
            correlation_id: None,
        }
    }

    fn session_start_command(session_id: &str) -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: SESSION_START_COMMAND_ID.into(),
            session_id: Some(session_id.into()),
            sequence: 1,
            timestamp_ms: 2,
            kind: ProtocolKind::Command(CommandKind::SessionStart),
            payload: Map::from_iter([
                ("mode".into(), json!("interview")),
                ("input_language".into(), json!("auto")),
                ("response_language".into(), json!("en")),
                ("review_language".into(), json!("en")),
                ("you_source".into(), json!("mic")),
                (
                    "brief_id".into(),
                    json!("018f0000-0000-7000-8000-000000000021"),
                ),
            ]),
            correlation_id: None,
        }
    }

    fn session_stop_command(session_id: &str) -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: "018f0000-0000-7000-8000-000000000022".into(),
            session_id: Some(session_id.into()),
            sequence: 2,
            timestamp_ms: 3,
            kind: ProtocolKind::Command(CommandKind::SessionStop),
            payload: Map::new(),
            correlation_id: None,
        }
    }

    fn query_command(session_id: &str) -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: "018f0000-0000-7000-8000-000000000023".into(),
            session_id: Some(session_id.into()),
            sequence: 3,
            timestamp_ms: 4,
            kind: ProtocolKind::Command(CommandKind::QueryTrigger),
            payload: Map::from_iter([
                ("text".into(), json!("Summarize this answer")),
                ("answer_format".into(), json!("chat")),
            ]),
            correlation_id: None,
        }
    }

    fn runtime_session_event(session_id: &str, state: &str) -> Envelope {
        Envelope {
            session_id: Some(session_id.into()),
            payload: Map::from_iter([
                ("state".into(), json!(state)),
                ("mode".into(), json!("interview")),
                ("input_language".into(), json!("auto")),
                ("response_language".into(), json!("en")),
                ("review_language".into(), json!("en")),
                ("you_source".into(), json!("mic")),
                ("listening".into(), json!(state == "listening")),
                ("system_audio_enabled".into(), json!(false)),
            ]),
            correlation_id: Some(SESSION_START_COMMAND_ID.into()),
            ..session_event(EventKind::SessionState, Map::new())
        }
    }

    fn idle_session_event() -> Envelope {
        serde_json::from_str(include_str!(
            "../../../protocol/v1/fixtures/session-state-idle.json"
        ))
        .unwrap()
    }

    fn runtime_error_event(correlation_id: &str) -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: "018f0000-0000-7000-8000-000000000026".into(),
            session_id: None,
            sequence: 4,
            timestamp_ms: 4,
            kind: ProtocolKind::Event(EventKind::RuntimeError),
            payload: Map::from_iter([
                ("code".into(), json!("session_start_failed")),
                ("message".into(), json!("Runtime command failed.")),
                ("recoverable".into(), json!(true)),
                ("source".into(), json!("runtime")),
            ]),
            correlation_id: Some(correlation_id.into()),
        }
    }

    async fn active_runtime_for_capture_stop(port: Arc<dyn SidecarPort>) -> SidecarSupervisor {
        let supervisor = SidecarSupervisor::with_port(port);
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        supervisor
    }

    fn last_capture_stop(writes: &Mutex<Vec<Vec<u8>>>) -> Envelope {
        writes
            .lock()
            .unwrap()
            .iter()
            .flat_map(|bytes| FrameDecoder::new(MAX_FRAME_BYTES).push(bytes).unwrap())
            .find(|command| {
                matches!(
                    command.kind,
                    ProtocolKind::Command(CommandKind::SessionStop)
                )
            })
            .expect("capture protection stop must be written")
    }

    fn correlated_runtime_session_event(
        session_id: &str,
        state: &str,
        correlation_id: &str,
    ) -> Envelope {
        let mut event = runtime_session_event(session_id, state);
        event.correlation_id = Some(correlation_id.into());
        event
    }

    async fn unreachable_authorization() -> Result<(), SidecarError> {
        panic!("rejected commands must not invoke the authorizer")
    }

    fn session_event(kind: EventKind, payload: Map<String, Value>) -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: "018f0000-0000-7000-8000-000000000008".into(),
            session_id: Some(SESSION_ID.into()),
            sequence: 4,
            timestamp_ms: 4,
            kind: ProtocolKind::Event(kind),
            payload,
            correlation_id: None,
        }
    }

    fn audio_health_event(id: &str, sequence: u64) -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: id.into(),
            session_id: None,
            sequence,
            timestamp_ms: sequence,
            kind: ProtocolKind::Event(EventKind::AudioHealth),
            payload: Map::from_iter([
                ("source".into(), json!("mic")),
                ("status".into(), json!("ready")),
                ("message".into(), Value::Null),
            ]),
            correlation_id: None,
        }
    }

    #[test]
    fn sidecar_program_is_resolved_beside_the_packaged_host() {
        let host = std::path::Path::new(r"C:\\package\\CallerInterview.exe");

        assert_eq!(SIDECAR_PROGRAM, "callerinterview-sidecar");
        assert_eq!(
            packaged_sidecar_path(host),
            std::path::PathBuf::from(r"C:\\package\\callerinterview-sidecar.exe")
        );
    }

    #[test]
    fn decoder_accepts_split_frame_chunks() {
        let envelope = fixture_envelope();
        let bytes = encode_frame(&envelope).unwrap();
        let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);

        assert!(decoder.push(&bytes[..3]).unwrap().is_empty());
        assert_eq!(decoder.push(&bytes[3..]).unwrap(), vec![envelope]);
    }

    #[test]
    fn decoder_accepts_multiple_frames_from_one_chunk() {
        let first = fixture_envelope();
        let mut second = fixture_envelope();
        second.sequence = 2;
        let bytes = [
            encode_frame(&first).unwrap(),
            encode_frame(&second).unwrap(),
        ]
        .concat();

        assert_eq!(
            FrameDecoder::new(MAX_FRAME_BYTES).push(&bytes).unwrap(),
            vec![first, second]
        );
    }

    #[test]
    fn decoder_rejects_oversized_frames_before_buffering_payload() {
        let mut decoder = FrameDecoder::new(8);
        let error = decoder.push(&(9_u32.to_be_bytes())).unwrap_err();

        assert_eq!(error.code(), "frame_too_large");
    }

    #[test]
    fn decoder_rejects_malformed_messagepack() {
        let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
        let error = decoder.push(&[0, 0, 0, 1, 0xc1]).unwrap_err();

        assert_eq!(error.code(), "invalid_frame");
    }

    #[test]
    fn decoder_rejects_protocol_version_mismatch() {
        let mut invalid = fixture_envelope();
        invalid.version = PROTOCOL_VERSION + 1;
        let error = FrameDecoder::new(MAX_FRAME_BYTES)
            .push(&encode_frame(&invalid).unwrap())
            .unwrap_err();

        assert_eq!(error.code(), "invalid_envelope");
    }

    #[test]
    fn known_events_require_bounded_payloads_and_correct_session_semantics() {
        let invalid_session_state = Envelope {
            session_id: None,
            payload: Map::new(),
            ..session_event(EventKind::SessionState, Map::new())
        };
        assert!(crate::protocol::validate_event(&invalid_session_state).is_err());

        let invalid_transcript = session_event(
            EventKind::TranscriptUpdated,
            Map::from_iter([
                (
                    "turn_id".into(),
                    json!("018f0000-0000-7000-8000-000000000009"),
                ),
                ("text".into(), json!("x".repeat(65 * 1024))),
                ("is_final".into(), json!(true)),
                ("speech_final".into(), json!(true)),
                ("speaker_role".into(), json!("interviewee")),
                ("source".into(), json!("mic")),
                ("language".into(), json!("en")),
                ("confidence".into(), json!(0.9)),
                ("started_at_ms".into(), json!(1)),
                ("ended_at_ms".into(), json!(2)),
            ]),
        );
        assert!(crate::protocol::validate_event(&invalid_transcript).is_err());

        let invalid_suggestion = session_event(
            EventKind::SuggestionChunk,
            Map::from_iter([
                ("suggestion_id".into(), json!("not-a-uuid")),
                ("text".into(), json!("candidate answer")),
            ]),
        );
        assert!(crate::protocol::validate_event(&invalid_suggestion).is_err());

        let invalid_knowledge_state = session_event(EventKind::KnowledgeState, Map::new());
        assert!(crate::protocol::validate_event(&invalid_knowledge_state).is_err());
    }

    #[tokio::test]
    async fn supervisor_rejects_command_until_validated_ready_event() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port));

        let error = supervisor.send(fixture_command()).await.unwrap_err();

        assert_eq!(error.code(), "sidecar_not_ready");
    }

    #[tokio::test]
    async fn only_the_current_handshake_correlation_may_make_a_generation_ready() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port: port.clone(),
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor = SidecarSupervisor::with_launcher(Arc::new(launcher), Duration::ZERO);

        supervisor.start().await.unwrap();
        let first_handshake = last_handshake_id(&port);
        supervisor.accept_event(1, ready_for(None)).await.unwrap();
        supervisor
            .accept_event(1, ready_for(Some("018f0000-0000-7000-8000-000000000099")))
            .await
            .unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Starting);

        supervisor.restart().await.unwrap();
        let current_handshake = last_handshake_id(&port);
        supervisor
            .accept_event(1, ready_for(Some(&first_handshake)))
            .await
            .unwrap();
        supervisor
            .accept_event(2, ready_for(Some(&first_handshake)))
            .await
            .unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Starting);

        supervisor
            .accept_event(2, ready_for(Some(&current_handshake)))
            .await
            .unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Ready);
    }

    #[tokio::test]
    async fn duplicate_ready_is_emitted_once_per_generation() {
        let port = Arc::new(FakeSidecarPort::default());
        let sink = Arc::new(CollectingEventSink::default());
        let supervisor = SidecarSupervisor::with_launcher_and_sink(
            Arc::new(FakeSidecarLauncher {
                port: port.clone(),
                launches: Arc::new(Mutex::new(0)),
            }),
            sink.clone(),
            Duration::ZERO,
            None,
        );

        supervisor.start().await.unwrap();
        let handshake_id = last_handshake_id(&port);
        let first = ready_for(Some(&handshake_id));
        let mut duplicate = first.clone();
        duplicate.id = "018f0000-0000-7000-8000-000000000099".into();
        duplicate.sequence = 1;

        supervisor.accept_event(1, first).await.unwrap();
        supervisor.accept_event(1, duplicate).await.unwrap();

        assert_eq!(supervisor.status().await.state, SidecarState::Ready);
        assert_eq!(sink.events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn multi_frame_chunk_continues_after_first_sink_failure() {
        let port = Arc::new(FakeSidecarPort::default());
        let sink = Arc::new(FailNextEventSink::default());
        let supervisor = SidecarSupervisor::with_launcher_and_sink(
            Arc::new(FakeSidecarLauncher {
                port: port.clone(),
                launches: Arc::new(Mutex::new(0)),
            }),
            sink.clone(),
            Duration::ZERO,
            None,
        );
        supervisor.start().await.unwrap();
        supervisor
            .accept_event(1, ready_for(Some(&last_handshake_id(&port))))
            .await
            .unwrap();
        sink.events.lock().unwrap().clear();
        sink.fail_next
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let first = audio_health_event("018f0000-0000-7000-8000-000000000021", 2);
        let second = audio_health_event("018f0000-0000-7000-8000-000000000022", 3);
        let chunk = [
            encode_frame(&first).unwrap(),
            encode_frame(&second).unwrap(),
        ]
        .concat();

        let error = supervisor.accept_stdout(1, &chunk).await.unwrap_err();

        assert_eq!(error.code(), "test_sink_failed");
        assert_eq!(sink.events.lock().unwrap().as_slice(), &[second]);
    }

    #[tokio::test]
    async fn stale_chunks_cannot_poison_the_current_generation_decoder() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port: port.clone(),
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor = SidecarSupervisor::with_launcher(Arc::new(launcher), Duration::ZERO);
        supervisor.start().await.unwrap();
        let half_header = &encode_frame(&fixture_envelope()).unwrap()[..2];
        supervisor.accept_stdout(1, half_header).await.unwrap();

        supervisor.restart().await.unwrap();
        supervisor.accept_stdout(1, &[0, 0]).await.unwrap();
        supervisor
            .accept_event(1, fixture_envelope())
            .await
            .unwrap();

        assert_eq!(supervisor.status().await.state, SidecarState::Starting);
        supervisor
            .accept_stdout(
                2,
                &encode_frame(&ready_for(Some(&last_handshake_id(&port)))).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Ready);
    }

    #[tokio::test]
    async fn stale_ready_cannot_emit_or_transition_a_new_generation() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port: port.clone(),
            launches: Arc::new(Mutex::new(0)),
        };
        let sink = Arc::new(CollectingEventSink::default());
        let supervisor = SidecarSupervisor::with_launcher_and_sink(
            Arc::new(launcher),
            sink.clone(),
            Duration::ZERO,
            None,
        );

        supervisor.start().await.unwrap();
        let first_handshake = last_handshake_id(&port);
        supervisor.restart().await.unwrap();
        let current_handshake = last_handshake_id(&port);

        supervisor
            .accept_event(1, ready_for(Some(&first_handshake)))
            .await
            .unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Starting);
        assert!(sink.events.lock().unwrap().is_empty());

        supervisor
            .accept_event(2, ready_for(Some(&current_handshake)))
            .await
            .unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Ready);
        assert_eq!(sink.events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn supervisor_restarts_once_after_unexpected_exit_then_fails() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port,
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor =
            SidecarSupervisor::with_launcher(Arc::new(launcher.clone()), Duration::ZERO);

        supervisor.start().await.unwrap();
        supervisor.handle_unexpected_exit(1).await;
        assert_eq!(supervisor.status().await.state, SidecarState::Starting);
        assert_eq!(*launcher.launches.lock().unwrap(), 2);

        supervisor.handle_unexpected_exit(2).await;
        assert_eq!(supervisor.status().await.state, SidecarState::Failed);
        assert_eq!(*launcher.launches.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn explicit_restart_resets_automatic_restart_budget() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port,
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor = SidecarSupervisor::with_launcher(Arc::new(launcher), Duration::ZERO);

        supervisor.start().await.unwrap();
        supervisor.handle_unexpected_exit(1).await;
        assert_eq!(supervisor.status().await.restart_count, 1);
        supervisor.restart().await.unwrap();

        assert_eq!(supervisor.status().await.restart_count, 0);
    }

    #[tokio::test]
    async fn intentional_old_exit_does_not_suppress_a_new_child_exit() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port,
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor =
            SidecarSupervisor::with_launcher(Arc::new(launcher.clone()), Duration::ZERO);

        supervisor.start().await.unwrap();
        supervisor.restart().await.unwrap();
        supervisor.handle_unexpected_exit(1).await;
        supervisor.handle_unexpected_exit(2).await;

        assert_eq!(supervisor.status().await.restart_count, 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Starting);
        assert_eq!(*launcher.launches.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn explicit_restart_failure_enters_failed_state_with_redacted_diagnostic() {
        #[derive(Clone)]
        struct FailingLauncher;
        #[async_trait]
        impl SidecarLauncher for FailingLauncher {
            async fn launch(&self) -> Result<Arc<dyn SidecarPort>, SidecarError> {
                Err(SidecarError::new(
                    "sidecar_launch_failed",
                    "DEEPSEEK_API_KEY=secret-value",
                ))
            }
        }
        let supervisor =
            SidecarSupervisor::with_launcher(Arc::new(FailingLauncher), Duration::ZERO);

        assert!(supervisor.restart().await.is_err());
        let status = supervisor.status().await;
        assert_eq!(status.state, SidecarState::Failed);
        assert!(!status.diagnostics.join(" ").contains("secret-value"));
    }

    #[tokio::test]
    async fn handshake_write_cleanup_failure_is_failed_and_never_launches_a_replacement() {
        #[derive(Clone)]
        struct FailingWriteAndCleanupPort;
        #[async_trait]
        impl SidecarPort for FailingWriteAndCleanupPort {
            async fn write(&self, _bytes: Vec<u8>) -> Result<(), SidecarError> {
                Err(SidecarError::new(
                    "sidecar_write_failed",
                    "DEEPSEEK_API_KEY=write-secret",
                ))
            }

            async fn kill(&self) -> Result<(), SidecarError> {
                Err(SidecarError::new(
                    "sidecar_kill_failed",
                    "DEEPSEEK_API_KEY=cleanup-secret",
                ))
            }
        }

        let launcher = FakeSidecarLauncher {
            port: Arc::new(FailingWriteAndCleanupPort),
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor =
            SidecarSupervisor::with_launcher(Arc::new(launcher.clone()), Duration::ZERO);

        assert!(supervisor.start().await.is_err());
        let status = supervisor.status().await;
        assert_eq!(status.state, SidecarState::Failed);
        assert_eq!(*launcher.launches.lock().unwrap(), 1);
        assert!(status.diagnostics.join(" ").contains("<redacted>"));
        assert!(!status.diagnostics.join(" ").contains("cleanup-secret"));
    }

    #[tokio::test]
    async fn handshake_timeout_cleanup_failure_is_failed_and_never_restarts() {
        #[derive(Clone)]
        struct FailingCleanupPort;
        #[async_trait]
        impl SidecarPort for FailingCleanupPort {
            async fn write(&self, _bytes: Vec<u8>) -> Result<(), SidecarError> {
                Ok(())
            }

            async fn kill(&self) -> Result<(), SidecarError> {
                Err(SidecarError::new(
                    "sidecar_kill_failed",
                    "DEEPSEEK_API_KEY=cleanup-secret",
                ))
            }
        }

        let launcher = FakeSidecarLauncher {
            port: Arc::new(FailingCleanupPort),
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor =
            SidecarSupervisor::with_launcher(Arc::new(launcher.clone()), Duration::ZERO);

        supervisor.start().await.unwrap();
        supervisor.handle_handshake_timeout(1).await;

        let status = supervisor.status().await;
        assert_eq!(status.state, SidecarState::Failed);
        assert_eq!(*launcher.launches.lock().unwrap(), 1);
        assert!(status.diagnostics.join(" ").contains("<redacted>"));
        assert!(!status.diagnostics.join(" ").contains("cleanup-secret"));
    }

    #[tokio::test]
    async fn reaped_while_termination_send_is_blocked_allows_a_fresh_explicit_restart() {
        let (control_sender, control_receiver) = mpsc::channel(1);
        let (queued_result, _queued_result_receiver) = oneshot::channel();
        control_sender
            .try_send(ProcessRequest::Terminate(queued_result))
            .unwrap();
        let (exit_sender, exit_receiver) = watch::channel(ProcessExit::Pending);
        let reaped_port = controlled_tokio_port(control_sender, exit_receiver);
        let fresh_port = Arc::new(FakeSidecarPort::default());
        let launcher = SequencedSidecarLauncher {
            ports: Arc::new(Mutex::new(VecDeque::from([
                Arc::new(ReadyControlPort { inner: reaped_port }) as Arc<dyn SidecarPort>,
                fresh_port.clone() as Arc<dyn SidecarPort>,
            ]))),
        };
        let supervisor = SidecarSupervisor::with_launcher(Arc::new(launcher), Duration::ZERO);
        supervisor.start().await.unwrap();

        let restart = supervisor.restart();
        tokio::pin!(restart);
        std::future::poll_fn(|context| {
            assert!(matches!(restart.as_mut().poll(context), Poll::Pending));
            Poll::Ready(())
        })
        .await;

        exit_sender.send(ProcessExit::Reaped).unwrap();
        drop(control_receiver);
        restart.await.unwrap();

        let fresh_handshake = last_handshake_id(&fresh_port);
        supervisor
            .accept_event(2, ready_for(Some(&fresh_handshake)))
            .await
            .unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Ready);
    }

    #[tokio::test]
    async fn closed_control_channel_without_reaped_exit_is_cleanup_failure() {
        let (control_sender, control_receiver) = mpsc::channel(1);
        drop(control_receiver);
        let (_exit_sender, exit_receiver) = watch::channel(ProcessExit::Pending);
        let port = controlled_tokio_port(control_sender, exit_receiver);

        let error = port.kill().await.unwrap_err();

        assert_eq!(error.code(), "sidecar_cleanup_failed");
        assert_eq!(*port.exit.borrow(), ProcessExit::Pending);
    }

    #[tokio::test]
    async fn supervisor_does_not_restart_after_explicit_shutdown() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port,
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor =
            SidecarSupervisor::with_launcher(Arc::new(launcher.clone()), Duration::ZERO);

        supervisor.shutdown().await.unwrap();
        supervisor.handle_unexpected_exit(0).await;

        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
        assert_eq!(*launcher.launches.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn obsolete_handshake_timeout_cannot_restart_a_newer_launch() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port,
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor =
            SidecarSupervisor::with_launcher(Arc::new(launcher.clone()), Duration::ZERO);

        supervisor.start().await.unwrap();
        supervisor.restart().await.unwrap();
        supervisor.handle_handshake_timeout(1).await;

        assert_eq!(supervisor.status().await.state, SidecarState::Starting);
        assert_eq!(*launcher.launches.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn supervisor_rejects_event_kinds_and_invalid_command_schemas_before_writing() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();

        let mut event_as_command = fixture_command();
        event_as_command.kind = ProtocolKind::Event(EventKind::RuntimeError);
        assert_eq!(
            supervisor.send(event_as_command).await.unwrap_err().code(),
            "invalid_command_kind"
        );

        let mut malformed = fixture_command();
        malformed.payload = Map::from_iter([(String::from("enabled"), json!("yes"))]);
        assert_eq!(
            supervisor
                .send_with_authorization(malformed, unreachable_authorization())
                .await
                .unwrap_err()
                .code(),
            "invalid_command_payload"
        );

        let mut missing_session = fixture_command();
        missing_session.session_id = None;
        assert_eq!(
            supervisor.send(missing_session).await.unwrap_err().code(),
            "invalid_session_id"
        );
        assert!(port.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn correlated_start_snapshot_before_write_return_confirms_the_pending_runtime() {
        let port = Arc::new(EarlyStartAckPort::default());
        let launcher = FakeSidecarLauncher {
            port: port.clone(),
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor = SidecarSupervisor::with_launcher(Arc::new(launcher), Duration::ZERO);

        supervisor.start().await.unwrap();
        let handshake_id = {
            let writes = port.writes.lock().unwrap();
            let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
            decoder
                .push(writes.last().unwrap())
                .unwrap()
                .pop()
                .unwrap()
                .id
        };
        supervisor
            .accept_event(1, ready_for(Some(&handshake_id)))
            .await
            .unwrap();

        let delivered = port.delivered.notified();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), delivered)
            .await
            .unwrap();

        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
    }

    #[tokio::test]
    async fn terminal_delivery_wins_while_query_authorization_is_pending() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();

        let authorization_entered = Arc::new(Notify::new());
        let authorization_release = Arc::new(Notify::new());
        let entered = authorization_entered.notified();
        let send = tokio::spawn({
            let supervisor = supervisor.clone();
            let authorization_entered = authorization_entered.clone();
            let authorization_release = authorization_release.clone();
            async move {
                supervisor
                    .send_with_authorization(query_command(SESSION_ID), async move {
                        authorization_entered.notify_waiters();
                        authorization_release.notified().await;
                        Ok(())
                    })
                    .await
            }
        });
        entered.await;

        let terminal = tokio::spawn({
            let supervisor = supervisor.clone();
            async move {
                supervisor
                    .accept_event(0, runtime_session_event(SESSION_ID, "stopped"))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_millis(50), terminal)
            .await
            .expect("terminal event must not wait for durable authorization")
            .unwrap()
            .unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);

        authorization_release.notify_waiters();
        let error = send.await.unwrap().unwrap_err();
        assert_eq!(error.code(), "query_runtime_session_inactive");
        assert_eq!(port.writes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn restart_wins_while_query_authorization_is_pending() {
        let first_port = Arc::new(FakeSidecarPort::default());
        let second_port = Arc::new(FakeSidecarPort::default());
        let supervisor = SidecarSupervisor::with_launcher(
            Arc::new(SequencedSidecarLauncher {
                ports: Arc::new(Mutex::new(VecDeque::from([
                    first_port.clone() as Arc<dyn SidecarPort>,
                    second_port.clone() as Arc<dyn SidecarPort>,
                ]))),
            }),
            Duration::ZERO,
        );
        supervisor.start().await.unwrap();
        supervisor
            .accept_event(1, ready_for(Some(&last_handshake_id(&first_port))))
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(1, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();

        let authorization_entered = Arc::new(Notify::new());
        let authorization_release = Arc::new(Notify::new());
        let entered = authorization_entered.notified();
        let send = tokio::spawn({
            let supervisor = supervisor.clone();
            let authorization_entered = authorization_entered.clone();
            let authorization_release = authorization_release.clone();
            async move {
                supervisor
                    .send_with_authorization(query_command(SESSION_ID), async move {
                        authorization_entered.notify_waiters();
                        authorization_release.notified().await;
                        Ok(())
                    })
                    .await
            }
        });
        entered.await;

        let restart = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.restart().await }
        });
        tokio::time::timeout(Duration::from_millis(50), restart)
            .await
            .expect("restart must not wait for durable authorization")
            .unwrap()
            .unwrap();

        authorization_release.notify_waiters();
        let error = send.await.unwrap().unwrap_err();
        assert_eq!(error.code(), "sidecar_runtime_changed");
        assert_eq!(first_port.writes.lock().unwrap().len(), 2);
        assert_eq!(second_port.writes.lock().unwrap().len(), 1);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn restart_waits_once_an_authorized_port_write_has_begun() {
        let first_port = Arc::new(BlockingCommandPort::new(false));
        let second_port = Arc::new(FakeSidecarPort::default());
        let supervisor = SidecarSupervisor::with_launcher(
            Arc::new(SequencedSidecarLauncher {
                ports: Arc::new(Mutex::new(VecDeque::from([
                    first_port.clone() as Arc<dyn SidecarPort>,
                    second_port.clone() as Arc<dyn SidecarPort>,
                ]))),
            }),
            Duration::ZERO,
        );
        supervisor.start().await.unwrap();
        let handshake_id = {
            let writes = first_port.writes.lock().unwrap();
            let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
            decoder
                .push(writes.last().unwrap())
                .unwrap()
                .pop()
                .unwrap()
                .id
        };
        supervisor
            .accept_event(1, ready_for(Some(&handshake_id)))
            .await
            .unwrap();

        let write_started = first_port.started.notified();
        let send = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.send(session_start_command(SESSION_ID)).await }
        });
        write_started.await;
        let mut restart = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.restart().await }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut restart)
                .await
                .is_err()
        );

        first_port.release.notify_waiters();
        send.await.unwrap().unwrap();
        restart.await.unwrap().unwrap();
        assert_eq!(first_port.writes.lock().unwrap().len(), 2);
        assert_eq!(second_port.writes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn authorization_timeout_does_not_block_restart_and_releases_the_session_gate() {
        let first_port = Arc::new(FakeSidecarPort::default());
        let second_port = Arc::new(FakeSidecarPort::default());
        let supervisor = SidecarSupervisor::with_launcher_and_sink_and_authorization_timeout(
            Arc::new(SequencedSidecarLauncher {
                ports: Arc::new(Mutex::new(VecDeque::from([
                    first_port.clone() as Arc<dyn SidecarPort>,
                    second_port.clone() as Arc<dyn SidecarPort>,
                ]))),
            }),
            Arc::new(CollectingEventSink::default()),
            Duration::ZERO,
            None,
            Duration::from_millis(100),
        );
        supervisor.start().await.unwrap();
        supervisor
            .accept_event(1, ready_for(Some(&last_handshake_id(&first_port))))
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(1, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();

        let gate = Arc::new(AsyncMutex::new(()));
        let authorization_entered = Arc::new(Notify::new());
        let entered = authorization_entered.notified();
        let send = tokio::spawn({
            let supervisor = supervisor.clone();
            let gate = gate.clone();
            let authorization_entered = authorization_entered.clone();
            async move {
                supervisor
                    .send_with_authorization_and_gate(query_command(SESSION_ID), gate, async move {
                        authorization_entered.notify_waiters();
                        std::future::pending::<()>().await;
                        Ok(())
                    })
                    .await
            }
        });
        entered.await;

        tokio::time::timeout(Duration::from_millis(50), supervisor.restart())
            .await
            .expect("restart must remain available while authorization is pending")
            .unwrap();
        let error = send.await.unwrap().unwrap_err();
        assert_eq!(error.code(), "sidecar_authorization_timeout");
        let _released = tokio::time::timeout(Duration::from_millis(50), gate.lock())
            .await
            .expect("authorization timeout must release the session operation gate");
    }

    #[tokio::test]
    async fn caller_cancellation_after_write_enqueue_preserves_start_tracking_and_gate() {
        let port = BlockingCommandPort::new(false);
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        let gate = Arc::new(AsyncMutex::new(()));
        let write_started = port.started.notified();
        let caller = tokio::spawn({
            let supervisor = supervisor.clone();
            let gate = gate.clone();
            async move {
                supervisor
                    .send_with_authorization_and_gate(
                        session_start_command(SESSION_ID),
                        gate,
                        async { Ok(()) },
                    )
                    .await
            }
        });
        write_started.await;

        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        let mut acknowledgement = tokio::spawn({
            let supervisor = supervisor.clone();
            async move {
                supervisor
                    .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
                    .await
            }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut acknowledgement)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(25), gate.clone().lock_owned())
                .await
                .is_err()
        );

        port.release.notify_waiters();
        acknowledgement.await.unwrap().unwrap();
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
        let _released = tokio::time::timeout(Duration::from_millis(50), gate.lock())
            .await
            .expect("owned dispatch must release the gate after tracking the write");
    }

    #[tokio::test]
    async fn caller_cancellation_after_failed_write_rolls_back_start_tracking() {
        let port = BlockingCommandPort::new(true);
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        let gate = Arc::new(AsyncMutex::new(()));
        let write_started = port.started.notified();
        let caller = tokio::spawn({
            let supervisor = supervisor.clone();
            let gate = gate.clone();
            async move {
                supervisor
                    .send_with_authorization_and_gate(
                        session_start_command(SESSION_ID),
                        gate,
                        async { Ok(()) },
                    )
                    .await
            }
        });
        write_started.await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(
            tokio::time::timeout(Duration::from_millis(25), gate.clone().lock_owned())
                .await
                .is_err()
        );

        port.release.notify_waiters();
        let released = tokio::time::timeout(Duration::from_millis(50), gate.lock())
            .await
            .expect("failed owned dispatch must release the gate");
        drop(released);

        port.set_fail_writes(false);
        let retry_started = port.started.notified();
        let retry = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.send(session_start_command(SESSION_ID)).await }
        });
        retry_started.await;
        port.release.notify_waiters();
        retry.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn sink_io_runs_outside_supervisor_locks_and_keeps_event_order() {
        let first_port = Arc::new(FakeSidecarPort::default());
        let second_port = Arc::new(FakeSidecarPort::default());
        let sink = Arc::new(BlockingEventSink::default());
        let supervisor = SidecarSupervisor::with_launcher_and_sink(
            Arc::new(SequencedSidecarLauncher {
                ports: Arc::new(Mutex::new(VecDeque::from([
                    first_port.clone() as Arc<dyn SidecarPort>,
                    second_port.clone() as Arc<dyn SidecarPort>,
                ]))),
            }),
            sink.clone(),
            Duration::ZERO,
            None,
        );
        supervisor.start().await.unwrap();
        supervisor
            .accept_event(1, ready_for(Some(&last_handshake_id(&first_port))))
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(1, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        sink.event_ids.lock().unwrap().clear();
        sink.block_next
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let first_event = audio_health_event("018f0000-0000-7000-8000-000000000041", 5);
        let first_id = first_event.id.clone();
        let sink_started = sink.started.notified();
        let first_delivery = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.accept_event(1, first_event).await }
        });
        sink_started.await;

        let terminal_event = runtime_session_event(SESSION_ID, "stopped");
        let terminal_id = terminal_event.id.clone();
        let second_delivery = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.accept_event(1, terminal_event).await }
        });
        tokio::time::timeout(Duration::from_millis(50), async {
            while supervisor.current_runtime_session().await.is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal acceptance must not wait for earlier sink I/O");

        tokio::time::timeout(Duration::from_millis(50), supervisor.restart())
            .await
            .expect("restart must not wait for sink persistence or downstream I/O")
            .unwrap();
        assert!(sink.event_ids.lock().unwrap().is_empty());

        sink.release.notify_waiters();
        first_delivery.await.unwrap().unwrap();
        second_delivery.await.unwrap().unwrap();
        assert_eq!(
            sink.event_ids.lock().unwrap().as_slice(),
            &[first_id, terminal_id]
        );
    }

    #[tokio::test]
    async fn lifecycle_and_control_commands_are_bound_to_the_tracked_runtime() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        let other_session = "018f0000-0000-7000-8000-000000000024";

        let inactive_stop = supervisor
            .send(session_stop_command(SESSION_ID))
            .await
            .unwrap_err();
        let inactive_control = supervisor.send(fixture_command()).await.unwrap_err();
        assert_eq!(inactive_stop.code(), "runtime_session_inactive");
        assert_eq!(inactive_control.code(), "runtime_session_inactive");

        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        let writes_before_rejections = port.writes.lock().unwrap().len();
        let conflicting_same = supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap_err();
        let conflicting_other = supervisor
            .send(session_start_command(other_session))
            .await
            .unwrap_err();
        let cross_stop = supervisor
            .send(session_stop_command(other_session))
            .await
            .unwrap_err();
        let mut cross_control = fixture_command();
        cross_control.session_id = Some(other_session.into());
        let cross_control = supervisor.send(cross_control).await.unwrap_err();
        assert_eq!(conflicting_same.code(), "runtime_session_conflict");
        assert_eq!(conflicting_other.code(), "runtime_session_conflict");
        assert_eq!(cross_stop.code(), "runtime_session_mismatch");
        assert_eq!(cross_control.code(), "runtime_session_mismatch");
        assert_eq!(port.writes.lock().unwrap().len(), writes_before_rejections);

        supervisor.send(fixture_command()).await.unwrap();
        assert_eq!(
            supervisor
                .send_with_authorization(query_command(SESSION_ID), unreachable_authorization(),)
                .await
                .unwrap_err()
                .code(),
            "query_runtime_session_inactive"
        );
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        let active_conflict = supervisor
            .send(session_start_command(other_session))
            .await
            .unwrap_err();
        assert_eq!(active_conflict.code(), "runtime_session_conflict");

        for error in [
            inactive_stop,
            inactive_control,
            conflicting_same,
            conflicting_other,
            cross_stop,
            cross_control,
            active_conflict,
        ] {
            let serialized = serde_json::to_string(&error).unwrap();
            assert!(!serialized.contains(SESSION_ID));
            assert!(!serialized.contains(other_session));
        }
    }

    #[tokio::test]
    async fn correlated_runtime_error_clears_a_pending_session_start() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();

        supervisor
            .accept_event(
                0,
                runtime_error_event("018f0000-0000-7000-8000-000000000027"),
            )
            .await
            .unwrap();
        assert_eq!(
            supervisor
                .send(session_start_command(SESSION_ID))
                .await
                .unwrap_err()
                .code(),
            "runtime_session_conflict"
        );

        supervisor
            .accept_event(0, runtime_error_event(SESSION_START_COMMAND_ID))
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn query_dispatch_requires_the_current_runtime_session_and_never_writes_cross_session() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();

        assert_eq!(supervisor.current_runtime_session().await, None);
        assert_eq!(
            supervisor
                .send_with_authorization(query_command(SESSION_ID), unreachable_authorization(),)
                .await
                .unwrap_err()
                .code(),
            "query_runtime_session_inactive"
        );

        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);
        assert_eq!(
            supervisor
                .send(query_command(SESSION_ID))
                .await
                .unwrap_err()
                .code(),
            "query_runtime_session_inactive"
        );
        supervisor
            .accept_event(0, idle_session_event())
            .await
            .unwrap();
        let mut stale_same_session = runtime_session_event(SESSION_ID, "listening");
        stale_same_session.correlation_id = Some("018f0000-0000-7000-8000-000000000025".into());
        supervisor
            .accept_event(0, stale_same_session)
            .await
            .unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
        supervisor.send(query_command(SESSION_ID)).await.unwrap();

        let writes_before_mismatch = port.writes.lock().unwrap().len();
        let other_session = "018f0000-0000-7000-8000-000000000024";
        assert_eq!(
            supervisor
                .send_with_authorization(query_command(other_session), unreachable_authorization(),)
                .await
                .unwrap_err()
                .code(),
            "query_runtime_session_mismatch"
        );
        assert_eq!(port.writes.lock().unwrap().len(), writes_before_mismatch);
    }

    #[tokio::test]
    async fn terminal_session_stop_event_clears_runtime_authorization() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();

        supervisor
            .send(session_stop_command(SESSION_ID))
            .await
            .unwrap();

        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "stopped"))
            .await
            .unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);
        assert_eq!(
            supervisor
                .send(query_command(SESSION_ID))
                .await
                .unwrap_err()
                .code(),
            "query_runtime_session_inactive"
        );
    }

    #[tokio::test]
    async fn capture_protection_loss_sends_one_valid_stop_for_the_active_runtime() {
        let port = BlockingStopPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();

        let stop_started = port.stop_started.notified();
        let stop_written = port.stop_written.notified();
        let first = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;
        let second_supervisor = supervisor.clone();
        let mut second = Box::pin(second_supervisor.stop_for_capture_protection_loss());
        let waker: &Waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(
            matches!(second.as_mut().poll(&mut context), Poll::Pending),
            "the second stop must wait for the capture protection stop serialization",
        );
        port.release_stop.notify_waiters();
        stop_written.await;
        let stop_command = last_capture_stop(port.writes.as_ref());
        supervisor
            .accept_event(
                0,
                correlated_runtime_session_event(SESSION_ID, "stopped", &stop_command.id),
            )
            .await
            .unwrap();
        first.await.unwrap().unwrap();
        second.await.unwrap();

        let stop_commands = port
            .writes
            .lock()
            .unwrap()
            .iter()
            .flat_map(|bytes| FrameDecoder::new(MAX_FRAME_BYTES).push(bytes).unwrap())
            .filter(|command| {
                matches!(
                    command.kind,
                    ProtocolKind::Command(CommandKind::SessionStop)
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(stop_commands.len(), 1);
        let stop = &stop_commands[0];
        assert_eq!(stop.version, PROTOCOL_VERSION);
        assert!(uuid::Uuid::parse_str(&stop.id).is_ok());
        assert_eq!(stop.session_id.as_deref(), Some(SESSION_ID));
        assert_eq!(stop.payload, Map::new());
        assert!(stop.timestamp_ms > 0);
        assert!(crate::protocol::validate_command(stop).is_ok());
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn capture_protection_stop_without_ack_times_out_and_kills_the_sidecar() {
        let port = BlockingStopPort::default();
        let supervisor =
            active_runtime_for_capture_stop(Arc::new(port.clone()) as Arc<dyn SidecarPort>).await;
        let stop_started = port.stop_started.notified();
        let stop_written = port.stop_written.notified();
        let killed = port.killed.notified();
        let stop = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;
        port.release_stop.notify_waiters();
        stop_written.await;

        assert!(
            !stop.is_finished(),
            "writing session.stop must not count as stopping capture"
        );
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
        assert_eq!(
            stop.await.unwrap().unwrap_err().code(),
            "sidecar_stop_timeout"
        );
        killed.await;
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn capture_loss_after_unacknowledged_ordinary_stop_still_awaits_cleanup_and_kills() {
        let port = BlockingStopPort::default();
        let supervisor =
            active_runtime_for_capture_stop(Arc::new(port.clone()) as Arc<dyn SidecarPort>).await;
        let ordinary_stop_started = port.stop_started.notified();
        let ordinary_stop_written = port.stop_written.notified();
        let ordinary_stop = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.send(session_stop_command(SESSION_ID)).await }
        });
        ordinary_stop_started.await;
        port.release_stop.notify_waiters();
        ordinary_stop_written.await;
        ordinary_stop.await.unwrap().unwrap();

        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID),
            "an ordinary stop write is not a terminal runtime event"
        );

        let cleanup_stop_started = port.stop_started.notified();
        let cleanup_stop_written = port.stop_written.notified();
        let killed = port.killed.notified();
        let cleanup = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        tokio::time::timeout(Duration::from_millis(100), cleanup_stop_started)
            .await
            .expect("capture loss must send its own protected stop after an ordinary stop");
        port.release_stop.notify_waiters();
        cleanup_stop_written.await;

        assert!(
            !cleanup.is_finished(),
            "the protected stop write must still await a correlated terminal event"
        );
        assert_eq!(
            port.writes
                .lock()
                .unwrap()
                .iter()
                .flat_map(|bytes| FrameDecoder::new(MAX_FRAME_BYTES).push(bytes).unwrap())
                .filter(|command| matches!(
                    command.kind,
                    ProtocolKind::Command(CommandKind::SessionStop)
                ))
                .count(),
            2
        );
        assert_eq!(
            cleanup.await.unwrap().unwrap_err().code(),
            "sidecar_stop_timeout"
        );
        killed.await;
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn capture_loss_emergency_stops_a_retained_child_when_supervisor_is_not_ready() {
        let port = BlockingStopPort::default();
        let supervisor =
            active_runtime_for_capture_stop(Arc::new(port.clone()) as Arc<dyn SidecarPort>).await;
        {
            let mut data = supervisor.inner.data.lock().await;
            data.state = SidecarState::Failed;
        }

        assert_eq!(
            supervisor
                .stop_for_capture_protection_loss()
                .await
                .unwrap_err()
                .code(),
            "sidecar_stop_failed"
        );
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
        assert_eq!(supervisor.current_runtime_session().await, None);
        assert_eq!(
            port.writes
                .lock()
                .unwrap()
                .iter()
                .flat_map(|bytes| FrameDecoder::new(MAX_FRAME_BYTES).push(bytes).unwrap())
                .filter(|command| matches!(
                    command.kind,
                    ProtocolKind::Command(CommandKind::SessionStop)
                ))
                .count(),
            0,
            "a non-ready retained child must be killed without trusting protocol dispatch"
        );
    }

    #[tokio::test]
    async fn matching_capture_protection_stop_ack_succeeds_and_clears_tracking() {
        let port = BlockingStopPort::default();
        let supervisor =
            active_runtime_for_capture_stop(Arc::new(port.clone()) as Arc<dyn SidecarPort>).await;
        let stop_started = port.stop_started.notified();
        let stop_written = port.stop_written.notified();
        let stop = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;
        port.release_stop.notify_waiters();
        stop_written.await;

        assert!(
            !stop.is_finished(),
            "privacy stop must wait for the correlated stopped state"
        );
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
        let stop_command = last_capture_stop(port.writes.as_ref());
        supervisor
            .accept_event(
                0,
                correlated_runtime_session_event(SESSION_ID, "stopped", &stop_command.id),
            )
            .await
            .unwrap();

        stop.await.unwrap().unwrap();
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn wrong_capture_protection_stop_correlation_is_ignored() {
        let port = BlockingStopPort::default();
        let supervisor =
            active_runtime_for_capture_stop(Arc::new(port.clone()) as Arc<dyn SidecarPort>).await;
        let stop_started = port.stop_started.notified();
        let stop_written = port.stop_written.notified();
        let stop = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;
        port.release_stop.notify_waiters();
        stop_written.await;

        supervisor
            .accept_event(
                0,
                correlated_runtime_session_event(
                    SESSION_ID,
                    "stopped",
                    "018f0000-0000-7000-8000-000000000025",
                ),
            )
            .await
            .unwrap();
        assert!(
            !stop.is_finished(),
            "an unrelated stopped state must not acknowledge the privacy stop"
        );
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );

        let stop_command = last_capture_stop(port.writes.as_ref());
        supervisor
            .accept_event(
                0,
                correlated_runtime_session_event(SESSION_ID, "stopped", &stop_command.id),
            )
            .await
            .unwrap();
        stop.await.unwrap().unwrap();
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 0);
    }

    #[tokio::test]
    async fn matching_capture_protection_stop_state_error_kills_the_sidecar() {
        let port = BlockingStopPort::default();
        let supervisor =
            active_runtime_for_capture_stop(Arc::new(port.clone()) as Arc<dyn SidecarPort>).await;
        let stop_started = port.stop_started.notified();
        let stop_written = port.stop_written.notified();
        let killed = port.killed.notified();
        let stop = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;
        port.release_stop.notify_waiters();
        stop_written.await;

        let stop_command = last_capture_stop(port.writes.as_ref());
        supervisor
            .accept_event(
                0,
                correlated_runtime_session_event(SESSION_ID, "error", &stop_command.id),
            )
            .await
            .unwrap();

        assert_eq!(
            stop.await.unwrap().unwrap_err().code(),
            "sidecar_stop_failed"
        );
        killed.await;
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
    }

    #[tokio::test]
    async fn matching_capture_protection_stop_runtime_error_kills_the_sidecar() {
        let port = BlockingStopPort::default();
        let supervisor =
            active_runtime_for_capture_stop(Arc::new(port.clone()) as Arc<dyn SidecarPort>).await;
        let stop_started = port.stop_started.notified();
        let stop_written = port.stop_written.notified();
        let killed = port.killed.notified();
        let stop = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;
        port.release_stop.notify_waiters();
        stop_written.await;

        let stop_command = last_capture_stop(port.writes.as_ref());
        supervisor
            .accept_event(0, runtime_error_event(&stop_command.id))
            .await
            .unwrap();

        assert_eq!(
            stop.await.unwrap().unwrap_err().code(),
            "sidecar_stop_failed"
        );
        killed.await;
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
    }

    #[tokio::test]
    async fn caller_cancellation_does_not_cancel_capture_protection_stop_cleanup() {
        let port = BlockingStopPort::default();
        let supervisor =
            active_runtime_for_capture_stop(Arc::new(port.clone()) as Arc<dyn SidecarPort>).await;
        let stop_started = port.stop_started.notified();
        let stop_written = port.stop_written.notified();
        let killed = port.killed.notified();
        let caller = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;

        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        port.release_stop.notify_waiters();
        tokio::time::timeout(Duration::from_millis(100), stop_written)
            .await
            .expect("owned privacy cleanup must finish the stop write after caller cancellation");
        tokio::time::timeout(Duration::from_secs(2), killed)
            .await
            .expect(
                "owned privacy cleanup must kill after the one-second acknowledgement deadline",
            );

        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn capture_protection_loss_stops_a_pending_runtime_session() {
        let port = BlockingStopPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);

        let stop_started = port.stop_started.notified();
        let stop_written = port.stop_written.notified();
        let stop_task = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;
        port.release_stop.notify_waiters();
        stop_written.await;
        let stop_command = last_capture_stop(port.writes.as_ref());
        supervisor
            .accept_event(
                0,
                correlated_runtime_session_event(SESSION_ID, "stopped", &stop_command.id),
            )
            .await
            .unwrap();
        stop_task.await.unwrap().unwrap();

        let stop_commands = port
            .writes
            .lock()
            .unwrap()
            .iter()
            .flat_map(|bytes| FrameDecoder::new(MAX_FRAME_BYTES).push(bytes).unwrap())
            .filter(|command| {
                matches!(
                    command.kind,
                    ProtocolKind::Command(CommandKind::SessionStop)
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(stop_commands.len(), 1);
        let stop = &stop_commands[0];
        assert_eq!(stop.session_id.as_deref(), Some(SESSION_ID));
        assert!(crate::protocol::validate_command(stop).is_ok());

        supervisor
            .send(session_start_command(
                "018f0000-0000-7000-8000-000000000024",
            ))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn capture_protection_loss_write_failure_shuts_down_the_active_runtime() {
        let port = ToggleWriteFailurePort::new(true);
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        port.set_fail_writes(true);

        assert_eq!(
            supervisor
                .stop_for_capture_protection_loss()
                .await
                .unwrap_err()
                .code(),
            "sidecar_write_failed"
        );
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn capture_protection_loss_hanging_stop_shuts_down_the_active_runtime() {
        let port = HangingStopPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();

        let stop_started = port.stop_started.notified();
        let stop = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.stop_for_capture_protection_loss().await }
        });
        stop_started.await;

        assert_eq!(
            stop.await.unwrap().unwrap_err().code(),
            "sidecar_stop_timeout"
        );
        assert_eq!(port.kills.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn capture_protection_loss_kill_failure_detaches_the_active_runtime() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let kills = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let supervisor = SidecarSupervisor::with_port(Arc::new(KillFailureDropPort {
            writes,
            kills: kills.clone(),
            drops: drops.clone(),
        }));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();

        assert_eq!(
            supervisor
                .stop_for_capture_protection_loss()
                .await
                .unwrap_err()
                .code(),
            "sidecar_cleanup_failed"
        );
        assert_eq!(kills.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(drops.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
        assert_eq!(supervisor.current_runtime_session().await, None);

        supervisor.handle_unexpected_exit(0).await;
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
    }

    #[tokio::test]
    async fn capture_protection_loss_control_channel_close_reaps_the_owned_child() {
        #[cfg(windows)]
        let mut command = {
            let mut command = TokioCommand::new("cmd.exe");
            command.args(["/d", "/c", "ping -n 30 127.0.0.1 > nul"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = TokioCommand::new("sh");
            command.args(["-c", "sleep 30"]);
            command
        };
        command.kill_on_drop(true);
        let child = command.spawn().unwrap();
        let (control_sender, control_receiver) = mpsc::channel(1);
        let (exit_sender, mut exit_receiver) = watch::channel(ProcessExit::Pending);
        tokio::spawn(process_control_task(child, control_receiver, exit_sender));
        drop(control_sender);

        tokio::time::timeout(
            Duration::from_secs(1),
            exit_receiver.wait_for(|state| matches!(state, ProcessExit::Reaped)),
        )
        .await
        .expect("closing the control channel must reap the owned child")
        .unwrap();
    }

    #[tokio::test]
    async fn matching_session_stop_retains_pending_start_until_terminal_event() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();

        supervisor
            .send(session_stop_command(SESSION_ID))
            .await
            .unwrap();
        assert_eq!(
            supervisor
                .send(session_start_command(
                    "018f0000-0000-7000-8000-000000000024",
                ))
                .await
                .unwrap_err()
                .code(),
            "runtime_session_conflict"
        );
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "stopped"))
            .await
            .unwrap();
        supervisor
            .send(session_start_command(
                "018f0000-0000-7000-8000-000000000024",
            ))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn non_poisoned_start_write_failure_rolls_back_pending_tracking() {
        let port = ToggleWriteFailurePort::new(false);
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        port.set_fail_writes(true);

        assert_eq!(
            supervisor
                .send(session_start_command(SESSION_ID))
                .await
                .unwrap_err()
                .code(),
            "sidecar_write_failed"
        );
        assert_eq!(supervisor.status().await.state, SidecarState::Ready);

        port.set_fail_writes(false);
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        assert_eq!(
            supervisor
                .send(session_start_command(SESSION_ID))
                .await
                .unwrap_err()
                .code(),
            "runtime_session_conflict"
        );
    }

    #[tokio::test]
    async fn non_poisoned_stop_write_failure_retains_runtime_tracking() {
        let port = ToggleWriteFailurePort::new(false);
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        port.set_fail_writes(true);

        assert_eq!(
            supervisor
                .send(session_stop_command(SESSION_ID))
                .await
                .unwrap_err()
                .code(),
            "sidecar_write_failed"
        );
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );

        port.set_fail_writes(false);
        supervisor
            .send(session_stop_command(SESSION_ID))
            .await
            .unwrap();
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "stopped"))
            .await
            .unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn poisoned_start_write_failure_invalidates_the_runtime_generation() {
        let port = ToggleWriteFailurePort::new(true);
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        port.set_fail_writes(true);

        assert_eq!(
            supervisor
                .send(session_start_command(SESSION_ID))
                .await
                .unwrap_err()
                .code(),
            "sidecar_write_failed"
        );
        assert_eq!(supervisor.status().await.state, SidecarState::Failed);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn poisoned_stop_write_failure_invalidates_the_active_runtime() {
        let port = ToggleWriteFailurePort::new(true);
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        port.set_fail_writes(true);

        assert_eq!(
            supervisor
                .send(session_stop_command(SESSION_ID))
                .await
                .unwrap_err()
                .code(),
            "sidecar_write_failed"
        );
        assert_eq!(supervisor.status().await.state, SidecarState::Failed);
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn matching_terminal_state_clears_runtime_but_cross_session_state_does_not() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();

        let other_session = "018f0000-0000-7000-8000-000000000024";
        supervisor
            .accept_event(0, runtime_session_event(other_session, "listening"))
            .await
            .unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);
        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        supervisor
            .accept_event(0, runtime_session_event(other_session, "stopped"))
            .await
            .unwrap();
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );

        supervisor
            .accept_event(0, runtime_session_event(SESSION_ID, "error"))
            .await
            .unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn restart_and_unexpected_exit_discard_generation_bound_runtime_sessions() {
        let first_port = Arc::new(FakeSidecarPort::default());
        let second_port = Arc::new(FakeSidecarPort::default());
        let first_launch: Arc<dyn SidecarPort> = first_port.clone();
        let second_launch: Arc<dyn SidecarPort> = second_port.clone();
        let supervisor = SidecarSupervisor::with_launcher(
            Arc::new(SequencedSidecarLauncher {
                ports: Arc::new(Mutex::new(VecDeque::from([first_launch, second_launch]))),
            }),
            Duration::ZERO,
        );

        supervisor.start().await.unwrap();
        supervisor
            .accept_event(1, ready_for(Some(&last_handshake_id(&first_port))))
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(1, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
        supervisor.restart().await.unwrap();
        assert_eq!(supervisor.current_runtime_session().await, None);

        supervisor
            .accept_event(2, ready_for(Some(&last_handshake_id(&second_port))))
            .await
            .unwrap();
        supervisor
            .send(session_start_command(SESSION_ID))
            .await
            .unwrap();
        supervisor
            .accept_event(2, runtime_session_event(SESSION_ID, "listening"))
            .await
            .unwrap();
        assert_eq!(
            supervisor.current_runtime_session().await.as_deref(),
            Some(SESSION_ID)
        );
        supervisor.handle_unexpected_exit(2).await;
        assert_eq!(supervisor.current_runtime_session().await, None);
    }

    #[tokio::test]
    async fn stderr_redaction_is_safe_across_chunk_boundaries_and_error_serialization() {
        let port = FakeSidecarPort::default();
        let supervisor = SidecarSupervisor::with_port(Arc::new(port));
        supervisor.accept_stderr(0, b"DEEPSEEK_API_").await;
        supervisor.accept_stderr(0, b"KEY=secret-value ").await;

        let diagnostics = supervisor.status().await.diagnostics.join(" ");
        assert!(!diagnostics.contains("secret-value"));
        assert!(!serde_json::to_string(&SidecarError::new(
            "sidecar_launch_failed",
            "DEEPSEEK_API_KEY=secret-value"
        ))
        .unwrap()
        .contains("secret-value"));
    }

    #[tokio::test]
    async fn shutdown_waits_for_a_serialized_port_write_before_cleanup() {
        #[derive(Clone)]
        struct BlockingPort {
            started: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        }
        #[async_trait]
        impl SidecarPort for BlockingPort {
            async fn write(&self, _bytes: Vec<u8>) -> Result<(), SidecarError> {
                self.started.notify_waiters();
                self.release.notified().await;
                Ok(())
            }
            async fn kill(&self) -> Result<(), SidecarError> {
                Ok(())
            }
        }

        let port = BlockingPort {
            started: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        };
        let supervisor = SidecarSupervisor::with_port(Arc::new(port.clone()));
        supervisor
            .accept_event(0, fixture_envelope())
            .await
            .unwrap();
        let started = port.started.notified();
        let send = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.send(session_start_command(SESSION_ID)).await }
        });
        started.await;

        let mut shutdown = tokio::spawn({
            let supervisor = supervisor.clone();
            async move { supervisor.shutdown().await }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
                .await
                .is_err()
        );
        port.release.notify_waiters();
        send.await.unwrap().unwrap();
        shutdown.await.unwrap().unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Stopped);
    }

    #[test]
    fn diagnostics_redact_secrets_and_never_treat_stderr_as_protocol() {
        for raw in [
            "DEEPSEEK_API_KEY=secret-value Bearer api-token sk-abcdefghijklmnop",
            "ANTHROPIC_API_KEY=second-secret",
            r#"{"DEEPSEEK_API_KEY":"json-secret"}"#,
            "Authorization: Bearer header-secret",
        ] {
            let diagnostic = redact_diagnostic(raw);
            assert!(diagnostic.contains("<redacted>"), "{raw}");
            for secret in [
                "secret-value",
                "api-token",
                "sk-abcdefghijklmnop",
                "second-secret",
                "json-secret",
                "header-secret",
            ] {
                assert!(!diagnostic.contains(secret), "{raw}");
            }
        }
    }

    #[tokio::test]
    async fn stderr_status_uses_a_fixed_diagnostic_instead_of_raw_provider_output() {
        let supervisor = SidecarSupervisor::with_port(Arc::new(FakeSidecarPort::default()));

        supervisor
            .accept_stderr(
                0,
                br#"{"DEEPSEEK_API_KEY":"json-secret"}
"#,
            )
            .await;

        let diagnostics = supervisor.status().await.diagnostics.join(" ");
        assert!(diagnostics.contains("sidecar stderr output received"));
        assert!(!diagnostics.contains("json-secret"));
        assert!(!diagnostics.contains("DEEPSEEK_API_KEY"));
    }

    #[tokio::test]
    async fn blocked_write_timeout_reaps_the_owned_child_without_touching_an_unrelated_helper() {
        fn blocking_stdin_command() -> TokioCommand {
            #[cfg(windows)]
            let mut command = {
                let mut command = TokioCommand::new("cmd.exe");
                command.args(["/d", "/c", "ping -n 30 127.0.0.1 > nul"]);
                command
            };
            #[cfg(not(windows))]
            let mut command = {
                let mut command = TokioCommand::new("sh");
                command.args(["-c", "sleep 30"]);
                command
            };
            command
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            command
        }

        let port =
            TokioSidecarPort::spawn(blocking_stdin_command(), Duration::from_millis(75)).unwrap();
        let mut unrelated = blocking_stdin_command().spawn().unwrap();
        let error = port.write(vec![0_u8; 4 * 1024 * 1024]).await.unwrap_err();

        assert_eq!(
            error.code(),
            "sidecar_write_timeout",
            "the timed-out write must kill and reap its owned child"
        );
        assert_eq!(*port.exit.borrow(), super::ProcessExit::Reaped);
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.start_kill().unwrap();
        unrelated.wait().await.unwrap();
    }

    #[tokio::test]
    async fn durable_event_is_committed_before_live_delivery() {
        let async_executor_thread = std::thread::current().id();
        let order = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(FakeDurableEventStore {
            order: order.clone(),
            ..Default::default()
        });
        let downstream = Arc::new(OrderedEventSink {
            order: order.clone(),
            events: Mutex::new(Vec::new()),
        });
        let health = Arc::new(RecordingStorageHealthReporter {
            order: order.clone(),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::new(
            "018f0000-0000-7000-8000-000000000099",
            store.clone(),
            downstream,
            health,
        );
        let event = session_event(
            EventKind::TranscriptUpdated,
            Map::from_iter([
                (
                    "turn_id".into(),
                    Value::String("018f0000-0000-7000-8000-000000000201".into()),
                ),
                ("text".into(), Value::String("final answer".into())),
                ("is_final".into(), Value::Bool(true)),
            ]),
        );

        sink.emit(&event).await.unwrap();
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let ready = order
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|entry| entry == "health:ready");
                if ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("ready reporting must complete after persistence acknowledgement");

        let observed_order = order.lock().unwrap();
        assert_eq!(&observed_order[..2], &["associate", "persist"]);
        assert!(observed_order.iter().any(|entry| entry == "health:ready"));
        let persisted_at = observed_order
            .iter()
            .position(|entry| entry == "persist")
            .unwrap();
        let emitted_at = observed_order
            .iter()
            .position(|entry| entry == "emit")
            .unwrap();
        assert!(persisted_at < emitted_at);
        drop(observed_order);
        let stored = store.events.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].turn_id.as_deref(),
            Some("018f0000-0000-7000-8000-000000000201")
        );
        assert_ne!(
            store.writer_threads.lock().unwrap()[0],
            async_executor_thread,
            "repository writes must cross the blocking-task boundary"
        );
    }

    #[tokio::test]
    async fn terminal_session_states_use_the_atomic_store_transition() {
        for (state, expected_status, event_id) in [
            (
                "stopped",
                SessionStatus::Completed,
                "018f0000-0000-7000-8000-000000000031",
            ),
            (
                "error",
                SessionStatus::Interrupted,
                "018f0000-0000-7000-8000-000000000032",
            ),
        ] {
            let store = Arc::new(FakeDurableEventStore::default());
            let sink = PersistenceAwareEventSink::new(
                "workspace",
                store.clone(),
                Arc::new(CollectingEventSink::default()),
                Arc::new(RecordingStorageHealthReporter {
                    order: Arc::new(Mutex::new(Vec::new())),
                    reports: Mutex::new(Vec::new()),
                }),
            );
            let mut event = session_event(
                EventKind::SessionState,
                Map::from_iter([("state".into(), Value::String(state.into()))]),
            );
            event.id = event_id.into();

            sink.emit(&event).await.unwrap();

            assert_eq!(store.events.lock().unwrap().len(), 1);
            assert_eq!(
                store.terminal_transitions.lock().unwrap().as_slice(),
                &[(expected_status, 4)]
            );
        }

        let store = Arc::new(FakeDurableEventStore::default());
        let sink = PersistenceAwareEventSink::new(
            "workspace",
            store.clone(),
            Arc::new(CollectingEventSink::default()),
            Arc::new(RecordingStorageHealthReporter {
                order: Arc::new(Mutex::new(Vec::new())),
                reports: Mutex::new(Vec::new()),
            }),
        );
        sink.emit(&session_event(
            EventKind::SessionState,
            Map::from_iter([("state".into(), Value::String("listening".into()))]),
        ))
        .await
        .unwrap();

        assert_eq!(store.events.lock().unwrap().len(), 1);
        assert!(store.terminal_transitions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn partial_transcripts_and_suggestion_chunks_are_never_persisted() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(FakeDurableEventStore {
            order: order.clone(),
            ..Default::default()
        });
        let downstream = Arc::new(OrderedEventSink {
            order: order.clone(),
            events: Mutex::new(Vec::new()),
        });
        let health = Arc::new(RecordingStorageHealthReporter {
            order: order.clone(),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::new("workspace", store.clone(), downstream, health);
        let partial = session_event(
            EventKind::TranscriptUpdated,
            Map::from_iter([
                ("turn_id".into(), Value::String("turn".into())),
                ("text".into(), Value::String("partial".into())),
                ("is_final".into(), Value::Bool(false)),
            ]),
        );
        let chunk = session_event(
            EventKind::SuggestionChunk,
            Map::from_iter([
                ("suggestion_id".into(), Value::String("suggestion".into())),
                ("text".into(), Value::String("chunk".into())),
            ]),
        );

        sink.emit(&partial).await.unwrap();
        sink.emit(&chunk).await.unwrap();

        assert!(store.events.lock().unwrap().is_empty());
        assert_eq!(&*order.lock().unwrap(), &["emit", "emit"]);
    }

    #[tokio::test]
    async fn idle_session_snapshot_is_live_only_and_does_not_degrade_storage() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(FakeDurableEventStore {
            order: order.clone(),
            ..Default::default()
        });
        let downstream = Arc::new(OrderedEventSink {
            order: order.clone(),
            events: Mutex::new(Vec::new()),
        });
        let health = Arc::new(RecordingStorageHealthReporter {
            order: order.clone(),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::new(
            "workspace",
            store.clone(),
            downstream.clone(),
            health.clone(),
        );
        let idle: Envelope = serde_json::from_str(include_str!(
            "../../../protocol/v1/fixtures/session-state-idle.json"
        ))
        .unwrap();

        sink.emit(&idle).await.unwrap();

        assert!(store.events.lock().unwrap().is_empty());
        assert!(health.reports.lock().unwrap().is_empty());
        assert_eq!(downstream.events.lock().unwrap().as_slice(), &[idle]);
        assert_eq!(&*order.lock().unwrap(), &["emit"]);
    }

    #[tokio::test]
    async fn storage_failure_retries_the_lost_event_before_reporting_recovery() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(FakeDurableEventStore {
            order: order.clone(),
            fail_writes: AtomicBool::new(true),
            ..Default::default()
        });
        let downstream = Arc::new(OrderedEventSink {
            order: order.clone(),
            events: Mutex::new(Vec::new()),
        });
        let health = Arc::new(RecordingStorageHealthReporter {
            order: order.clone(),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::new(
            "workspace",
            store.clone(),
            downstream.clone(),
            health.clone(),
        );
        let mut event = session_event(EventKind::SessionState, Map::new());

        sink.emit(&event).await.unwrap();
        store
            .fail_writes
            .store(false, std::sync::atomic::Ordering::SeqCst);
        event.id = "018f0000-0000-7000-8000-000000000009".into();
        sink.emit(&event).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while store.events.lock().unwrap().len() < 2 {
                store.persisted.notified().await;
            }
        })
        .await
        .expect("the autonomous worker must drain both pending events");
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let ready = health.reports.lock().unwrap().last() == Some(&StorageHealth::ready());
                if ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("storage health must recover after the FIFO drains");

        assert_eq!(downstream.events.lock().unwrap().len(), 2);
        let reports = health.reports.lock().unwrap();
        assert_eq!(reports[0].status, StorageHealthStatus::Degraded);
        assert_eq!(reports[0].code, Some("storage_write_failed"));
        assert!(reports[0].recoverable);
        assert_eq!(reports.last().unwrap(), &StorageHealth::ready());
        let stored = store.events.lock().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].event_id, "018f0000-0000-7000-8000-000000000008");
        assert_eq!(stored[1].event_id, "018f0000-0000-7000-8000-000000000009");
        let observed_order = order.lock().unwrap();
        assert_eq!(
            &observed_order[..3],
            &["persist", "health:degraded", "emit"]
        );
        let persist_positions = observed_order
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| (entry == "persist").then_some(index))
            .collect::<Vec<_>>();
        let emit_positions = observed_order
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| (entry == "emit").then_some(index))
            .collect::<Vec<_>>();
        assert_eq!(persist_positions.len(), 3);
        assert_eq!(emit_positions.len(), 2);
        assert!(emit_positions[1] > persist_positions[2]);
    }

    #[tokio::test]
    async fn blocked_storage_write_has_a_bounded_acknowledgement() {
        let downstream = Arc::new(CollectingEventSink::default());
        let health = Arc::new(RecordingStorageHealthReporter {
            order: Arc::new(Mutex::new(Vec::new())),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::with_persistence_timeout(
            "workspace",
            Arc::new(SlowDurableEventStore {
                delay: Duration::from_millis(100),
            }),
            downstream.clone(),
            health.clone(),
            Duration::from_millis(10),
        );
        let event = session_event(
            EventKind::TranscriptUpdated,
            Map::from_iter([
                (
                    "turn_id".into(),
                    Value::String("018f0000-0000-7000-8000-000000000201".into()),
                ),
                ("text".into(), Value::String("final answer".into())),
                ("is_final".into(), Value::Bool(true)),
            ]),
        );

        tokio::time::timeout(Duration::from_millis(50), sink.emit(&event))
            .await
            .expect("storage acknowledgement must be bounded")
            .unwrap();

        assert_eq!(downstream.events.lock().unwrap().as_slice(), &[event]);
        let reports = health.reports.lock().unwrap();
        assert_eq!(
            reports.last().unwrap().status,
            StorageHealthStatus::Degraded
        );
    }

    #[tokio::test]
    async fn final_transcript_associates_event_id_before_auto_suggestion_persists() {
        const TRANSCRIPT_EVENT_ID: &str = "018f0000-0000-7000-8000-000000000101";
        const SUGGESTION_EVENT_ID: &str = "018f0000-0000-7000-8000-000000000102";
        const TURN_ID: &str = "018f0000-0000-7000-8000-000000000201";
        let order = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(FakeDurableEventStore {
            order: order.clone(),
            ..Default::default()
        });
        let downstream = Arc::new(OrderedEventSink {
            order: order.clone(),
            events: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::new(
            "workspace",
            store.clone(),
            downstream,
            Arc::new(RecordingStorageHealthReporter {
                order: order.clone(),
                reports: Mutex::new(Vec::new()),
            }),
        );
        let mut transcript = session_event(
            EventKind::TranscriptUpdated,
            Map::from_iter([
                ("turn_id".into(), Value::String(TURN_ID.into())),
                ("text".into(), Value::String("final answer".into())),
                ("is_final".into(), Value::Bool(true)),
            ]),
        );
        transcript.id = TRANSCRIPT_EVENT_ID.into();
        let mut suggestion = session_event(
            EventKind::SuggestionCompleted,
            Map::from_iter([
                (
                    "suggestion_id".into(),
                    Value::String("018f0000-0000-7000-8000-000000000301".into()),
                ),
                ("text".into(), Value::String("suggested answer".into())),
            ]),
        );
        suggestion.id = SUGGESTION_EVENT_ID.into();
        suggestion.correlation_id = Some(TRANSCRIPT_EVENT_ID.into());

        sink.emit(&transcript).await.unwrap();
        sink.emit(&suggestion).await.unwrap();
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let ready_count = order
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|entry| entry.as_str() == "health:ready")
                    .count();
                if ready_count >= 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("ready reporting must follow the final persistence acknowledgement");

        assert_eq!(
            store
                .associations
                .lock()
                .unwrap()
                .get(TRANSCRIPT_EVENT_ID)
                .map(String::as_str),
            Some(TURN_ID)
        );
        let stored = store.events.lock().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[1].request_id.as_deref(), Some(TRANSCRIPT_EVENT_ID));
        assert_eq!(stored[1].turn_id.as_deref(), Some(TURN_ID));
        let observed_order = order.lock().unwrap();
        assert_eq!(&observed_order[..2], &["associate", "persist"]);
        assert_eq!(
            observed_order
                .iter()
                .filter(|entry| entry.as_str() == "persist")
                .count(),
            2
        );
        assert_eq!(
            observed_order
                .iter()
                .filter(|entry| entry.as_str() == "emit")
                .count(),
            2
        );
        assert!(observed_order
            .iter()
            .any(|entry| entry.as_str() == "health:ready"));
    }

    #[test]
    fn final_transcript_association_collision_fails_before_timeline_append() {
        const EVENT_ID: &str = "018f0000-0000-7000-8000-000000000101";
        const EXPECTED_TURN_ID: &str = "018f0000-0000-7000-8000-000000000201";
        const CONFLICTING_TURN_ID: &str = "018f0000-0000-7000-8000-000000000202";
        let store = FakeDurableEventStore {
            associations: Mutex::new(HashMap::from([(
                EVENT_ID.into(),
                CONFLICTING_TURN_ID.into(),
            )])),
            ..Default::default()
        };
        let mut transcript = session_event(
            EventKind::TranscriptUpdated,
            Map::from_iter([
                ("turn_id".into(), Value::String(EXPECTED_TURN_ID.into())),
                ("text".into(), Value::String("final answer".into())),
                ("is_final".into(), Value::Bool(true)),
            ]),
        );
        transcript.id = EVENT_ID.into();

        assert!(persist_durable_event(&store, "workspace", 1, &transcript).is_err());
        assert!(store.events.lock().unwrap().is_empty());
        assert_eq!(
            store
                .associations
                .lock()
                .unwrap()
                .get(EVENT_ID)
                .map(String::as_str),
            Some(CONFLICTING_TURN_ID)
        );
    }

    #[tokio::test]
    async fn failed_head_retries_without_new_traffic() {
        let store = Arc::new(FailOnceDurableEventStore::default());
        let downstream = Arc::new(CollectingEventSink::default());
        let health = Arc::new(RecordingStorageHealthReporter {
            order: Arc::new(Mutex::new(Vec::new())),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            downstream.clone(),
            health.clone(),
            Duration::from_millis(10),
            Duration::from_millis(5),
        );
        let event = session_event(EventKind::SessionState, Map::new());

        sink.emit(&event).await.unwrap();
        tokio::time::timeout(Duration::from_millis(250), store.persisted.notified())
            .await
            .expect("the queued event must retry without another emit call");
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let ready = health.reports.lock().unwrap().last() == Some(&StorageHealth::ready());
                if ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("storage health must recover after the retry succeeds");

        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(store.events.lock().unwrap().as_slice().len(), 1);
        assert_eq!(downstream.events.lock().unwrap().as_slice(), &[event]);
        let reports = health.reports.lock().unwrap();
        assert_eq!(
            reports.first().unwrap().status,
            StorageHealthStatus::Degraded
        );
        assert_eq!(reports.last().unwrap(), &StorageHealth::ready());
    }

    #[tokio::test]
    async fn timed_out_head_keeps_one_blocking_write_during_later_emits() {
        let store = Arc::new(BlockingDurableEventStore::default());
        let downstream = Arc::new(CollectingEventSink::default());
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            downstream.clone(),
            Arc::new(RecordingStorageHealthReporter {
                order: Arc::new(Mutex::new(Vec::new())),
                reports: Mutex::new(Vec::new()),
            }),
            Duration::from_millis(5),
            Duration::from_millis(5),
        );
        let first = session_event(EventKind::SessionState, Map::new());
        let mut second = first.clone();
        second.id = "018f0000-0000-7000-8000-000000000009".into();
        second.sequence += 1;

        sink.emit(&first).await.unwrap();
        sink.emit(&second).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(store.max_active_writes.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            downstream.events.lock().unwrap().as_slice(),
            &[first, second]
        );

        store.release();
        tokio::time::timeout(Duration::from_millis(250), async {
            while store.events.lock().unwrap().len() < 2 {
                store.persisted.notified().await;
            }
        })
        .await
        .expect("both queued events must drain in FIFO order");
        let stored = store.events.lock().unwrap();
        assert_eq!(stored[0].event_id, "018f0000-0000-7000-8000-000000000008");
        assert_eq!(stored[1].event_id, "018f0000-0000-7000-8000-000000000009");
    }

    #[tokio::test]
    async fn full_backlog_degrades_storage_but_never_suppresses_live_delivery() {
        let store = Arc::new(BlockingDurableEventStore::default());
        let downstream = Arc::new(CollectingEventSink::default());
        let health = Arc::new(RecordingStorageHealthReporter {
            order: Arc::new(Mutex::new(Vec::new())),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            downstream.clone(),
            health.clone(),
            Duration::from_millis(1),
            Duration::from_millis(5),
        );

        for index in 0..=MAX_PENDING_DURABLE_EVENTS {
            let mut event = session_event(EventKind::SessionState, Map::new());
            event.id = format!("018f0000-0000-7000-8000-{index:012}");
            event.sequence = u64::try_from(index).unwrap();
            sink.emit(&event).await.unwrap();
        }

        assert_eq!(
            downstream.events.lock().unwrap().len(),
            MAX_PENDING_DURABLE_EVENTS + 1
        );
        assert!(health
            .reports
            .lock()
            .unwrap()
            .iter()
            .any(|report| report.code == Some("storage_backlog_full")));
        assert!(health
            .reports
            .lock()
            .unwrap()
            .iter()
            .any(|report| { report.code == Some("storage_backlog_full") && !report.recoverable }));
        assert_eq!(
            sink.queue.lock().await.pending.len(),
            MAX_PENDING_DURABLE_EVENTS
        );
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);
        store.release();
    }

    #[tokio::test]
    async fn full_deferred_backlog_evicts_oldest_to_admit_terminal_event() {
        const OLDEST_DEFERRED_ID: &str = "018f0000-0000-7000-8000-000000001000";
        const TERMINAL_ID: &str = "018f0000-0000-7000-8000-999999999999";
        let store = Arc::new(FakeDurableEventStore::default());
        let downstream = Arc::new(CollectingEventSink::default());
        let health = Arc::new(RecordingStorageHealthReporter {
            order: Arc::new(Mutex::new(Vec::new())),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            downstream.clone(),
            health.clone(),
            Duration::from_millis(10),
            Duration::from_millis(5),
        );
        {
            let mut queue = sink.queue.lock().await;
            for index in 0..MAX_PENDING_DURABLE_EVENTS {
                let mut event = session_event(
                    EventKind::SuggestionCompleted,
                    Map::from_iter([
                        (
                            "suggestion_id".into(),
                            Value::String(format!("suggestion-{index}")),
                        ),
                        ("text".into(), Value::String("answer".into())),
                    ]),
                );
                event.id = format!("018f0000-0000-7000-8000-{:012}", index + 1_000);
                event.correlation_id = Some(format!("missing-provider-{index}"));
                let (acknowledgement, _) = watch::channel(PersistenceAcknowledgement::Pending);
                queue.deferred.push_back(PendingDurableEvent {
                    source_generation: 0,
                    event,
                    acknowledgement,
                });
            }
        }
        let mut terminal = session_event(
            EventKind::SessionState,
            Map::from_iter([("state".into(), Value::String("stopped".into()))]),
        );
        terminal.id = TERMINAL_ID.into();

        sink.emit(&terminal).await.unwrap();
        sink.shutdown().await.unwrap();

        assert_eq!(downstream.events.lock().unwrap().as_slice(), &[terminal]);
        assert!(store
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.event_id == TERMINAL_ID));
        let queue = sink.queue.lock().await;
        assert!(queue.pending.len() + queue.deferred.len() <= MAX_PENDING_DURABLE_EVENTS);
        assert!(queue.quarantined.iter().any(|item| {
            item.event.id == OLDEST_DEFERRED_ID && item.code == "storage_deferred_evicted"
        }));
        drop(queue);
        assert!(health.reports.lock().unwrap().iter().any(|report| {
            report.code == Some("storage_deferred_evicted") && !report.recoverable
        }));
    }

    #[tokio::test]
    async fn pending_same_id_with_changed_envelope_is_rejected_before_live_delivery() {
        let store = Arc::new(BlockingDurableEventStore::default());
        let downstream = Arc::new(CollectingEventSink::default());
        let health = Arc::new(RecordingStorageHealthReporter {
            order: Arc::new(Mutex::new(Vec::new())),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            downstream.clone(),
            health.clone(),
            Duration::from_millis(5),
            Duration::from_millis(5),
        );
        let original = session_event(EventKind::SessionState, Map::new());
        let mut collision = original.clone();
        collision.sequence += 1;
        collision
            .payload
            .insert("state".into(), Value::String("changed".into()));

        sink.emit(&original).await.unwrap();
        let error = sink.emit(&collision).await.unwrap_err();

        assert_eq!(error.code(), "storage_event_collision");
        assert_eq!(
            downstream.events.lock().unwrap().as_slice(),
            std::slice::from_ref(&original)
        );
        assert!(health.reports.lock().unwrap().iter().any(|report| {
            report.code == Some("storage_event_collision") && !report.recoverable
        }));
        assert_eq!(sink.queue.lock().await.pending.len(), 1);
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);

        store.release();
        tokio::time::timeout(Duration::from_millis(250), store.persisted.notified())
            .await
            .expect("the original event must persist after the store recovers");
        {
            let stored = store.events.lock().unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(
                stored[0].source_sequence,
                i64::try_from(original.sequence).unwrap()
            );
        }
        sink.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn pending_exact_duplicate_is_idempotent() {
        let store = Arc::new(BlockingDurableEventStore::default());
        let downstream = Arc::new(CollectingEventSink::default());
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            downstream.clone(),
            Arc::new(RecordingStorageHealthReporter {
                order: Arc::new(Mutex::new(Vec::new())),
                reports: Mutex::new(Vec::new()),
            }),
            Duration::from_millis(5),
            Duration::from_millis(5),
        );
        let event = session_event(EventKind::SessionState, Map::new());

        sink.emit(&event).await.unwrap();
        sink.emit(&event).await.unwrap();
        assert_eq!(sink.queue.lock().await.pending.len(), 1);
        store.release();
        tokio::time::timeout(Duration::from_millis(250), store.persisted.notified())
            .await
            .expect("the original pending write must complete");

        assert_eq!(downstream.events.lock().unwrap().len(), 2);
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(store.events.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn missing_suggestion_dependency_is_deferred_while_transcript_and_terminal_progress() {
        const TRANSCRIPT_ID: &str = "018f0000-0000-7000-8000-000000000401";
        const SUGGESTION_ID: &str = "018f0000-0000-7000-8000-000000000402";
        const TERMINAL_ID: &str = "018f0000-0000-7000-8000-000000000403";
        const TURN_ID: &str = "018f0000-0000-7000-8000-000000000404";
        let store = Arc::new(FakeDurableEventStore::default());
        let health = Arc::new(RecordingStorageHealthReporter {
            order: Arc::new(Mutex::new(Vec::new())),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            Arc::new(CollectingEventSink::default()),
            health.clone(),
            Duration::from_millis(10),
            Duration::from_millis(5),
        );
        let mut suggestion = session_event(
            EventKind::SuggestionCompleted,
            Map::from_iter([
                (
                    "suggestion_id".into(),
                    Value::String("018f0000-0000-7000-8000-000000000405".into()),
                ),
                ("text".into(), Value::String("answer".into())),
            ]),
        );
        suggestion.id = SUGGESTION_ID.into();
        suggestion.correlation_id = Some(TRANSCRIPT_ID.into());
        let mut transcript = session_event(
            EventKind::TranscriptUpdated,
            Map::from_iter([
                ("turn_id".into(), Value::String(TURN_ID.into())),
                ("text".into(), Value::String("question".into())),
                ("is_final".into(), Value::Bool(true)),
            ]),
        );
        transcript.id = TRANSCRIPT_ID.into();
        let mut terminal = session_event(
            EventKind::SessionState,
            Map::from_iter([("state".into(), Value::String("stopped".into()))]),
        );
        terminal.id = TERMINAL_ID.into();

        sink.emit(&suggestion).await.unwrap();
        sink.emit(&transcript).await.unwrap();
        sink.emit(&terminal).await.unwrap();
        tokio::time::timeout(Duration::from_millis(250), async {
            while store.events.lock().unwrap().len() < 3 {
                store.persisted.notified().await;
            }
        })
        .await
        .expect("the deferred suggestion must retry after its transcript association exists");

        let stored = store.events.lock().unwrap();
        assert_eq!(
            stored
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            [TRANSCRIPT_ID, SUGGESTION_ID, TERMINAL_ID]
        );
        assert_eq!(
            store.terminal_transitions.lock().unwrap().as_slice(),
            &[(SessionStatus::Completed, 4)]
        );
        assert!(health.reports.lock().unwrap().iter().any(|report| {
            report.code == Some("storage_dependency_missing") && report.recoverable
        }));
    }

    #[tokio::test]
    async fn permanent_store_error_is_quarantined_before_later_event_persists() {
        const QUARANTINED_ID: &str = "018f0000-0000-7000-8000-000000000411";
        const LATER_ID: &str = "018f0000-0000-7000-8000-000000000412";
        let store = Arc::new(FakeDurableEventStore {
            permanent_failures: Mutex::new(HashMap::from([(
                QUARANTINED_ID.into(),
                DurableStoreError::InvalidSession,
            )])),
            ..Default::default()
        });
        let health = Arc::new(RecordingStorageHealthReporter {
            order: Arc::new(Mutex::new(Vec::new())),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            Arc::new(CollectingEventSink::default()),
            health.clone(),
            Duration::from_millis(10),
            Duration::from_millis(5),
        );
        let mut quarantined = session_event(EventKind::SessionState, Map::new());
        quarantined.id = QUARANTINED_ID.into();
        let mut later = session_event(EventKind::SessionState, Map::new());
        later.id = LATER_ID.into();

        sink.emit(&quarantined).await.unwrap();
        sink.emit(&later).await.unwrap();
        tokio::time::timeout(Duration::from_millis(250), async {
            while store.events.lock().unwrap().is_empty() {
                store.persisted.notified().await;
            }
        })
        .await
        .expect("a permanent head failure must not block the next durable event");

        assert_eq!(store.events.lock().unwrap()[0].event_id, LATER_ID);
        let queue = sink.queue.lock().await;
        assert_eq!(queue.quarantined.len(), 1);
        assert_eq!(queue.quarantined[0].event.id, QUARANTINED_ID);
        assert!(queue.sticky_degraded);
        assert!(health.reports.lock().unwrap().iter().any(|report| {
            report.code == Some("storage_invalid_session") && !report.recoverable
        }));
    }

    #[tokio::test]
    async fn repository_event_collision_is_rejected_before_live_delivery() {
        const COLLISION_ID: &str = "018f0000-0000-7000-8000-000000000413";
        let store = Arc::new(FakeDurableEventStore {
            permanent_failures: Mutex::new(HashMap::from([(
                COLLISION_ID.into(),
                DurableStoreError::EventCollision,
            )])),
            ..Default::default()
        });
        let downstream = Arc::new(CollectingEventSink::default());
        let health = Arc::new(RecordingStorageHealthReporter {
            order: Arc::new(Mutex::new(Vec::new())),
            reports: Mutex::new(Vec::new()),
        });
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store,
            downstream.clone(),
            health.clone(),
            Duration::from_millis(20),
            Duration::from_millis(5),
        );
        let mut event = session_event(EventKind::SessionState, Map::new());
        event.id = COLLISION_ID.into();

        let error = sink.emit(&event).await.unwrap_err();

        assert_eq!(error.code(), "storage_event_collision");
        assert!(downstream.events.lock().unwrap().is_empty());
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let quarantined = sink.queue.lock().await.quarantined.len() == 1;
                let reported = health.reports.lock().unwrap().iter().any(|report| {
                    report.code == Some("storage_event_collision") && !report.recoverable
                });
                if quarantined && reported {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("a repository collision must be quarantined after bounded retries");
        assert!(health.reports.lock().unwrap().iter().any(|report| {
            report.code == Some("storage_event_collision") && !report.recoverable
        }));
        let repeated_error = sink.emit(&event).await.unwrap_err();
        assert_eq!(repeated_error.code(), "storage_event_collision");
        assert!(downstream.events.lock().unwrap().is_empty());
        sink.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn normally_queued_event_waits_for_persistence_after_sticky_degradation() {
        let store = Arc::new(BlockingDurableEventStore::default());
        let downstream = Arc::new(CollectingEventSink::default());
        let sink = Arc::new(PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            downstream.clone(),
            Arc::new(RecordingStorageHealthReporter {
                order: Arc::new(Mutex::new(Vec::new())),
                reports: Mutex::new(Vec::new()),
            }),
            Duration::from_millis(100),
            Duration::from_millis(5),
        ));
        {
            let mut queue = sink.queue.lock().await;
            queue.sticky_degraded = true;
            queue.degraded_code = Some("storage_backlog_full");
        }
        let event = session_event(EventKind::SessionState, Map::new());
        let emit = tokio::spawn({
            let sink = sink.clone();
            let event = event.clone();
            async move { sink.emit(&event).await }
        });
        tokio::time::timeout(Duration::from_millis(50), store.started.notified())
            .await
            .expect("the durable write must start");
        tokio::task::yield_now().await;

        assert!(downstream.events.lock().unwrap().is_empty());
        assert!(!emit.is_finished());

        store.release();
        emit.await.unwrap().unwrap();
        assert_eq!(downstream.events.lock().unwrap().as_slice(), &[event]);
        sink.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn blocked_persistence_worker_shutdown_is_bounded_and_starts_no_retry() {
        let store = Arc::new(BlockingDurableEventStore::default());
        let sink = Arc::new(PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            Arc::new(CollectingEventSink::default()),
            Arc::new(RecordingStorageHealthReporter {
                order: Arc::new(Mutex::new(Vec::new())),
                reports: Mutex::new(Vec::new()),
            }),
            Duration::from_millis(5),
            Duration::from_millis(5),
        ));
        sink.emit(&session_event(EventKind::SessionState, Map::new()))
            .await
            .unwrap();
        let supervisor = SidecarSupervisor::with_launcher_and_sink(
            Arc::new(FakeSidecarLauncher {
                port: Arc::new(FakeSidecarPort::default()),
                launches: Arc::new(Mutex::new(0)),
            }),
            sink,
            Duration::ZERO,
            None,
        );

        tokio::time::timeout(Duration::from_millis(50), supervisor.shutdown())
            .await
            .expect("supervisor shutdown must cancel persistence retries")
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(store.active_writes.load(AtomicOrdering::SeqCst), 0);
        assert!(store.events.lock().unwrap().is_empty());

        store.release();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);
        assert!(store.events.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn uncooperative_persistence_attempt_returns_stable_shutdown_timeout() {
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            Arc::new(SlowDurableEventStore {
                delay: Duration::from_millis(250),
            }),
            Arc::new(CollectingEventSink::default()),
            Arc::new(RecordingStorageHealthReporter {
                order: Arc::new(Mutex::new(Vec::new())),
                reports: Mutex::new(Vec::new()),
            }),
            Duration::from_millis(5),
            Duration::from_millis(5),
        );
        sink.emit(&session_event(EventKind::SessionState, Map::new()))
            .await
            .unwrap();

        let error = sink.shutdown().await.unwrap_err();

        assert_eq!(error.code(), "storage_shutdown_timeout");
        assert!(sink.queue.lock().await.worker.is_some());
        tokio::time::sleep(Duration::from_millis(175)).await;
        sink.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn timeout_health_reporter_never_observes_the_queue_locked() {
        let queue = Arc::new(AsyncMutex::new(DurableEventQueue::default()));
        let health_delivery = Arc::new(AsyncMutex::new(()));
        let health = Arc::new(ReentrantStorageHealthReporter::default());
        *health.queue.lock().unwrap() = Some(queue.clone());
        let event = session_event(EventKind::SessionState, Map::new());
        let (acknowledgement, _) = watch::channel(PersistenceAcknowledgement::Pending);
        queue.lock().await.pending.push_back(PendingDurableEvent {
            source_generation: 0,
            event: event.clone(),
            acknowledgement,
        });

        super::report_timeout_if_event_pending(
            &queue,
            &health_delivery,
            health.as_ref(),
            &event,
            "storage_write_failed",
        )
        .await;

        assert!(!health.observed_locked_queue.load(AtomicOrdering::SeqCst));
        assert_eq!(health.reports.lock().unwrap().len(), 1);
        assert!(health.reports.lock().unwrap()[0].recoverable);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delayed_ready_reporting_cannot_create_false_timeout_degradation() {
        let store = Arc::new(FakeDurableEventStore::default());
        let downstream = Arc::new(CollectingEventSink::default());
        let health = Arc::new(BlockingReadyStorageHealthReporter::default());
        let sink = PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store,
            downstream.clone(),
            health.clone(),
            Duration::from_millis(20),
            Duration::from_millis(5),
        );
        let event = session_event(EventKind::SessionState, Map::new());

        tokio::time::timeout(Duration::from_millis(50), sink.emit(&event))
            .await
            .expect("the persistence acknowledgement must precede ready reporting")
            .unwrap();
        tokio::time::timeout(Duration::from_millis(50), health.ready_started.notified())
            .await
            .expect("ready reporting must begin");

        assert_eq!(downstream.events.lock().unwrap().as_slice(), &[event]);
        assert!(!health
            .reports
            .lock()
            .unwrap()
            .iter()
            .any(|report| report.status == StorageHealthStatus::Degraded));
        health.release_ready();
        sink.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_first_emits_start_exactly_one_persistence_worker() {
        let store = Arc::new(BlockingDurableEventStore::default());
        let sink = Arc::new(PersistenceAwareEventSink::with_persistence_options(
            "workspace",
            store.clone(),
            Arc::new(CollectingEventSink::default()),
            Arc::new(RecordingStorageHealthReporter {
                order: Arc::new(Mutex::new(Vec::new())),
                reports: Mutex::new(Vec::new()),
            }),
            Duration::from_millis(5),
            Duration::from_millis(5),
        ));
        let first = session_event(EventKind::SessionState, Map::new());
        let mut second = first.clone();
        second.id = "018f0000-0000-7000-8000-000000000421".into();
        second.sequence += 1;

        let (first_result, second_result) = tokio::join!(sink.emit(&first), sink.emit(&second));
        first_result.unwrap();
        second_result.unwrap();
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(sink.queue.lock().await.pending.len(), 2);

        sink.shutdown().await.unwrap();
        store.release();
    }

    #[tokio::test]
    async fn reverse_suggestion_completion_uses_durable_request_associations() {
        const REQUEST_A: &str = "018f0000-0000-7000-8000-000000000101";
        const REQUEST_B: &str = "018f0000-0000-7000-8000-000000000102";
        const TURN_A: &str = "018f0000-0000-7000-8000-000000000201";
        const TURN_B: &str = "018f0000-0000-7000-8000-000000000202";
        let order = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(FakeDurableEventStore {
            associations: Mutex::new(HashMap::from([
                (REQUEST_A.into(), TURN_A.into()),
                (REQUEST_B.into(), TURN_B.into()),
            ])),
            order: order.clone(),
            ..Default::default()
        });
        let sink = PersistenceAwareEventSink::new(
            "workspace",
            store.clone(),
            Arc::new(OrderedEventSink {
                order: order.clone(),
                events: Mutex::new(Vec::new()),
            }),
            Arc::new(RecordingStorageHealthReporter {
                order,
                reports: Mutex::new(Vec::new()),
            }),
        );
        let completion = |id: &str, request_id: &str, suggestion_id: &str| Envelope {
            id: id.into(),
            correlation_id: Some(request_id.into()),
            ..session_event(
                EventKind::SuggestionCompleted,
                Map::from_iter([
                    ("suggestion_id".into(), Value::String(suggestion_id.into())),
                    ("text".into(), Value::String("answer".into())),
                ]),
            )
        };

        sink.emit(&completion(
            "018f0000-0000-7000-8000-000000000011",
            REQUEST_B,
            "018f0000-0000-7000-8000-000000000302",
        ))
        .await
        .unwrap();
        sink.emit(&completion(
            "018f0000-0000-7000-8000-000000000010",
            REQUEST_A,
            "018f0000-0000-7000-8000-000000000301",
        ))
        .await
        .unwrap();

        let stored = store.events.lock().unwrap();
        assert_eq!(stored[0].request_id.as_deref(), Some(REQUEST_B));
        assert_eq!(stored[0].turn_id.as_deref(), Some(TURN_B));
        assert_eq!(stored[1].request_id.as_deref(), Some(REQUEST_A));
        assert_eq!(stored[1].turn_id.as_deref(), Some(TURN_A));
    }
}
