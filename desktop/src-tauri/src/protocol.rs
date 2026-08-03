use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameError {
    code: &'static str,
    message: &'static str,
}

impl FrameError {
    fn too_large() -> Self {
        Self {
            code: "frame_too_large",
            message: "frame payload exceeds the configured maximum",
        }
    }

    fn invalid_frame() -> Self {
        Self {
            code: "invalid_frame",
            message: "frame contains invalid MessagePack",
        }
    }

    fn invalid_envelope() -> Self {
        Self {
            code: "invalid_envelope",
            message: "frame contains an invalid protocol envelope",
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for FrameError {}

pub struct FrameDecoder {
    max_frame_bytes: usize,
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new(max_frame_bytes: usize) -> Self {
        Self {
            max_frame_bytes,
            buffer: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Envelope>, FrameError> {
        self.buffer.extend_from_slice(chunk);
        let mut envelopes = Vec::new();

        loop {
            if self.buffer.len() < 4 {
                break;
            }

            let payload_size = u32::from_be_bytes(self.buffer[..4].try_into().unwrap()) as usize;
            if payload_size > self.max_frame_bytes {
                self.buffer.clear();
                return Err(FrameError::too_large());
            }
            let frame_size = 4 + payload_size;
            if self.buffer.len() < frame_size {
                break;
            }

            let payload = self.buffer[4..frame_size].to_vec();
            self.buffer.drain(..frame_size);
            let mut deserializer = rmp_serde::Deserializer::new(std::io::Cursor::new(&payload));
            let value =
                Value::deserialize(&mut deserializer).map_err(|_| FrameError::invalid_frame())?;
            if deserializer.position() != payload.len() as u64 {
                return Err(FrameError::invalid_frame());
            }
            let envelope =
                serde_json::from_value(value).map_err(|_| FrameError::invalid_envelope())?;
            envelopes.push(envelope);
        }

        Ok(envelopes)
    }
}

pub fn encode_frame(envelope: &Envelope) -> Result<Vec<u8>, FrameError> {
    let payload = rmp_serde::to_vec_named(envelope).map_err(|_| FrameError::invalid_frame())?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::too_large());
    }

    let payload_size = u32::try_from(payload.len()).map_err(|_| FrameError::too_large())?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&payload_size.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum CommandKind {
    #[serde(rename = "handshake.request")]
    HandshakeRequest,
    #[serde(rename = "session.start")]
    SessionStart,
    #[serde(rename = "session.stop")]
    SessionStop,
    #[serde(rename = "listening.set")]
    ListeningSet,
    #[serde(rename = "you_source.set")]
    YouSourceSet,
    #[serde(rename = "query.trigger")]
    QueryTrigger,
    #[serde(rename = "audio.system.set")]
    AudioSystemSet,
    #[serde(rename = "audio.device.set")]
    AudioDeviceSet,
    #[serde(rename = "knowledge.ingest")]
    KnowledgeIngest,
    #[serde(rename = "session.snapshot.request")]
    SessionSnapshotRequest,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum EventKind {
    #[serde(rename = "sidecar.ready")]
    SidecarReady,
    #[serde(rename = "session.state")]
    SessionState,
    #[serde(rename = "transcript.updated")]
    TranscriptUpdated,
    #[serde(rename = "suggestion.chunk")]
    SuggestionChunk,
    #[serde(rename = "suggestion.completed")]
    SuggestionCompleted,
    #[serde(rename = "audio.health")]
    AudioHealth,
    #[serde(rename = "provider.health")]
    ProviderHealth,
    #[serde(rename = "knowledge.state")]
    KnowledgeState,
    #[serde(rename = "runtime.error")]
    RuntimeError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolKind {
    Command(CommandKind),
    Event(EventKind),
}

impl ProtocolKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Command(CommandKind::HandshakeRequest) => "handshake.request",
            Self::Command(CommandKind::SessionStart) => "session.start",
            Self::Command(CommandKind::SessionStop) => "session.stop",
            Self::Command(CommandKind::ListeningSet) => "listening.set",
            Self::Command(CommandKind::YouSourceSet) => "you_source.set",
            Self::Command(CommandKind::QueryTrigger) => "query.trigger",
            Self::Command(CommandKind::AudioSystemSet) => "audio.system.set",
            Self::Command(CommandKind::AudioDeviceSet) => "audio.device.set",
            Self::Command(CommandKind::KnowledgeIngest) => "knowledge.ingest",
            Self::Command(CommandKind::SessionSnapshotRequest) => "session.snapshot.request",
            Self::Event(EventKind::SidecarReady) => "sidecar.ready",
            Self::Event(EventKind::SessionState) => "session.state",
            Self::Event(EventKind::TranscriptUpdated) => "transcript.updated",
            Self::Event(EventKind::SuggestionChunk) => "suggestion.chunk",
            Self::Event(EventKind::SuggestionCompleted) => "suggestion.completed",
            Self::Event(EventKind::AudioHealth) => "audio.health",
            Self::Event(EventKind::ProviderHealth) => "provider.health",
            Self::Event(EventKind::KnowledgeState) => "knowledge.state",
            Self::Event(EventKind::RuntimeError) => "runtime.error",
        }
    }
}

impl From<CommandKind> for ProtocolKind {
    fn from(value: CommandKind) -> Self {
        Self::Command(value)
    }
}

impl From<EventKind> for ProtocolKind {
    fn from(value: EventKind) -> Self {
        Self::Event(value)
    }
}

impl Serialize for ProtocolKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProtocolKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let kind = String::deserialize(deserializer)?;
        let value = match kind.as_str() {
            "handshake.request" => CommandKind::HandshakeRequest.into(),
            "session.start" => CommandKind::SessionStart.into(),
            "session.stop" => CommandKind::SessionStop.into(),
            "listening.set" => CommandKind::ListeningSet.into(),
            "you_source.set" => CommandKind::YouSourceSet.into(),
            "query.trigger" => CommandKind::QueryTrigger.into(),
            "audio.system.set" => CommandKind::AudioSystemSet.into(),
            "audio.device.set" => CommandKind::AudioDeviceSet.into(),
            "knowledge.ingest" => CommandKind::KnowledgeIngest.into(),
            "session.snapshot.request" => CommandKind::SessionSnapshotRequest.into(),
            "sidecar.ready" => EventKind::SidecarReady.into(),
            "session.state" => EventKind::SessionState.into(),
            "transcript.updated" => EventKind::TranscriptUpdated.into(),
            "suggestion.chunk" => EventKind::SuggestionChunk.into(),
            "suggestion.completed" => EventKind::SuggestionCompleted.into(),
            "audio.health" => EventKind::AudioHealth.into(),
            "provider.health" => EventKind::ProviderHealth.into(),
            "knowledge.state" => EventKind::KnowledgeState.into(),
            "runtime.error" => EventKind::RuntimeError.into(),
            _ => return Err(de::Error::custom("kind must be a known namespaced string")),
        };
        Ok(value)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    #[serde(deserialize_with = "deserialize_protocol_version")]
    pub version: u16,
    #[serde(deserialize_with = "deserialize_uuid_string")]
    pub id: String,
    #[serde(deserialize_with = "deserialize_optional_uuid_string")]
    pub session_id: Option<String>,
    #[serde(deserialize_with = "deserialize_safe_integer")]
    pub sequence: u64,
    #[serde(deserialize_with = "deserialize_safe_integer")]
    pub timestamp_ms: u64,
    pub kind: ProtocolKind,
    pub payload: Map<String, Value>,
    #[serde(deserialize_with = "deserialize_optional_uuid_string")]
    pub correlation_id: Option<String>,
}

fn deserialize_protocol_version<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    let version = u16::deserialize(deserializer)?;
    if version != PROTOCOL_VERSION {
        return Err(de::Error::custom("unsupported protocol version"));
    }
    Ok(version)
}

fn deserialize_safe_integer<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value > MAX_SAFE_INTEGER {
        return Err(de::Error::custom("must be a safe integer"));
    }
    Ok(value)
}

fn deserialize_uuid_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    validate_uuid(&value).map_err(de::Error::custom)?;
    Ok(value)
}

fn deserialize_optional_uuid_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    if let Some(value) = value.as_deref() {
        validate_uuid(value).map_err(de::Error::custom)?;
    }
    Ok(value)
}

fn validate_uuid(value: &str) -> Result<(), &'static str> {
    let parsed = Uuid::parse_str(value).map_err(|_| "must be a UUID string")?;
    if parsed.hyphenated().to_string() != value.to_ascii_lowercase() {
        return Err("must be a UUID string");
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandValidationError {
    InvalidVersion,
    InvalidCommandKind,
    InvalidCommandPayload,
    InvalidSessionId,
}

impl CommandValidationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidVersion => "invalid_protocol_version",
            Self::InvalidCommandKind => "invalid_command_kind",
            Self::InvalidCommandPayload => "invalid_command_payload",
            Self::InvalidSessionId => "invalid_session_id",
        }
    }
}

pub fn validate_command(command: &Envelope) -> Result<(), CommandValidationError> {
    if command.version != PROTOCOL_VERSION {
        return Err(CommandValidationError::InvalidVersion);
    }
    if validate_uuid(&command.id).is_err()
        || command
            .correlation_id
            .as_deref()
            .is_some_and(|value| validate_uuid(value).is_err())
    {
        return Err(CommandValidationError::InvalidSessionId);
    }

    let ProtocolKind::Command(kind) = &command.kind else {
        return Err(CommandValidationError::InvalidCommandKind);
    };

    let requires_session = !matches!(
        kind,
        CommandKind::HandshakeRequest | CommandKind::SessionSnapshotRequest
    );
    if requires_session && command.session_id.is_none()
        || command
            .session_id
            .as_deref()
            .is_some_and(|value| validate_uuid(value).is_err())
    {
        return Err(CommandValidationError::InvalidSessionId);
    }

    let payload = &command.payload;
    let valid = match kind {
        CommandKind::HandshakeRequest
        | CommandKind::SessionStop
        | CommandKind::SessionSnapshotRequest => payload.is_empty(),
        CommandKind::SessionStart => {
            has_exact_keys(
                payload,
                &[
                    "mode",
                    "input_language",
                    "response_language",
                    "review_language",
                    "you_source",
                    "brief_id",
                ],
            ) && required_string(payload, "mode")
                && required_string(payload, "input_language")
                && required_string(payload, "response_language")
                && required_string(payload, "review_language")
                && required_source(payload, "you_source")
                && required_string(payload, "brief_id")
        }
        CommandKind::ListeningSet | CommandKind::AudioSystemSet => {
            has_exact_keys(payload, &["enabled"])
                && payload.get("enabled").is_some_and(Value::is_boolean)
        }
        CommandKind::YouSourceSet => {
            has_exact_keys(payload, &["source"]) && required_source(payload, "source")
        }
        CommandKind::QueryTrigger => {
            has_exact_keys(payload, &["text", "answer_format"])
                && required_string(payload, "text")
                && required_string(payload, "answer_format")
        }
        CommandKind::AudioDeviceSet => {
            has_exact_keys(payload, &["source", "device"])
                && required_source(payload, "source")
                && payload
                    .get("device")
                    .is_some_and(|value| value.is_null() || value.as_i64().is_some())
        }
        CommandKind::KnowledgeIngest => {
            has_exact_keys(payload, &["paths"])
                && payload
                    .get("paths")
                    .and_then(Value::as_array)
                    .is_some_and(|paths| paths.iter().all(Value::is_string))
        }
    };

    valid
        .then_some(())
        .ok_or(CommandValidationError::InvalidCommandPayload)
}

pub fn validate_event(event: &Envelope) -> Result<(), CommandValidationError> {
    if event.version != PROTOCOL_VERSION {
        return Err(CommandValidationError::InvalidVersion);
    }
    if validate_uuid(&event.id).is_err()
        || event
            .session_id
            .as_deref()
            .is_some_and(|value| validate_uuid(value).is_err())
        || event
            .correlation_id
            .as_deref()
            .is_some_and(|value| validate_uuid(value).is_err())
    {
        return Err(CommandValidationError::InvalidSessionId);
    }
    let ProtocolKind::Event(kind) = &event.kind else {
        return Err(CommandValidationError::InvalidCommandKind);
    };

    let valid = match kind {
        EventKind::SidecarReady => {
            event.session_id.is_none()
                && has_exact_keys(&event.payload, &["status"])
                && event.payload.get("status") == Some(&Value::String("ready".into()))
        }
        EventKind::SessionState => {
            event.session_id.is_some()
                && has_exact_keys(
                    &event.payload,
                    &[
                        "state",
                        "mode",
                        "input_language",
                        "response_language",
                        "review_language",
                        "you_source",
                        "listening",
                        "system_audio_enabled",
                    ],
                )
                && required_bounded_string(&event.payload, "state")
                && required_bounded_string(&event.payload, "mode")
                && required_bounded_string(&event.payload, "input_language")
                && required_bounded_string(&event.payload, "response_language")
                && required_bounded_string(&event.payload, "review_language")
                && required_source(&event.payload, "you_source")
                && event
                    .payload
                    .get("listening")
                    .is_some_and(Value::is_boolean)
                && event
                    .payload
                    .get("system_audio_enabled")
                    .is_some_and(Value::is_boolean)
        }
        EventKind::TranscriptUpdated => {
            event.session_id.is_some()
                && has_exact_keys(
                    &event.payload,
                    &[
                        "turn_id",
                        "text",
                        "is_final",
                        "speech_final",
                        "speaker_role",
                        "source",
                        "language",
                        "confidence",
                        "started_at_ms",
                        "ended_at_ms",
                    ],
                )
                && required_uuid_value(&event.payload, "turn_id")
                && required_bounded_string(&event.payload, "text")
                && event.payload.get("is_final").is_some_and(Value::is_boolean)
                && event
                    .payload
                    .get("speech_final")
                    .is_some_and(Value::is_boolean)
                && matches!(
                    event.payload.get("speaker_role").and_then(Value::as_str),
                    Some("interviewee" | "interviewer")
                )
                && required_source(&event.payload, "source")
                && required_bounded_string(&event.payload, "language")
                && event
                    .payload
                    .get("confidence")
                    .and_then(Value::as_f64)
                    .is_some_and(|value| (0.0..=1.0).contains(&value))
                && required_safe_integer(&event.payload, "started_at_ms")
                && required_safe_integer(&event.payload, "ended_at_ms")
        }
        EventKind::SuggestionChunk | EventKind::SuggestionCompleted => {
            event.session_id.is_some()
                && has_exact_keys(&event.payload, &["suggestion_id", "text"])
                && required_uuid_value(&event.payload, "suggestion_id")
                && required_bounded_string(&event.payload, "text")
        }
        EventKind::AudioHealth => {
            has_exact_keys(&event.payload, &["source", "status", "message"])
                && required_source(&event.payload, "source")
                && required_bounded_string(&event.payload, "status")
                && optional_bounded_string(&event.payload, "message")
        }
        EventKind::ProviderHealth => {
            has_exact_keys(&event.payload, &["provider", "status", "message"])
                && required_bounded_string(&event.payload, "provider")
                && required_bounded_string(&event.payload, "status")
                && optional_bounded_string(&event.payload, "message")
        }
        EventKind::KnowledgeState => {
            event.session_id.is_some()
                && has_exact_keys(
                    &event.payload,
                    &["state", "chunks", "files", "chunks_added"],
                )
                && required_bounded_string(&event.payload, "state")
                && required_safe_integer(&event.payload, "chunks")
                && required_safe_integer(&event.payload, "files")
                && required_safe_integer(&event.payload, "chunks_added")
        }
        EventKind::RuntimeError => {
            has_exact_keys(
                &event.payload,
                &["code", "message", "recoverable", "source"],
            ) && required_bounded_string(&event.payload, "code")
                && required_bounded_string(&event.payload, "message")
                && event
                    .payload
                    .get("recoverable")
                    .is_some_and(Value::is_boolean)
                && required_bounded_string(&event.payload, "source")
        }
    };
    if !valid {
        return Err(CommandValidationError::InvalidCommandPayload);
    }
    Ok(())
}

fn has_exact_keys(payload: &Map<String, Value>, expected: &[&str]) -> bool {
    payload.len() == expected.len() && expected.iter().all(|key| payload.contains_key(*key))
}

fn required_string(payload: &Map<String, Value>, key: &str) -> bool {
    payload
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

fn required_source(payload: &Map<String, Value>, key: &str) -> bool {
    matches!(
        payload.get(key).and_then(Value::as_str),
        Some("mic" | "system")
    )
}

const MAX_EVENT_STRING_BYTES: usize = 64 * 1024;

fn required_bounded_string(payload: &Map<String, Value>, key: &str) -> bool {
    payload
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty() && value.len() <= MAX_EVENT_STRING_BYTES)
}

fn optional_bounded_string(payload: &Map<String, Value>, key: &str) -> bool {
    payload.get(key).is_some_and(|value| {
        value.is_null()
            || value
                .as_str()
                .is_some_and(|text| text.len() <= MAX_EVENT_STRING_BYTES)
    })
}

fn required_uuid_value(payload: &Map<String, Value>, key: &str) -> bool {
    payload
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| validate_uuid(value).is_ok())
}

fn required_safe_integer(payload: &Map<String, Value>, key: &str) -> bool {
    payload
        .get(key)
        .and_then(Value::as_u64)
        .is_some_and(|value| value <= MAX_SAFE_INTEGER)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        encode_frame, Envelope, EventKind, FrameDecoder, ProtocolKind, MAX_FRAME_BYTES,
        MAX_SAFE_INTEGER, PROTOCOL_VERSION,
    };

    fn enum_values(source: &str, enum_name: &str) -> BTreeSet<String> {
        source
            .split_once(enum_name)
            .expect("enum must exist")
            .1
            .split_once('}')
            .expect("enum must close")
            .0
            .lines()
            .filter_map(|line| {
                let value = line.split_once("= \"")?.1.split_once('"')?.0;
                value.contains('.').then(|| value.to_owned())
            })
            .collect()
    }

    #[test]
    fn canonical_event_kinds_match_across_protocol_surfaces() {
        let expected = BTreeSet::from([
            "sidecar.ready".to_owned(),
            "session.state".to_owned(),
            "transcript.updated".to_owned(),
            "suggestion.chunk".to_owned(),
            "suggestion.completed".to_owned(),
            "audio.health".to_owned(),
            "provider.health".to_owned(),
            "knowledge.state".to_owned(),
            "runtime.error".to_owned(),
        ]);
        let typescript = enum_values(
            include_str!("../../src/shared/protocol.ts"),
            "export enum EventKind",
        );
        let python = enum_values(
            include_str!("../../../ai_assistant/sidecar/protocol.py"),
            "class EventKind",
        );
        let command_kinds = enum_values(
            include_str!("../../src/shared/protocol.ts"),
            "export enum CommandKind",
        );
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../protocol/v1/envelope.schema.json"))
                .unwrap();
        let schema_kinds = schema["properties"]["kind"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        let schema_events = schema_kinds
            .difference(&command_kinds)
            .cloned()
            .collect::<BTreeSet<_>>();
        let rust = [
            EventKind::SidecarReady,
            EventKind::SessionState,
            EventKind::TranscriptUpdated,
            EventKind::SuggestionChunk,
            EventKind::SuggestionCompleted,
            EventKind::AudioHealth,
            EventKind::ProviderHealth,
            EventKind::KnowledgeState,
            EventKind::RuntimeError,
        ]
        .into_iter()
        .map(|kind| ProtocolKind::from(kind).as_str().to_owned())
        .collect::<BTreeSet<_>>();

        assert_eq!(typescript, expected);
        assert_eq!(python, expected);
        assert_eq!(schema_events, expected);
        assert_eq!(rust, expected);
        assert_eq!(
            schema_kinds,
            command_kinds.union(&expected).cloned().collect()
        );
    }

    #[test]
    fn transcript_fixture_round_trips() {
        let raw = include_str!("../../../protocol/v1/fixtures/transcript-final.json");
        let value: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(value.version, PROTOCOL_VERSION);
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::from_str::<serde_json::Value>(raw).unwrap()
        );
    }

    #[test]
    fn event_fixture_kind_is_known() {
        let raw = include_str!("../../../protocol/v1/fixtures/sidecar-ready.json");
        let value: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(value.kind, EventKind::SidecarReady.into());
    }

    #[test]
    fn unknown_payload_fields_round_trip() {
        let raw = r#"{
            "version": 1,
            "id": "018f0000-0000-7000-8000-000000000001",
            "session_id": null,
            "sequence": 0,
            "timestamp_ms": 0,
            "kind": "sidecar.ready",
            "payload": {"future_extension": true},
            "correlation_id": null
        }"#;
        let value: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::from_str::<serde_json::Value>(raw).unwrap()
        );
    }

    #[test]
    fn unsupported_protocol_version_is_rejected() {
        let raw = include_str!("../../../protocol/v1/fixtures/sidecar-ready.json")
            .replace("\"version\": 1", "\"version\": 2");
        assert!(serde_json::from_str::<Envelope>(&raw).is_err());
    }

    #[test]
    fn safe_integer_boundary_round_trips() {
        let mut raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../protocol/v1/fixtures/sidecar-ready.json"
        ))
        .unwrap();
        raw["sequence"] = serde_json::json!(MAX_SAFE_INTEGER);
        raw["timestamp_ms"] = serde_json::json!(MAX_SAFE_INTEGER);

        let value: Envelope = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(value).unwrap(), raw);
    }

    #[test]
    fn safe_integer_overflow_is_rejected() {
        let mut raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../protocol/v1/fixtures/sidecar-ready.json"
        ))
        .unwrap();
        raw["sequence"] = serde_json::json!(MAX_SAFE_INTEGER + 1);
        raw["timestamp_ms"] = serde_json::json!(MAX_SAFE_INTEGER + 1);

        assert!(serde_json::from_value::<Envelope>(raw).is_err());
    }

    #[test]
    fn decoder_rejects_a_frame_larger_than_the_protocol_limit() {
        let error = FrameDecoder::new(MAX_FRAME_BYTES)
            .push(&((MAX_FRAME_BYTES as u32 + 1).to_be_bytes()))
            .unwrap_err();

        assert_eq!(error.code(), "frame_too_large");
    }

    #[test]
    fn decoder_rejects_a_second_messagepack_value_inside_one_frame() {
        let envelope: Envelope = serde_json::from_str(include_str!(
            "../../../protocol/v1/fixtures/sidecar-ready.json"
        ))
        .unwrap();
        let mut frame = encode_frame(&envelope).unwrap();
        frame.extend_from_slice(&[0xc0]);
        let payload_size = u32::try_from(frame.len() - 4).unwrap();
        frame[..4].copy_from_slice(&payload_size.to_be_bytes());

        assert_eq!(
            FrameDecoder::new(MAX_FRAME_BYTES)
                .push(&frame)
                .unwrap_err()
                .code(),
            "invalid_frame"
        );
    }
}
