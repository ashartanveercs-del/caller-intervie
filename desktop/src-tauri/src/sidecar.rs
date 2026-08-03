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

pub fn smoke_packaged_sidecar() -> Result<(), SidecarError> {
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::Instant;

    let host = std::env::current_exe()
        .map_err(|error| SidecarError::new("sidecar_smoke_failed", error.to_string()))?;
    let path = packaged_sidecar_path(&host);
    let mut child = std::process::Command::new(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| SidecarError::new("sidecar_smoke_failed", error.to_string()))?;
    let handshake = Envelope {
        version: crate::protocol::PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        session_id: None,
        sequence: 0,
        timestamp_ms: 0,
        kind: CommandKind::HandshakeRequest.into(),
        payload: Default::default(),
        correlation_id: None,
    };
    child
        .stdin
        .as_mut()
        .ok_or_else(|| SidecarError::new("sidecar_smoke_failed", "sidecar stdin is unavailable"))?
        .write_all(
            &encode_frame(&handshake)
                .map_err(|error| SidecarError::new(error.code(), error.to_string()))?,
        )
        .map_err(|error| SidecarError::new("sidecar_smoke_failed", error.to_string()))?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        SidecarError::new("sidecar_smoke_failed", "sidecar stdout is unavailable")
    })?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 8192];
        while let Ok(length) = stdout.read(&mut chunk) {
            if length == 0 {
                break;
            }
            if sender.send(chunk[..length].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + PRODUCTION_HANDSHAKE_TIMEOUT;
    let mut decoder = FrameDecoder::new(MAX_FRAME_BYTES);
    let result = 'ready: loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let chunk = receiver.recv_timeout(remaining).map_err(|_| {
            SidecarError::new(
                "sidecar_handshake_timeout",
                "sidecar did not emit sidecar.ready within 45 seconds",
            )
        })?;
        for event in decoder
            .push(&chunk)
            .map_err(|error| SidecarError::new(error.code(), error.to_string()))?
        {
            if matches!(
                event.kind,
                crate::protocol::ProtocolKind::Event(EventKind::SidecarReady)
            ) {
                validate_event(&event).map_err(|error| {
                    SidecarError::new(error.code(), "sidecar emitted an invalid ready event")
                })?;
                break 'ready Ok(());
            }
        }
    };
    let _ = child.kill();
    result
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
    ignored_exit_events: u8,
    diagnostics: Vec<String>,
}

impl Default for SupervisorData {
    fn default() -> Self {
        Self {
            state: SidecarState::Starting,
            restart_count: 0,
            generation: 0,
            explicit_shutdown: false,
            ignored_exit_events: 0,
            diagnostics: Vec::new(),
        }
    }
}

struct SupervisorInner {
    data: AsyncMutex<SupervisorData>,
    port: AsyncMutex<Option<Arc<dyn SidecarPort>>>,
    decoder: AsyncMutex<FrameDecoder>,
    stderr: AsyncMutex<(u64, String)>,
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
                data: AsyncMutex::new(SupervisorData::default()),
                port: AsyncMutex::new(Some(port)),
                decoder: AsyncMutex::new(FrameDecoder::new(MAX_FRAME_BYTES)),
                stderr: AsyncMutex::new((0, String::new())),
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
                port: AsyncMutex::new(None),
                decoder: AsyncMutex::new(FrameDecoder::new(MAX_FRAME_BYTES)),
                stderr: AsyncMutex::new((0, String::new())),
                launcher: Some(launcher),
                sink,
                restart_delay,
                handshake_timeout,
            }),
        }
    }

    pub async fn start(&self) -> Result<SidecarStatus, SidecarError> {
        if let Err(error) = self.launch().await {
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
        if !matches!(self.inner.data.lock().await.state, SidecarState::Ready) {
            return Err(SidecarError::new(
                "sidecar_not_ready",
                "sidecar has not completed its handshake",
            ));
        }
        let bytes = encode_frame(&command)
            .map_err(|error| SidecarError::new(error.code(), error.to_string()))?;
        let port = self.inner.port.lock().await.clone().ok_or_else(|| {
            SidecarError::new("sidecar_unavailable", "sidecar port is unavailable")
        })?;
        port.write(bytes).await
    }

    pub async fn accept_stdout(&self, generation: u64, chunk: &[u8]) -> Result<(), SidecarError> {
        if !self.is_current(generation).await {
            return Ok(());
        }
        let envelopes = self
            .inner
            .decoder
            .lock()
            .await
            .push(chunk)
            .map_err(|error| SidecarError::new(error.code(), error.to_string()))?;
        for event in envelopes {
            self.accept_event(generation, event).await?;
        }
        Ok(())
    }

    pub async fn accept_event(&self, generation: u64, event: Envelope) -> Result<(), SidecarError> {
        if !self.is_current(generation).await {
            return Ok(());
        }
        validate_event(&event)
            .map_err(|error| SidecarError::new(error.code(), "sidecar event validation failed"))?;
        let is_ready = matches!(
            event.kind,
            crate::protocol::ProtocolKind::Event(EventKind::SidecarReady)
        );
        {
            let mut data = self.inner.data.lock().await;
            if is_ready {
                data.state = SidecarState::Ready;
            } else if !matches!(data.state, SidecarState::Ready) {
                return Err(SidecarError::new(
                    "sidecar_not_ready",
                    "sidecar emitted an event before sidecar.ready",
                ));
            }
        }
        self.inner.sink.emit(&event)
    }

    pub async fn accept_stderr(&self, generation: u64, stderr: &[u8]) {
        if !self.is_current(generation).await {
            return;
        }
        let mut pending = self.inner.stderr.lock().await;
        if pending.0 != generation {
            return;
        }
        pending.1.push_str(&String::from_utf8_lossy(stderr));
        while let Some(end) = pending.1.find(['\n', '\r']) {
            let line = pending.1.drain(..=end).collect::<String>();
            self.push_diagnostic(format!("stderr: {line}")).await;
        }
    }

    pub async fn handle_unexpected_exit(&self, generation: u64) {
        self.flush_stderr(generation).await;
        {
            let mut data = self.inner.data.lock().await;
            if data.generation != generation {
                return;
            }
            if data.ignored_exit_events > 0 {
                data.ignored_exit_events -= 1;
                return;
            }
            if data.explicit_shutdown {
                data.state = SidecarState::Stopped;
                return;
            }
        }
        self.restart_after_unexpected_exit().await;
    }

    async fn restart_after_unexpected_exit(&self) {
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
        if let Err(error) = self.launch().await {
            let mut data = self.inner.data.lock().await;
            data.state = SidecarState::Failed;
            self.push_diagnostic_locked(&mut data, &error.to_string());
        }
    }

    pub async fn restart(&self) -> Result<SidecarStatus, SidecarError> {
        self.stop_current_for_restart().await?;
        {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = false;
            data.state = SidecarState::Starting;
            data.restart_count = 0;
        }
        if let Err(error) = self.launch().await {
            self.fail(&error).await;
            return Err(error);
        }
        Ok(self.status().await)
    }

    pub async fn shutdown(&self) -> Result<(), SidecarError> {
        {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = true;
            data.state = SidecarState::Stopped;
        }
        if let Some(port) = self.inner.port.lock().await.take() {
            port.kill().await?;
        }
        Ok(())
    }

    async fn launch(&self) -> Result<(), SidecarError> {
        let launcher = self.inner.launcher.as_ref().ok_or_else(|| {
            SidecarError::new("sidecar_unavailable", "sidecar launcher is unavailable")
        })?;
        let port = launcher.launch().await?;
        *self.inner.port.lock().await = Some(port.clone());
        let generation = {
            let mut data = self.inner.data.lock().await;
            data.explicit_shutdown = false;
            data.state = SidecarState::Starting;
            data.generation += 1;
            data.generation
        };
        *self.inner.decoder.lock().await = FrameDecoder::new(MAX_FRAME_BYTES);
        *self.inner.stderr.lock().await = (generation, String::new());
        port.observe(self.clone(), generation);
        self.send_handshake().await?;
        self.arm_handshake_timeout(generation);
        Ok(())
    }

    async fn send_handshake(&self) -> Result<(), SidecarError> {
        let command = Envelope {
            version: crate::protocol::PROTOCOL_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            session_id: None,
            sequence: 0,
            timestamp_ms: 0,
            kind: CommandKind::HandshakeRequest.into(),
            payload: Default::default(),
            correlation_id: None,
        };
        let bytes = encode_frame(&command)
            .map_err(|error| SidecarError::new(error.code(), error.to_string()))?;
        let port = self.inner.port.lock().await.clone().ok_or_else(|| {
            SidecarError::new("sidecar_unavailable", "sidecar port is unavailable")
        })?;
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
        let timed_out = {
            let mut data = self.inner.data.lock().await;
            if data.generation != generation || !matches!(data.state, SidecarState::Starting) {
                false
            } else {
                data.state = SidecarState::Restarting;
                true
            }
        };
        if !timed_out {
            return;
        }

        self.push_diagnostic("sidecar handshake timed out").await;
        let port = self.inner.port.lock().await.take();
        if let Some(port) = port {
            self.inner.data.lock().await.ignored_exit_events += 1;
            let _ = port.kill().await;
        }
        self.restart_after_unexpected_exit().await;
    }

    async fn stop_current_for_restart(&self) -> Result<(), SidecarError> {
        if let Some(port) = self.inner.port.lock().await.take() {
            self.inner.data.lock().await.ignored_exit_events += 1;
            port.kill().await?;
        }
        Ok(())
    }

    async fn is_current(&self, generation: u64) -> bool {
        self.inner.data.lock().await.generation == generation
    }

    async fn push_diagnostic(&self, diagnostic: impl AsRef<str>) {
        let mut data = self.inner.data.lock().await;
        self.push_diagnostic_locked(&mut data, diagnostic.as_ref());
    }

    async fn flush_stderr(&self, generation: u64) {
        let line = {
            let mut pending = self.inner.stderr.lock().await;
            if pending.0 != generation {
                return;
            }
            std::mem::take(&mut pending.1)
        };
        if !line.is_empty() {
            self.push_diagnostic(format!("stderr: {line}")).await;
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
        Ok(Arc::new(TauriSidecarPort {
            child: Arc::new(Mutex::new(Some(child))),
            events: Arc::new(AsyncMutex::new(Some(events))),
        }))
    }
}

struct TauriSidecarPort {
    child: Arc<Mutex<Option<CommandChild>>>,
    events: Arc<AsyncMutex<Option<tauri::async_runtime::Receiver<CommandEvent>>>>,
}

#[async_trait]
impl SidecarPort for TauriSidecarPort {
    async fn write(&self, bytes: Vec<u8>) -> Result<(), SidecarError> {
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
        let write = tokio::task::spawn_blocking(move || {
            let result = child.write(&bytes);
            if let Ok(mut slot) = slot.lock() {
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
        let child = self
            .child
            .lock()
            .map_err(|_| {
                SidecarError::new("sidecar_kill_failed", "sidecar child lock was poisoned")
            })?
            .take()
            .ok_or_else(|| {
                SidecarError::new("sidecar_unavailable", "sidecar child is unavailable")
            })?;
        child
            .kill()
            .map_err(|error| SidecarError::new("sidecar_kill_failed", error.to_string()))
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
        packaged_sidecar_path, redact_diagnostic, SidecarError, SidecarLauncher, SidecarPort,
        SidecarState, SidecarSupervisor, SIDECAR_PROGRAM,
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
    async fn old_generation_bytes_and_ready_events_are_ignored_after_restart() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port,
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
            .accept_event(2, fixture_envelope())
            .await
            .unwrap();
        assert_eq!(supervisor.status().await.state, SidecarState::Ready);
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

        supervisor.handle_unexpected_exit(0).await;
        assert_eq!(supervisor.status().await.state, SidecarState::Starting);
        assert_eq!(*launcher.launches.lock().unwrap(), 1);

        supervisor.handle_unexpected_exit(1).await;
        assert_eq!(supervisor.status().await.state, SidecarState::Failed);
        assert_eq!(*launcher.launches.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn explicit_restart_resets_automatic_restart_budget() {
        let port = Arc::new(FakeSidecarPort::default());
        let launcher = FakeSidecarLauncher {
            port,
            launches: Arc::new(Mutex::new(0)),
        };
        let supervisor = SidecarSupervisor::with_launcher(Arc::new(launcher), Duration::ZERO);

        supervisor.handle_unexpected_exit(0).await;
        assert_eq!(supervisor.status().await.restart_count, 1);
        supervisor.restart().await.unwrap();

        assert_eq!(supervisor.status().await.restart_count, 0);
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
}
