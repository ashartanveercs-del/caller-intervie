use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::{
    process::{CommandChild, CommandEvent},
    ShellExt,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::protocol::{
    encode_frame, validate_command, validate_event, CommandKind, Envelope, EventKind, FrameDecoder,
    MAX_FRAME_BYTES,
};

pub const SIDECAR_PROGRAM: &str = "callerinterview-sidecar";
pub const SIDECAR_EVENT: &str = "sidecar://event";
pub const PRODUCTION_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_DIAGNOSTICS: usize = 20;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

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
    fn observe(&self, _supervisor: SidecarSupervisor, _generation: u64) {}
}

#[async_trait]
pub trait SidecarLauncher: Send + Sync {
    async fn launch(&self) -> Result<Arc<dyn SidecarPort>, SidecarError>;
}

pub trait SidecarEventSink: Send + Sync {
    fn emit(&self, event: &Envelope) -> Result<(), SidecarError>;
}

#[derive(Default)]
struct NoopEventSink;

impl SidecarEventSink for NoopEventSink {
    fn emit(&self, _event: &Envelope) -> Result<(), SidecarError> {
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
                launcher: Some(launcher),
                sink,
                restart_delay,
                handshake_timeout,
            }),
        }
    }

    pub async fn start(&self) -> Result<SidecarStatus, SidecarError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
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
        let port = {
            let data = self.inner.data.lock().await;
            if !matches!(data.state, SidecarState::Ready) {
                return Err(SidecarError::new(
                    "sidecar_not_ready",
                    "sidecar has not completed its handshake",
                ));
            }
            data.active
                .as_ref()
                .map(|active| active.port.clone())
                .ok_or_else(|| {
                    SidecarError::new("sidecar_unavailable", "sidecar port is unavailable")
                })?
        };
        port.write(bytes).await
    }

    pub async fn accept_stdout(&self, generation: u64, chunk: &[u8]) -> Result<(), SidecarError> {
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
        for event in envelopes {
            self.accept_event_locked(&mut data, generation, event)?;
        }
        Ok(())
    }

    pub async fn accept_event(&self, generation: u64, event: Envelope) -> Result<(), SidecarError> {
        let mut data = self.inner.data.lock().await;
        self.accept_event_locked(&mut data, generation, event)
    }

    fn accept_event_locked(
        &self,
        data: &mut SupervisorData,
        generation: u64,
        event: Envelope,
    ) -> Result<(), SidecarError> {
        let Some(active) = data.active.as_ref() else {
            return Ok(());
        };
        if active.generation != generation || data.generation != generation {
            return Ok(());
        }
        validate_event(&event)
            .map_err(|error| SidecarError::new(error.code(), "sidecar event validation failed"))?;
        let is_ready = matches!(
            event.kind,
            crate::protocol::ProtocolKind::Event(EventKind::SidecarReady)
        );
        if is_ready {
            if active.handshake_id.as_deref() != event.correlation_id.as_deref() {
                return Ok(());
            }
            data.state = SidecarState::Ready;
        } else if !matches!(data.state, SidecarState::Ready) {
            return Err(SidecarError::new(
                "sidecar_not_ready",
                "sidecar emitted an event before sidecar.ready",
            ));
        }
        self.inner.sink.emit(&event)
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
        for line in lines {
            self.push_diagnostic_locked(&mut data, &format!("stderr: {line}"));
        }
    }

    pub async fn handle_unexpected_exit(&self, generation: u64) {
        let _lifecycle = self.inner.lifecycle.lock().await;
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
        let port = {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = true;
            data.state = SidecarState::Stopped;
            data.active.take().map(|active| {
                data.intentional_exit_generations.insert(active.generation);
                active.port
            })
        };
        if let Some(port) = port {
            port.kill().await?;
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
            self.discard_generation(generation).await;
            let _ = port.kill().await;
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
        let port = {
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
            data.active.take().map(|active| active.port)
        };
        if let Some(port) = port {
            let _ = port.kill().await;
        }
        self.restart_after_unexpected_exit_internal().await;
    }

    async fn stop_current_for_restart_internal(&self) -> Result<(), SidecarError> {
        let port = {
            let mut data = self.inner.data.lock().await;
            data.active.take().map(|active| {
                data.intentional_exit_generations.insert(active.generation);
                active.port
            })
        };
        if let Some(port) = port {
            port.kill().await?;
        }
        Ok(())
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
            self.push_diagnostic_locked(data, &format!("stderr: {line}"));
        }
    }

    async fn discard_generation(&self, generation: u64) {
        let mut data = self.inner.data.lock().await;
        if data
            .active
            .as_ref()
            .is_some_and(|active| active.generation == generation)
        {
            data.active = None;
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
    let mut result = value.to_owned();
    for marker in [
        "DEEPSEEK_API_KEY=",
        "DEEPGRAM_API_KEY=",
        "OPENAI_API_KEY=",
        "Bearer ",
        "sk-",
    ] {
        let mut cursor = 0;
        while let Some(offset) = result[cursor..].find(marker) {
            let start = cursor + offset;
            let value_start = start + marker.len();
            let value_end = result[value_start..]
                .find(char::is_whitespace)
                .map(|offset| value_start + offset)
                .unwrap_or(result.len());
            result.replace_range(value_start..value_end, "<redacted>");
            cursor = value_start + "<redacted>".len();
        }
    }
    result
}

#[cfg(windows)]
fn terminate_process_by_pid(pid: u32) -> Result<(), SidecarError> {
    let status = std::process::Command::new("taskkill.exe")
        .args(["/pid", &pid.to_string(), "/t", "/f"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|error| SidecarError::new("sidecar_kill_failed", error.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(SidecarError::new(
            "sidecar_kill_failed",
            "taskkill could not terminate the sidecar process",
        ))
    }
}

#[cfg(not(windows))]
fn terminate_process_by_pid(_pid: u32) -> Result<(), SidecarError> {
    Err(SidecarError::new(
        "sidecar_kill_failed",
        "independent sidecar termination is not available on this platform",
    ))
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
        let (events, child) = self
            .app
            .shell()
            .sidecar(SIDECAR_PROGRAM)
            .map_err(|error| SidecarError::new("sidecar_launch_failed", error.to_string()))?
            .set_raw_out(true)
            .spawn()
            .map_err(|error| SidecarError::new("sidecar_launch_failed", error.to_string()))?;
        let pid = child.pid();
        Ok(Arc::new(TauriSidecarPort {
            child: Arc::new(Mutex::new(Some(child))),
            pid,
            terminated: Arc::new(AtomicBool::new(false)),
            events: Arc::new(AsyncMutex::new(Some(events))),
        }))
    }
}

struct TauriSidecarPort {
    child: Arc<Mutex<Option<CommandChild>>>,
    pid: u32,
    terminated: Arc<AtomicBool>,
    events: Arc<AsyncMutex<Option<tauri::async_runtime::Receiver<CommandEvent>>>>,
}

#[async_trait]
impl SidecarPort for TauriSidecarPort {
    async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
        if self.terminated.load(Ordering::Acquire) {
            return Err(SidecarError::new(
                "sidecar_unavailable",
                "sidecar child is unavailable",
            ));
        }
        let mut child = self
            .child
            .lock()
            .map_err(|_| {
                SidecarError::new("sidecar_write_failed", "sidecar child lock was poisoned")
            })?
            .take()
            .ok_or_else(|| {
                SidecarError::new("sidecar_unavailable", "sidecar child is unavailable")
            })?;
        let slot = self.child.clone();
        let terminated = self.terminated.clone();
        let write = tokio::task::spawn_blocking(move || {
            let result = child.write(&bytes);
            if terminated.load(Ordering::Acquire) {
                let _ = child.kill();
            } else if let Ok(mut slot) = slot.lock() {
                *slot = Some(child);
            }
            result
        });
        match tokio::time::timeout(WRITE_TIMEOUT, write).await {
            Ok(Ok(result)) => {
                result.map_err(|error| SidecarError::new("sidecar_write_failed", error.to_string()))
            }
            Ok(Err(error)) => Err(SidecarError::new("sidecar_write_failed", error.to_string())),
            Err(_) => Err(SidecarError::new(
                "sidecar_write_timeout",
                "sidecar stdin write timed out",
            )),
        }
    }

    async fn kill(&self) -> Result<(), SidecarError> {
        self.terminated.store(true, Ordering::Release);
        let child = self
            .child
            .lock()
            .map_err(|_| {
                SidecarError::new("sidecar_kill_failed", "sidecar child lock was poisoned")
            })?
            .take();
        match child {
            Some(child) => child
                .kill()
                .map_err(|error| SidecarError::new("sidecar_kill_failed", error.to_string())),
            None => terminate_process_by_pid(self.pid),
        }
    }

    fn observe(&self, supervisor: SidecarSupervisor, generation: u64) {
        let events = self.events.clone();
        tauri::async_runtime::spawn(async move {
            let Some(mut receiver) = events.lock().await.take() else {
                return;
            };
            while let Some(event) = receiver.recv().await {
                match event {
                    CommandEvent::Stdout(chunk) => {
                        if let Err(error) = supervisor.accept_stdout(generation, &chunk).await {
                            supervisor.push_diagnostic(error.to_string()).await;
                        }
                    }
                    CommandEvent::Stderr(chunk) => {
                        supervisor.accept_stderr(generation, &chunk).await
                    }
                    CommandEvent::Terminated(_) | CommandEvent::Error(_) => {
                        supervisor.handle_unexpected_exit(generation).await;
                        return;
                    }
                    _ => {}
                }
            }
        });
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

impl SidecarEventSink for TauriEventSink {
    fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
        self.app
            .emit(SIDECAR_EVENT, event)
            .map_err(|error| SidecarError::new("sidecar_event_emit_failed", error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::{json, Map, Value};

    use crate::protocol::{
        encode_frame, CommandKind, Envelope, EventKind, FrameDecoder, ProtocolKind,
        MAX_FRAME_BYTES, PROTOCOL_VERSION,
    };

    use super::{
        packaged_sidecar_path, redact_diagnostic, SidecarError, SidecarEventSink, SidecarLauncher,
        SidecarPort, SidecarState, SidecarSupervisor, SIDECAR_PROGRAM,
    };

    #[cfg(windows)]
    use super::terminate_process_by_pid;

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

    impl SidecarEventSink for CollectingEventSink {
        fn emit(&self, event: &Envelope) -> Result<(), SidecarError> {
            self.events.lock().unwrap().push(event.clone());
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
        let diagnostic =
            redact_diagnostic("DEEPSEEK_API_KEY=secret-value Bearer api-token sk-abcdefghijklmnop");

        assert!(!diagnostic.contains("secret-value"));
        assert!(!diagnostic.contains("api-token"));
        assert!(!diagnostic.contains("sk-abcdefghijklmnop"));
        assert!(diagnostic.contains("<redacted>"));
    }

    #[cfg(windows)]
    #[test]
    fn forced_pid_termination_reaps_a_real_blocked_child() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "ping -n 30 127.0.0.1 > nul"])
            .spawn()
            .unwrap();

        terminate_process_by_pid(child.id()).unwrap();
        assert!(!child.wait().unwrap().success());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn blocked_write_timeout_keeps_an_actual_child_killable() {
        #[derive(Clone)]
        struct RealBlockingPort {
            pid: u32,
            started: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl SidecarPort for RealBlockingPort {
            async fn write(&self, _bytes: Vec<u8>) -> Result<(), SidecarError> {
                self.started.notify_waiters();
                self.release.notified().await;
                Err(SidecarError::new(
                    "sidecar_write_timeout",
                    "simulated blocked stdin write timed out",
                ))
            }

            async fn kill(&self) -> Result<(), SidecarError> {
                terminate_process_by_pid(self.pid)
            }
        }

        let mut child = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "ping -n 30 127.0.0.1 > nul"])
            .spawn()
            .unwrap();
        let port = RealBlockingPort {
            pid: child.id(),
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

        tokio::time::timeout(Duration::from_millis(100), supervisor.shutdown())
            .await
            .unwrap()
            .unwrap();
        port.release.notify_waiters();
        assert_eq!(
            send.await.unwrap().unwrap_err().code(),
            "sidecar_write_timeout"
        );
        assert!(!child.wait().unwrap().success());
    }
}
