use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
    ProtocolKind, MAX_FRAME_BYTES,
};
use crate::storage::{
    AppendEventResult, AssociateRequestResult, NewTimelineEvent, RequestTurnAssociation,
    SessionRepository, SessionStatus, TimelineEventKind,
};

pub const SIDECAR_PROGRAM: &str = "callerinterview-sidecar";
pub const SIDECAR_EVENT: &str = "sidecar://event";
pub const STORAGE_HEALTH_EVENT: &str = "storage://health";
pub const PRODUCTION_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_DIAGNOSTICS: usize = 20;
const MAX_PENDING_DURABLE_EVENTS: usize = 1_024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const STORAGE_PERSIST_TIMEOUT: Duration = Duration::from_secs(5);
const STORAGE_RETRY_DELAY: Duration = Duration::from_millis(250);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

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

    fn degraded(code: &'static str) -> Self {
        Self {
            status: StorageHealthStatus::Degraded,
            code: Some(code),
            message: Some(
                "Session history could not be saved. The live session is still available.",
            ),
            recoverable: true,
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

pub trait DurableEventStore: Send + Sync {
    fn append_event(&self, event: &NewTimelineEvent) -> Result<AppendEventResult, ()>;
    fn append_terminal_event(
        &self,
        event: &NewTimelineEvent,
        status: SessionStatus,
        completed_at_ms: i64,
    ) -> Result<AppendEventResult, ()>;
    fn associate_request_with_turn(
        &self,
        association: &RequestTurnAssociation,
    ) -> Result<AssociateRequestResult, ()>;
    fn resolve_request_turn(
        &self,
        workspace_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<String>, ()>;
}

impl DurableEventStore for SessionRepository {
    fn append_event(&self, event: &NewTimelineEvent) -> Result<AppendEventResult, ()> {
        SessionRepository::append_event(self, event).map_err(|_| ())
    }

    fn append_terminal_event(
        &self,
        event: &NewTimelineEvent,
        status: SessionStatus,
        completed_at_ms: i64,
    ) -> Result<AppendEventResult, ()> {
        SessionRepository::append_terminal_event(self, event, status, completed_at_ms)
            .map_err(|_| ())
    }

    fn associate_request_with_turn(
        &self,
        association: &RequestTurnAssociation,
    ) -> Result<AssociateRequestResult, ()> {
        SessionRepository::associate_request_with_turn(self, association).map_err(|_| ())
    }

    fn resolve_request_turn(
        &self,
        workspace_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<String>, ()> {
        SessionRepository::resolve_request_turn(self, workspace_id, session_id, request_id)
            .map_err(|_| ())
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
    delivery: AsyncMutex<()>,
    launcher: Option<Arc<dyn SidecarLauncher>>,
    sink: Arc<dyn SidecarEventSink>,
    restart_delay: Duration,
    handshake_timeout: Option<Duration>,
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
                    }),
                    ..SupervisorData::default()
                }),
                lifecycle: AsyncMutex::new(()),
                delivery: AsyncMutex::new(()),
                launcher: None,
                sink: Arc::new(NoopEventSink),
                restart_delay: Duration::ZERO,
                handshake_timeout: None,
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
        Self {
            inner: Arc::new(SupervisorInner {
                data: AsyncMutex::new(SupervisorData::default()),
                lifecycle: AsyncMutex::new(()),
                delivery: AsyncMutex::new(()),
                launcher: Some(launcher),
                sink,
                restart_delay,
                handshake_timeout,
            }),
        }
    }

    pub async fn start(&self) -> Result<SidecarStatus, SidecarError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
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

    pub async fn send(&self, command: Envelope) -> Result<(), SidecarError> {
        validate_command(&command).map_err(|error| {
            SidecarError::new(error.code(), "sidecar command validation failed")
        })?;
        let bytes = encode_frame(&command)
            .map_err(|error| SidecarError::new(error.code(), error.to_string()))?;
        let (port, generation) = {
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
            (active.port.clone(), active.generation)
        };
        match port.write(bytes).await {
            Ok(()) => Ok(()),
            Err(error) => {
                if port.is_poisoned() {
                    self.handle_poisoned_write(generation, &error).await;
                }
                Err(error)
            }
        }
    }

    pub async fn accept_stdout(&self, generation: u64, chunk: &[u8]) -> Result<(), SidecarError> {
        let _delivery = self.inner.delivery.lock().await;
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
            let envelopes = active
                .decoder
                .push(chunk)
                .map_err(|error| SidecarError::new(error.code(), error.to_string()))?;
            envelopes
        };
        let mut first_error = None;
        for event in envelopes {
            let accepted = match {
                let mut data = self.inner.data.lock().await;
                self.accept_event_locked(&mut data, generation, event)
            } {
                Ok(accepted) => accepted,
                Err(error) => {
                    first_error.get_or_insert(error);
                    continue;
                }
            };
            if let Some(event) = accepted {
                if let Err(error) = self.inner.sink.emit(&event).await {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub async fn accept_event(&self, generation: u64, event: Envelope) -> Result<(), SidecarError> {
        let _delivery = self.inner.delivery.lock().await;
        let accepted = {
            let mut data = self.inner.data.lock().await;
            self.accept_event_locked(&mut data, generation, event)?
        };
        if let Some(event) = accepted {
            self.inner.sink.emit(&event).await?;
        }
        Ok(())
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
        let _delivery = self.inner.delivery.lock().await;
        let generation = {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = true;
            data.state = SidecarState::Stopped;
            data.active.as_ref().map(|active| active.generation)
        };
        if let Some(generation) = generation {
            self.cleanup_generation(generation).await?;
        }
        Ok(())
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
    let mut receiving_controls = true;
    loop {
        tokio::select! {
            status = child.wait() => {
                if status.is_ok() {
                    let _ = exit.send(ProcessExit::Reaped);
                    return;
                }
            }
            request = receiver.recv(), if receiving_controls => match request {
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
                None => receiving_controls = false,
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
    persistence_timeout: Duration,
    retry_delay: Duration,
}

#[derive(Clone)]
struct PendingDurableEvent {
    source_generation: u64,
    event: Envelope,
    acknowledgement: watch::Sender<PersistenceAcknowledgement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PersistenceAcknowledgement {
    Pending,
    Persisted,
    Degraded(&'static str),
}

#[derive(Default)]
struct DurableEventQueue {
    events: VecDeque<PendingDurableEvent>,
    worker_running: bool,
    degraded_code: Option<&'static str>,
    sticky_degraded: bool,
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
        Self {
            workspace_id: workspace_id.into(),
            store,
            downstream,
            health,
            source_generation: AtomicU64::new(0),
            queue: Arc::new(AsyncMutex::new(DurableEventQueue::default())),
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

    fn spawn_persistence_worker(&self) {
        tokio::spawn(run_persistence_worker(
            self.workspace_id.clone(),
            self.store.clone(),
            self.health.clone(),
            self.queue.clone(),
            self.persistence_timeout,
            self.retry_delay,
        ));
    }
}

#[async_trait]
impl SidecarEventSink for PersistenceAwareEventSink {
    async fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
        if matches!(&event.kind, ProtocolKind::Event(EventKind::SidecarReady)) {
            self.source_generation.fetch_add(1, Ordering::SeqCst);
        } else if Self::is_persistence_candidate(event) {
            let mut acknowledgement = None;
            let mut wait_for_acknowledgement = false;
            let mut start_worker = false;
            let mut report_code = None;
            {
                let mut queue = self.queue.lock().await;
                if let Some(existing) = queue.events.iter().find(|item| item.event.id == event.id) {
                    if existing.event == *event {
                        acknowledgement = Some(existing.acknowledgement.subscribe());
                        wait_for_acknowledgement = queue.degraded_code.is_none();
                    } else {
                        let code = "storage_event_collision";
                        let should_report = queue.degraded_code != Some(code);
                        queue.degraded_code = Some(code);
                        queue.sticky_degraded = true;
                        report_code = should_report.then_some(code);
                    }
                } else if queue.events.len() >= MAX_PENDING_DURABLE_EVENTS {
                    let code = "storage_backlog_full";
                    let should_report = queue.degraded_code != Some(code);
                    queue.degraded_code = Some(code);
                    queue.sticky_degraded = true;
                    report_code = should_report.then_some(code);
                } else {
                    wait_for_acknowledgement = queue.degraded_code.is_none();
                    let (sender, receiver) = watch::channel(PersistenceAcknowledgement::Pending);
                    queue.events.push_back(PendingDurableEvent {
                        source_generation: self.source_generation.load(Ordering::SeqCst),
                        event: event.clone(),
                        acknowledgement: sender,
                    });
                    acknowledgement = Some(receiver);
                    if !queue.worker_running {
                        queue.worker_running = true;
                        start_worker = true;
                    }
                }
            }

            if let Some(code) = report_code {
                let _ = self.health.report(&StorageHealth::degraded(code));
            }
            if start_worker {
                self.spawn_persistence_worker();
            }
            if wait_for_acknowledgement {
                let receiver = acknowledgement
                    .expect("a queued durable event must have an acknowledgement channel");
                if await_initial_persistence(receiver, self.persistence_timeout)
                    .await
                    .is_none()
                {
                    report_storage_degraded(
                        &self.queue,
                        self.health.as_ref(),
                        Self::storage_failure_code(event),
                    )
                    .await;
                }
            }
        }
        self.downstream.emit(event).await
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
    tokio::time::timeout(
        timeout.saturating_add(Duration::from_millis(10)),
        acknowledgement,
    )
    .await
    .ok()
}

async fn report_storage_degraded(
    queue: &Arc<AsyncMutex<DurableEventQueue>>,
    health: &dyn StorageHealthReporter,
    code: &'static str,
) {
    let should_report = {
        let mut queue = queue.lock().await;
        if queue.sticky_degraded {
            return;
        }
        let should_report = queue.degraded_code != Some(code);
        queue.degraded_code = Some(code);
        should_report
    };
    if should_report {
        let _ = health.report(&StorageHealth::degraded(code));
    }
}

fn spawn_persistence_attempt(
    store: Arc<dyn DurableEventStore>,
    workspace_id: String,
    item: PendingDurableEvent,
) -> tokio::task::JoinHandle<Result<(), ()>> {
    tokio::task::spawn_blocking(move || {
        persist_durable_event(
            store.as_ref(),
            &workspace_id,
            item.source_generation,
            &item.event,
        )
    })
}

async fn run_persistence_worker(
    workspace_id: String,
    store: Arc<dyn DurableEventStore>,
    health: Arc<dyn StorageHealthReporter>,
    queue: Arc<AsyncMutex<DurableEventQueue>>,
    persistence_timeout: Duration,
    retry_delay: Duration,
) {
    loop {
        let item = {
            let mut queue_state = queue.lock().await;
            let Some(item) = queue_state.events.front().cloned() else {
                queue_state.worker_running = false;
                return;
            };
            item
        };
        let failure_code = PersistenceAwareEventSink::storage_failure_code(&item.event);
        let mut attempt =
            spawn_persistence_attempt(store.clone(), workspace_id.clone(), item.clone());

        loop {
            match tokio::time::timeout(persistence_timeout, &mut attempt).await {
                Ok(Ok(Ok(()))) => {
                    let report_ready = {
                        let mut queue_state = queue.lock().await;
                        let persisted = queue_state
                            .events
                            .pop_front()
                            .expect("persistence worker must own the FIFO head");
                        debug_assert_eq!(persisted.event, item.event);
                        if queue_state.events.is_empty() && !queue_state.sticky_degraded {
                            queue_state.degraded_code = None;
                            true
                        } else {
                            false
                        }
                    };
                    if report_ready {
                        let _ = health.report(&StorageHealth::ready());
                    }
                    item.acknowledgement
                        .send_replace(PersistenceAcknowledgement::Persisted);
                    break;
                }
                Ok(_) => {
                    report_storage_degraded(&queue, health.as_ref(), failure_code).await;
                    item.acknowledgement
                        .send_replace(PersistenceAcknowledgement::Degraded(failure_code));
                    tokio::time::sleep(retry_delay).await;
                    attempt = spawn_persistence_attempt(
                        store.clone(),
                        workspace_id.clone(),
                        item.clone(),
                    );
                }
                Err(_) => {
                    report_storage_degraded(&queue, health.as_ref(), failure_code).await;
                    item.acknowledgement
                        .send_replace(PersistenceAcknowledgement::Degraded(failure_code));
                    tokio::time::sleep(retry_delay).await;
                }
            }
        }
    }
}

fn persist_durable_event(
    store: &dyn DurableEventStore,
    workspace_id: &str,
    source_generation: u64,
    event: &Envelope,
) -> Result<(), ()> {
    let ProtocolKind::Event(kind) = &event.kind else {
        return Err(());
    };
    let session_id = event.session_id.as_deref().ok_or(())?;
    let timeline_kind = match kind {
        EventKind::SessionState => TimelineEventKind::SessionState,
        EventKind::TranscriptUpdated
            if event.payload.get("is_final").and_then(Value::as_bool) == Some(true) =>
        {
            TimelineEventKind::TranscriptFinal
        }
        EventKind::SuggestionCompleted => TimelineEventKind::SuggestionCompleted,
        _ => return Err(()),
    };
    let timestamp_ms = i64::try_from(event.timestamp_ms).map_err(|_| ())?;
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
            let request_id = request_id.as_deref().ok_or(())?;
            Some(
                store
                    .resolve_request_turn(workspace_id, session_id, request_id)?
                    .ok_or(())?,
            )
        }
        _ => None,
    };
    if matches!(kind, EventKind::TranscriptUpdated) {
        store.associate_request_with_turn(&RequestTurnAssociation {
            workspace_id: workspace_id.to_owned(),
            session_id: session_id.to_owned(),
            request_id: event.id.clone(),
            turn_id: turn_id.clone().ok_or(())?,
            created_at_ms: timestamp_ms,
        })?;
    }
    let durable_event = NewTimelineEvent {
        workspace_id: workspace_id.to_owned(),
        session_id: session_id.to_owned(),
        event_id: event.id.clone(),
        source_generation: i64::try_from(source_generation).map_err(|_| ())?,
        source_sequence: i64::try_from(event.sequence).map_err(|_| ())?,
        timestamp_ms,
        kind: timeline_kind,
        correlation_id: event.correlation_id.clone(),
        request_id,
        turn_id,
        payload: Value::Object(event.payload.clone()),
    };
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
    use std::task::Poll;
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::{json, Map, Value};
    use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex};

    use crate::protocol::{
        encode_frame, CommandKind, Envelope, EventKind, FrameDecoder, ProtocolKind,
        MAX_FRAME_BYTES, PROTOCOL_VERSION,
    };
    use crate::storage::{
        AppendEventResult, AssociateRequestResult, NewTimelineEvent, RequestTurnAssociation,
        SessionStatus,
    };

    use super::{
        packaged_sidecar_path, persist_durable_event, redact_diagnostic, DurableEventStore,
        PersistenceAwareEventSink, ProcessExit, ProcessRequest, SidecarError, SidecarEventSink,
        SidecarLauncher, SidecarPort, SidecarState, SidecarSupervisor, StorageHealth,
        StorageHealthReporter, StorageHealthStatus, TokioCommand, TokioSidecarPort,
        MAX_PENDING_DURABLE_EVENTS, SIDECAR_PROGRAM,
    };

    const SESSION_ID: &str = "018f0000-0000-7000-8000-000000000003";

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
    struct FakeDurableEventStore {
        events: Mutex<Vec<NewTimelineEvent>>,
        terminal_transitions: Mutex<Vec<(SessionStatus, i64)>>,
        associations: Mutex<HashMap<String, String>>,
        fail_writes: AtomicBool,
        order: Arc<Mutex<Vec<String>>>,
        writer_threads: Mutex<Vec<std::thread::ThreadId>>,
        persisted: tokio::sync::Notify,
    }

    impl DurableEventStore for FakeDurableEventStore {
        fn append_event(&self, event: &NewTimelineEvent) -> Result<AppendEventResult, ()> {
            self.writer_threads
                .lock()
                .unwrap()
                .push(std::thread::current().id());
            self.order.lock().unwrap().push("persist".into());
            if self.fail_writes.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(());
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
        ) -> Result<AppendEventResult, ()> {
            let result = self.append_event(event)?;
            self.terminal_transitions
                .lock()
                .unwrap()
                .push((status, completed_at_ms));
            Ok(result)
        }

        fn associate_request_with_turn(
            &self,
            association: &RequestTurnAssociation,
        ) -> Result<AssociateRequestResult, ()> {
            self.order.lock().unwrap().push("associate".into());
            let mut associations = self.associations.lock().unwrap();
            match associations.get(&association.request_id) {
                Some(turn_id) if turn_id == &association.turn_id => {
                    Ok(AssociateRequestResult::Duplicate)
                }
                Some(_) => Err(()),
                None => {
                    associations
                        .insert(association.request_id.clone(), association.turn_id.clone());
                    Ok(AssociateRequestResult::Inserted)
                }
            }
        }

        fn resolve_request_turn(
            &self,
            _workspace_id: &str,
            _session_id: &str,
            request_id: &str,
        ) -> Result<Option<String>, ()> {
            Ok(self.associations.lock().unwrap().get(request_id).cloned())
        }
    }

    struct SlowDurableEventStore {
        delay: Duration,
    }

    impl DurableEventStore for SlowDurableEventStore {
        fn append_event(&self, _event: &NewTimelineEvent) -> Result<AppendEventResult, ()> {
            std::thread::sleep(self.delay);
            Ok(AppendEventResult::Inserted { host_sequence: 1 })
        }

        fn append_terminal_event(
            &self,
            event: &NewTimelineEvent,
            _status: SessionStatus,
            _completed_at_ms: i64,
        ) -> Result<AppendEventResult, ()> {
            self.append_event(event)
        }

        fn associate_request_with_turn(
            &self,
            _association: &RequestTurnAssociation,
        ) -> Result<AssociateRequestResult, ()> {
            Ok(AssociateRequestResult::Inserted)
        }

        fn resolve_request_turn(
            &self,
            _workspace_id: &str,
            _session_id: &str,
            _request_id: &str,
        ) -> Result<Option<String>, ()> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct BlockingDurableEventStore {
        gate: (Mutex<bool>, Condvar),
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
        fn append_event(&self, event: &NewTimelineEvent) -> Result<AppendEventResult, ()> {
            self.attempts.fetch_add(1, AtomicOrdering::SeqCst);
            let active = self.active_writes.fetch_add(1, AtomicOrdering::SeqCst) + 1;
            self.max_active_writes
                .fetch_max(active, AtomicOrdering::SeqCst);
            self.started.notify_one();

            let mut released = self.gate.0.lock().unwrap();
            while !*released {
                released = self.gate.1.wait(released).unwrap();
            }
            drop(released);

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
        ) -> Result<AppendEventResult, ()> {
            self.append_event(event)
        }

        fn associate_request_with_turn(
            &self,
            association: &RequestTurnAssociation,
        ) -> Result<AssociateRequestResult, ()> {
            let mut associations = self.associations.lock().unwrap();
            match associations.get(&association.request_id) {
                Some(turn_id) if turn_id == &association.turn_id => {
                    Ok(AssociateRequestResult::Duplicate)
                }
                Some(_) => Err(()),
                None => {
                    associations
                        .insert(association.request_id.clone(), association.turn_id.clone());
                    Ok(AssociateRequestResult::Inserted)
                }
            }
        }

        fn resolve_request_turn(
            &self,
            _workspace_id: &str,
            _session_id: &str,
            request_id: &str,
        ) -> Result<Option<String>, ()> {
            Ok(self.associations.lock().unwrap().get(request_id).cloned())
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
        fn append_event(&self, event: &NewTimelineEvent) -> Result<AppendEventResult, ()> {
            if self.attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                return Err(());
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
        ) -> Result<AppendEventResult, ()> {
            self.append_event(event)
        }

        fn associate_request_with_turn(
            &self,
            association: &RequestTurnAssociation,
        ) -> Result<AssociateRequestResult, ()> {
            let mut associations = self.associations.lock().unwrap();
            match associations.get(&association.request_id) {
                Some(turn_id) if turn_id == &association.turn_id => {
                    Ok(AssociateRequestResult::Duplicate)
                }
                Some(_) => Err(()),
                None => {
                    associations
                        .insert(association.request_id.clone(), association.turn_id.clone());
                    Ok(AssociateRequestResult::Inserted)
                }
            }
        }

        fn resolve_request_turn(
            &self,
            _workspace_id: &str,
            _session_id: &str,
            request_id: &str,
        ) -> Result<Option<String>, ()> {
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
            supervisor.send(malformed).await.unwrap_err().code(),
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
    async fn shutdown_does_not_wait_for_a_blocked_port_write() {
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
            async move { supervisor.send(fixture_command()).await }
        });
        started.await;

        assert!(
            tokio::time::timeout(Duration::from_millis(50), supervisor.shutdown())
                .await
                .is_ok()
        );
        port.release.notify_waiters();
        let _ = send.await;
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

        assert_eq!(
            &*order.lock().unwrap(),
            &["associate", "persist", "health:ready", "emit"]
        );
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
        assert_eq!(
            &*order.lock().unwrap(),
            &[
                "persist",
                "health:degraded",
                "emit",
                "emit",
                "persist",
                "persist",
                "health:ready",
            ]
        );
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
        assert_eq!(
            &*order.lock().unwrap(),
            &[
                "associate",
                "persist",
                "health:ready",
                "emit",
                "persist",
                "health:ready",
                "emit",
            ]
        );
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
        assert_eq!(
            sink.queue.lock().await.events.len(),
            MAX_PENDING_DURABLE_EVENTS
        );
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);
        store.release();
    }

    #[tokio::test]
    async fn pending_same_id_with_changed_envelope_is_a_live_only_collision() {
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
        sink.emit(&collision).await.unwrap();

        assert_eq!(downstream.events.lock().unwrap().len(), 2);
        assert!(health
            .reports
            .lock()
            .unwrap()
            .iter()
            .any(|report| report.code == Some("storage_event_collision")));
        assert_eq!(sink.queue.lock().await.events.len(), 1);
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);

        store.release();
        tokio::time::timeout(Duration::from_millis(250), store.persisted.notified())
            .await
            .expect("the original event must persist after the store recovers");
        let stored = store.events.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].source_sequence,
            i64::try_from(original.sequence).unwrap()
        );
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
        assert_eq!(sink.queue.lock().await.events.len(), 1);
        store.release();
        tokio::time::timeout(Duration::from_millis(250), store.persisted.notified())
            .await
            .expect("the original pending write must complete");

        assert_eq!(downstream.events.lock().unwrap().len(), 2);
        assert_eq!(store.attempts.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(store.events.lock().unwrap().len(), 1);
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
