use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

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

#[cfg(test)]
mod tests {
    use super::{Envelope, EventKind, MAX_SAFE_INTEGER, PROTOCOL_VERSION};

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
}
