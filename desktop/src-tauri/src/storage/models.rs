use std::fmt;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineEventKind {
    TranscriptPartial,
    TranscriptFinal,
    SuggestionChunk,
    SuggestionCompleted,
    SessionState,
    Note,
    Pin,
}

impl TimelineEventKind {
    pub fn is_durable(&self) -> bool {
        !matches!(self, Self::TranscriptPartial | Self::SuggestionChunk)
    }

    pub(crate) fn as_db(&self) -> &'static str {
        match self {
            Self::TranscriptPartial => "transcript.partial",
            Self::TranscriptFinal => "transcript.final",
            Self::SuggestionChunk => "suggestion.chunk",
            Self::SuggestionCompleted => "suggestion.completed",
            Self::SessionState => "session.state",
            Self::Note => "note",
            Self::Pin => "pin",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, ModelError> {
        match value {
            "transcript.partial" => Ok(Self::TranscriptPartial),
            "transcript.final" => Ok(Self::TranscriptFinal),
            "suggestion.chunk" => Ok(Self::SuggestionChunk),
            "suggestion.completed" => Ok(Self::SuggestionCompleted),
            "session.state" => Ok(Self::SessionState),
            "note" => Ok(Self::Note),
            "pin" => Ok(Self::Pin),
            _ => Err(ModelError::UnknownTimelineEventKind),
        }
    }
}

impl fmt::Display for TimelineEventKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_db())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModelError {
    #[error("workspace id is required")]
    MissingWorkspaceId,
    #[error("session id is required")]
    MissingSessionId,
    #[error("session language is required")]
    MissingLanguage,
    #[error("session mode is required")]
    MissingMode,
    #[error("active sessions cannot have a completion timestamp")]
    ActiveSessionHasCompletionTimestamp,
    #[error("completed or interrupted sessions require a completion timestamp")]
    FinishedSessionMissingCompletionTimestamp,
    #[error("event id is required")]
    MissingEventId,
    #[error("a final transcript requires a turn id")]
    MissingTurnId,
    #[error("a completed suggestion requires request and turn ids")]
    MissingSuggestionAssociation,
    #[error("request id is required")]
    MissingRequestId,
    #[error("session brief must be a JSON object")]
    BriefMustBeObject,
    #[error("unknown persisted timeline event kind")]
    UnknownTimelineEventKind,
    #[error("unknown persisted session status")]
    UnknownSessionStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Completed,
    Interrupted,
}

impl SessionStatus {
    pub(crate) fn as_db(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, ModelError> {
        match value {
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(ModelError::UnknownSessionStatus),
        }
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_db())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSession {
    pub workspace_id: String,
    pub session_id: String,
    pub mode: String,
    pub status: SessionStatus,
    pub ui_language: Option<String>,
    pub input_language: String,
    pub response_language: String,
    pub review_language: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

impl NewSession {
    pub fn validate(&self) -> Result<(), ModelError> {
        validate_ownership(&self.workspace_id, &self.session_id)?;
        if self.mode.is_empty() {
            return Err(ModelError::MissingMode);
        }
        if self.input_language.is_empty()
            || self.response_language.is_empty()
            || self.review_language.is_empty()
            || self.ui_language.as_deref().is_some_and(str::is_empty)
        {
            return Err(ModelError::MissingLanguage);
        }
        match (&self.status, self.completed_at_ms) {
            (SessionStatus::Active, Some(_)) => {
                Err(ModelError::ActiveSessionHasCompletionTimestamp)
            }
            (SessionStatus::Completed | SessionStatus::Interrupted, None) => {
                Err(ModelError::FinishedSessionMissingCompletionTimestamp)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSession {
    pub workspace_id: String,
    pub session_id: String,
    pub mode: String,
    pub status: SessionStatus,
    pub ui_language: Option<String>,
    pub input_language: String,
    pub response_language: String,
    pub review_language: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub brief: Option<StoredSessionBrief>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSessionBrief {
    pub workspace_id: String,
    pub session_id: String,
    pub brief: Value,
    pub updated_at_ms: i64,
}

impl NewSessionBrief {
    pub fn validate(&self) -> Result<(), ModelError> {
        validate_ownership(&self.workspace_id, &self.session_id)?;
        if !self.brief.is_object() {
            return Err(ModelError::BriefMustBeObject);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSessionBrief {
    pub brief: Value,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestTurnAssociation {
    pub workspace_id: String,
    pub session_id: String,
    pub request_id: String,
    pub turn_id: String,
    pub created_at_ms: i64,
}

impl RequestTurnAssociation {
    pub fn validate(&self) -> Result<(), ModelError> {
        validate_ownership(&self.workspace_id, &self.session_id)?;
        if self.request_id.is_empty() {
            return Err(ModelError::MissingRequestId);
        }
        if self.turn_id.is_empty() {
            return Err(ModelError::MissingTurnId);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssociateRequestResult {
    Inserted,
    Duplicate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewTimelineEvent {
    pub workspace_id: String,
    pub session_id: String,
    pub event_id: String,
    pub source_generation: i64,
    pub source_sequence: i64,
    pub timestamp_ms: i64,
    pub kind: TimelineEventKind,
    pub correlation_id: Option<String>,
    pub request_id: Option<String>,
    pub turn_id: Option<String>,
    pub payload: Value,
}

impl NewTimelineEvent {
    pub fn validate(&self) -> Result<(), ModelError> {
        validate_ownership(&self.workspace_id, &self.session_id)?;
        if self.event_id.is_empty() {
            return Err(ModelError::MissingEventId);
        }
        match self.kind {
            TimelineEventKind::TranscriptFinal
                if self.turn_id.as_deref().unwrap_or_default().is_empty() =>
            {
                Err(ModelError::MissingTurnId)
            }
            TimelineEventKind::SuggestionCompleted
                if self.request_id.as_deref().unwrap_or_default().is_empty()
                    || self.turn_id.as_deref().unwrap_or_default().is_empty() =>
            {
                Err(ModelError::MissingSuggestionAssociation)
            }
            _ => Ok(()),
        }
    }
}

fn validate_ownership(workspace_id: &str, session_id: &str) -> Result<(), ModelError> {
    if workspace_id.is_empty() {
        return Err(ModelError::MissingWorkspaceId);
    }
    if session_id.is_empty() {
        return Err(ModelError::MissingSessionId);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredTimelineEvent {
    pub workspace_id: String,
    pub session_id: String,
    pub event_id: String,
    pub host_sequence: i64,
    pub source_generation: i64,
    pub source_sequence: i64,
    pub timestamp_ms: i64,
    pub kind: TimelineEventKind,
    pub correlation_id: Option<String>,
    pub request_id: Option<String>,
    pub turn_id: Option<String>,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppendEventResult {
    Inserted { host_sequence: i64 },
    Duplicate { host_sequence: i64 },
}

#[cfg(test)]
mod tests {
    use super::{
        ModelError, NewSessionBrief, NewTimelineEvent, StoredSession, StoredSessionBrief,
        TimelineEventKind,
    };
    use serde_json::json;

    fn event(kind: TimelineEventKind) -> NewTimelineEvent {
        NewTimelineEvent {
            workspace_id: "workspace-a".into(),
            session_id: "session-a".into(),
            event_id: "event-a".into(),
            source_generation: 1,
            source_sequence: 7,
            timestamp_ms: 1_700_000_000_000,
            kind,
            correlation_id: None,
            request_id: None,
            turn_id: None,
            payload: json!({"text": "marker"}),
        }
    }
    #[test]
    fn partials_and_chunks_are_representable_but_not_durable() {
        assert!(!TimelineEventKind::TranscriptPartial.is_durable());
        assert!(!TimelineEventKind::SuggestionChunk.is_durable());
        assert!(TimelineEventKind::TranscriptFinal.is_durable());
        assert!(TimelineEventKind::SuggestionCompleted.is_durable());
        assert!(TimelineEventKind::SessionState.is_durable());
        assert!(TimelineEventKind::Note.is_durable());
        assert!(TimelineEventKind::Pin.is_durable());
    }
    #[test]
    fn completed_suggestion_requires_durable_request_to_turn_association() {
        let mut suggestion = event(TimelineEventKind::SuggestionCompleted);
        assert_eq!(
            suggestion.validate(),
            Err(ModelError::MissingSuggestionAssociation)
        );
        suggestion.request_id = Some("request-a".into());
        suggestion.turn_id = Some("turn-a".into());
        assert_eq!(suggestion.validate(), Ok(()));
    }
    #[test]
    fn final_transcript_requires_a_turn_id() {
        let transcript = event(TimelineEventKind::TranscriptFinal);
        assert_eq!(transcript.validate(), Err(ModelError::MissingTurnId));
    }
    #[test]
    fn empty_ownership_identifiers_are_rejected() {
        let mut transcript = event(TimelineEventKind::TranscriptFinal);
        transcript.turn_id = Some("turn-a".into());
        transcript.workspace_id.clear();
        assert_eq!(transcript.validate(), Err(ModelError::MissingWorkspaceId));
    }

    #[test]
    fn session_briefs_require_json_objects() {
        let brief = NewSessionBrief {
            workspace_id: "workspace-a".into(),
            session_id: "session-a".into(),
            brief: json!("legacy summary"),
            updated_at_ms: 1,
        };
        assert_eq!(brief.validate(), Err(ModelError::BriefMustBeObject));
    }

    #[test]
    fn stored_sessions_are_equatable_when_they_include_a_brief() {
        fn requires_eq<T: Eq>() {}

        requires_eq::<StoredSession>();
        assert_eq!(
            StoredSessionBrief {
                brief: json!({"title": "stored"}),
                updated_at_ms: 1,
            },
            StoredSessionBrief {
                brief: json!({"title": "stored"}),
                updated_at_ms: 1,
            }
        );
    }
}
