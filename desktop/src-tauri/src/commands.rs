use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::State;

use crate::capture_protection::{CaptureProtectionController, CaptureProtectionStatus};
use crate::protocol::{
    is_supported_session_mode, validate_command, CommandKind, Envelope, EventKind, ProtocolKind,
    PROTOCOL_VERSION,
};
use crate::sidecar::{SidecarError, SidecarStatus, StorageHealth};
use crate::state::AppState;
use crate::storage::{
    NewSession, NewSessionBrief, QueryDispatchAuthorization, RepositoryError,
    RequestTurnAssociation, SessionRepository, SessionStatus, StoredSession, StoredTimelineEvent,
    TimelineEventKind,
};

const MAX_LANGUAGE_TAG_BYTES: usize = 63;
const MAX_BRIEF_BYTES: usize = 256 * 1024;
const REPOSITORY_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

pub type SidecarCommandError = SidecarError;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCommandError {
    code: &'static str,
    message: &'static str,
}

impl StorageCommandError {
    fn invalid(message: &'static str) -> Self {
        Self {
            code: "invalid_storage_command",
            message,
        }
    }

    fn from_repository(error: RepositoryError) -> Self {
        match error {
            RepositoryError::NotFound => Self {
                code: "session_not_found",
                message: "The requested session was not found.",
            },
            RepositoryError::InvalidSessionLimit { .. } => {
                Self::invalid("Session list limit must be between 1 and 100.")
            }
            RepositoryError::InvalidCompletionStatus => {
                Self::invalid("Session completion status is invalid.")
            }
            RepositoryError::RequestTurnAssociationConflict => Self {
                code: "request_turn_conflict",
                message: "The request is already associated with a different turn.",
            },
            RepositoryError::SessionCompletionConflict => Self {
                code: "session_completion_conflict",
                message: "The session already has a different completion status.",
            },
            _ => Self {
                code: "storage_unavailable",
                message: "Encrypted session storage is unavailable.",
            },
        }
    }

    fn task_failed() -> Self {
        Self {
            code: "storage_task_failed",
            message: "Encrypted session storage could not complete the request.",
        }
    }

    fn timed_out() -> Self {
        Self {
            code: "storage_timeout",
            message: "Encrypted session storage timed out.",
        }
    }
}

impl std::fmt::Display for StorageCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for StorageCommandError {}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionInput {
    mode: String,
    ui_language: Option<String>,
    input_language: String,
    response_language: String,
    review_language: String,
}

impl CreateSessionInput {
    fn validate(&self) -> Result<(), StorageCommandError> {
        validate_mode(&self.mode)?;
        if let Some(ui_language) = self.ui_language.as_deref() {
            validate_language_tag(ui_language, false)?;
        }
        validate_language_tag(&self.input_language, true)?;
        validate_language_tag(&self.response_language, false)?;
        validate_language_tag(&self.review_language, false)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveSessionBriefInput {
    session_id: String,
    brief: Map<String, Value>,
}

impl SaveSessionBriefInput {
    fn validate(&self) -> Result<(), StorageCommandError> {
        validate_uuid(&self.session_id)?;
        let encoded = serde_json::to_vec(&self.brief)
            .map_err(|_| StorageCommandError::invalid("Session brief is invalid."))?;
        if encoded.len() > MAX_BRIEF_BYTES {
            return Err(StorageCommandError::invalid("Session brief is too large."));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatusInput {
    Completed,
    Interrupted,
}

impl From<CompletionStatusInput> for SessionStatus {
    fn from(value: CompletionStatusInput) -> Self {
        match value {
            CompletionStatusInput::Completed => Self::Completed,
            CompletionStatusInput::Interrupted => Self::Interrupted,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssociateRequestWithTurnInput {
    session_id: String,
    request_id: String,
    turn_id: String,
}

impl AssociateRequestWithTurnInput {
    fn validate(&self) -> Result<(), StorageCommandError> {
        validate_uuid(&self.session_id)?;
        validate_uuid(&self.request_id)?;
        validate_uuid(&self.turn_id)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionRecordDto {
    id: String,
    mode: String,
    status: SessionStatusDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    ui_language: Option<String>,
    input_language: String,
    response_language: String,
    review_language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    brief: Option<Value>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum SessionStatusDto {
    Active,
    Completed,
    Interrupted,
}

impl From<SessionStatus> for SessionStatusDto {
    fn from(value: SessionStatus) -> Self {
        match value {
            SessionStatus::Active => Self::Active,
            SessionStatus::Completed => Self::Completed,
            SessionStatus::Interrupted => Self::Interrupted,
        }
    }
}

impl From<StoredSession> for SessionRecordDto {
    fn from(session: StoredSession) -> Self {
        Self {
            id: session.session_id,
            mode: session.mode,
            status: session.status.into(),
            ui_language: session.ui_language,
            input_language: session.input_language,
            response_language: session.response_language,
            review_language: session.review_language,
            brief: session.brief.map(|brief| brief.brief),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RequestTurnAssociationDto {
    request_id: String,
    turn_id: String,
}

enum DurableAuthorizationRequest {
    Query {
        workspace_id: String,
        session_id: String,
        request_id: String,
    },
    SessionStart {
        workspace_id: String,
        session_id: String,
    },
}

enum DurableAuthorization {
    Query(QueryDispatchAuthorization),
    SessionStart(Option<SessionStatus>),
}

#[tauri::command]
pub async fn sidecar_status(
    state: State<'_, AppState>,
) -> Result<SidecarStatus, SidecarCommandError> {
    Ok(state.sidecar.status().await)
}

#[tauri::command]
pub async fn storage_health(
    state: State<'_, AppState>,
) -> Result<StorageHealth, SidecarCommandError> {
    state.storage_health.current()
}

#[tauri::command]
pub fn capture_protection_status(state: State<'_, AppState>) -> CaptureProtectionStatus {
    state.capture_protection.status()
}

#[tauri::command]
pub async fn retry_capture_protection(
    state: State<'_, AppState>,
) -> Result<CaptureProtectionStatus, SidecarCommandError> {
    Ok(state.capture_protection.reapply_and_verify_async().await)
}

#[tauri::command]
pub async fn send_sidecar_command(
    state: State<'_, AppState>,
    command: Envelope,
) -> Result<(), SidecarCommandError> {
    let repository = state.repository.clone();
    let workspace_id = state.workspace_id.clone();
    let sidecar = state.sidecar.clone();
    let session_operation_gate = state.session_operation_gate.clone();
    let capture_protection = state.capture_protection.clone();
    let protection_command = command.clone();

    dispatch_with_capture_protection(
        &protection_command,
        capture_protection.as_ref(),
        move || async move {
            let authorization_command = command.clone();
            let authorization = ensure_sidecar_command_authorized(
                authorization_command,
                workspace_id,
                move |request| match request {
                    DurableAuthorizationRequest::Query {
                        workspace_id,
                        session_id,
                        request_id,
                    } => repository
                        .authorize_query_dispatch(&workspace_id, &session_id, &request_id)
                        .map(DurableAuthorization::Query),
                    DurableAuthorizationRequest::SessionStart {
                        workspace_id,
                        session_id,
                    } => repository
                        .get_session(&workspace_id, &session_id)
                        .map(|session| {
                            DurableAuthorization::SessionStart(
                                session.map(|session| session.status),
                            )
                        }),
                },
            );
            sidecar
                .send_with_authorization_and_gate(command, session_operation_gate, authorization)
                .await
        },
    )
    .await
}

fn command_requires_capture_protection(command: &Envelope) -> bool {
    matches!(
        &command.kind,
        ProtocolKind::Command(CommandKind::SessionStart | CommandKind::QueryTrigger)
    ) || matches!(
        &command.kind,
        ProtocolKind::Command(CommandKind::ListeningSet | CommandKind::AudioSystemSet)
    ) && command.payload.get("enabled").and_then(Value::as_bool) == Some(true)
}

async fn dispatch_with_capture_protection<F, Fut>(
    command: &Envelope,
    capture_protection: &CaptureProtectionController,
    dispatch: F,
) -> Result<(), SidecarError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), SidecarError>>,
{
    let _protection_lease = if command_requires_capture_protection(command) {
        Some(capture_protection.acquire_protected_lease().await?)
    } else {
        None
    };
    dispatch().await
}

async fn ensure_sidecar_command_authorized<F>(
    command: Envelope,
    workspace_id: String,
    authorize: F,
) -> Result<(), SidecarError>
where
    F: FnOnce(DurableAuthorizationRequest) -> Result<DurableAuthorization, RepositoryError>
        + Send
        + 'static,
{
    validate_command(&command)
        .map_err(|error| SidecarError::new(error.code(), "sidecar command validation failed"))?;
    let (request, query_authorization) = match &command.kind {
        ProtocolKind::Command(CommandKind::QueryTrigger) => (
            DurableAuthorizationRequest::Query {
                workspace_id,
                session_id: command.session_id.clone().ok_or_else(|| {
                    SidecarError::new("invalid_session_id", "sidecar command validation failed")
                })?,
                // The sidecar echoes the query command ID as suggestion correlation_id.
                request_id: command.id.clone(),
            },
            true,
        ),
        ProtocolKind::Command(CommandKind::SessionStart) => (
            DurableAuthorizationRequest::SessionStart {
                workspace_id,
                session_id: command.session_id.clone().ok_or_else(|| {
                    SidecarError::new("invalid_session_id", "sidecar command validation failed")
                })?,
            },
            false,
        ),
        _ => return Ok(()),
    };
    let unavailable = || {
        if query_authorization {
            SidecarError::new(
                "query_association_unavailable",
                "The durable query context could not be verified.",
            )
        } else {
            SidecarError::new(
                "session_start_authorization_unavailable",
                "The durable session could not be verified.",
            )
        }
    };
    let authorization = tauri::async_runtime::spawn_blocking(move || authorize(request))
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?;

    match authorization {
        DurableAuthorization::Query(QueryDispatchAuthorization::Authorized) => Ok(()),
        DurableAuthorization::Query(QueryDispatchAuthorization::SessionMissing)
        | DurableAuthorization::Query(QueryDispatchAuthorization::SessionInactive) => {
            Err(SidecarError::new(
                "query_session_stale",
                "The durable session is no longer active.",
            ))
        }
        DurableAuthorization::Query(QueryDispatchAuthorization::AssociationMissing) => {
            Err(SidecarError::new(
                "query_association_missing",
                "A durable request association is required before query dispatch.",
            ))
        }
        DurableAuthorization::Query(QueryDispatchAuthorization::TranscriptMissing) => {
            Err(SidecarError::new(
                "query_turn_not_durable",
                "The associated transcript turn is not durably available.",
            ))
        }
        DurableAuthorization::SessionStart(Some(SessionStatus::Active)) => Ok(()),
        DurableAuthorization::SessionStart(None)
        | DurableAuthorization::SessionStart(Some(
            SessionStatus::Completed | SessionStatus::Interrupted,
        )) => Err(SidecarError::new(
            "session_start_not_active",
            "The durable session is missing or no longer active.",
        )),
        _ => Err(unavailable()),
    }
}

#[tauri::command]
pub async fn restart_sidecar(
    state: State<'_, AppState>,
) -> Result<SidecarStatus, SidecarCommandError> {
    state.sidecar.restart().await
}

#[tauri::command]
pub async fn create_session(
    state: State<'_, AppState>,
    input: CreateSessionInput,
) -> Result<SessionRecordDto, StorageCommandError> {
    input.validate()?;
    let session_id = uuid::Uuid::new_v4().hyphenated().to_string();
    let session = NewSession {
        workspace_id: state.workspace_id.clone(),
        session_id,
        mode: input.mode,
        status: SessionStatus::Active,
        ui_language: input.ui_language,
        input_language: input.input_language,
        response_language: input.response_language,
        review_language: input.review_language,
        started_at_ms: now_ms()?,
        completed_at_ms: None,
    };
    let repository = state.repository.clone();
    let returned = session.clone();
    run_repository(repository, move |repository| {
        repository.create_session(&session)?;
        Ok(StoredSession {
            workspace_id: returned.workspace_id,
            session_id: returned.session_id,
            mode: returned.mode,
            status: returned.status,
            ui_language: returned.ui_language,
            input_language: returned.input_language,
            response_language: returned.response_language,
            review_language: returned.review_language,
            started_at_ms: returned.started_at_ms,
            completed_at_ms: None,
            brief: None,
        })
    })
    .await
    .map(Into::into)
}

#[tauri::command]
pub async fn save_session_brief(
    state: State<'_, AppState>,
    input: SaveSessionBriefInput,
) -> Result<(), StorageCommandError> {
    input.validate()?;
    let brief = NewSessionBrief {
        workspace_id: state.workspace_id.clone(),
        session_id: input.session_id,
        brief: Value::Object(input.brief),
        updated_at_ms: now_ms()?,
    };
    run_repository(state.repository.clone(), move |repository| {
        repository.save_session_brief(&brief)
    })
    .await
}

#[tauri::command]
pub async fn complete_session(
    state: State<'_, AppState>,
    session_id: String,
    status: CompletionStatusInput,
) -> Result<(), StorageCommandError> {
    validate_uuid(&session_id)?;
    let _session_operation = state.session_operation_gate.lock().await;
    let workspace_id = state.workspace_id.clone();
    let completed_at_ms = now_ms()?;
    run_repository(state.repository.clone(), move |repository| {
        if repository.complete_session(
            &workspace_id,
            &session_id,
            status.into(),
            completed_at_ms,
        )? {
            Ok(())
        } else {
            Err(RepositoryError::NotFound)
        }
    })
    .await
}

#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
    limit: usize,
) -> Result<Vec<SessionRecordDto>, StorageCommandError> {
    if !(1..=100).contains(&limit) {
        return Err(StorageCommandError::invalid(
            "Session list limit must be between 1 and 100.",
        ));
    }
    let workspace_id = state.workspace_id.clone();
    run_repository(state.repository.clone(), move |repository| {
        repository.list_sessions(&workspace_id, limit)
    })
    .await
    .map(|sessions| sessions.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn get_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionRecordDto, StorageCommandError> {
    validate_uuid(&session_id)?;
    let workspace_id = state.workspace_id.clone();
    run_repository(state.repository.clone(), move |repository| {
        repository
            .get_session(&workspace_id, &session_id)?
            .ok_or(RepositoryError::NotFound)
    })
    .await
    .map(Into::into)
}

#[tauri::command]
pub async fn get_timeline(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<Envelope>, StorageCommandError> {
    validate_uuid(&session_id)?;
    let workspace_id = state.workspace_id.clone();
    let timeline = run_repository(state.repository.clone(), move |repository| {
        if repository
            .get_session(&workspace_id, &session_id)?
            .is_none()
        {
            return Err(RepositoryError::NotFound);
        }
        repository.get_timeline(&workspace_id, &session_id)
    })
    .await?;
    timeline
        .into_iter()
        .filter_map(|event| match timeline_envelope(event) {
            Ok(Some(envelope)) => Some(Ok(envelope)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

#[tauri::command]
pub async fn restore_active_session(
    state: State<'_, AppState>,
) -> Result<Option<SessionRecordDto>, StorageCommandError> {
    let workspace_id = state.workspace_id.clone();
    run_repository(state.repository.clone(), move |repository| {
        repository.restore_active_session(&workspace_id)
    })
    .await
    .map(|session| session.map(Into::into))
}

#[tauri::command]
pub async fn delete_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), StorageCommandError> {
    validate_uuid(&session_id)?;
    let _session_operation = state.session_operation_gate.lock().await;
    let workspace_id = state.workspace_id.clone();
    run_repository(state.repository.clone(), move |repository| {
        if repository.delete_session(&workspace_id, &session_id)? {
            Ok(())
        } else {
            Err(RepositoryError::NotFound)
        }
    })
    .await
}

#[tauri::command]
pub async fn associate_request_with_turn(
    state: State<'_, AppState>,
    input: AssociateRequestWithTurnInput,
) -> Result<(), StorageCommandError> {
    input.validate()?;
    let association = RequestTurnAssociation {
        workspace_id: state.workspace_id.clone(),
        session_id: input.session_id,
        request_id: input.request_id,
        turn_id: input.turn_id,
        created_at_ms: now_ms()?,
    };
    run_repository(state.repository.clone(), move |repository| {
        repository
            .associate_request_with_turn(&association)
            .map(|_| ())
    })
    .await
}

#[tauri::command]
pub async fn get_request_turn_associations(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<RequestTurnAssociationDto>, StorageCommandError> {
    validate_uuid(&session_id)?;
    let workspace_id = state.workspace_id.clone();
    run_repository(state.repository.clone(), move |repository| {
        if repository
            .get_session(&workspace_id, &session_id)?
            .is_none()
        {
            return Err(RepositoryError::NotFound);
        }
        repository.get_request_turn_associations(&workspace_id, &session_id)
    })
    .await
    .map(|associations| {
        associations
            .into_iter()
            .map(|association| RequestTurnAssociationDto {
                request_id: association.request_id,
                turn_id: association.turn_id,
            })
            .collect()
    })
}

async fn run_repository<T, F>(
    repository: Arc<SessionRepository>,
    operation: F,
) -> Result<T, StorageCommandError>
where
    T: Send + 'static,
    F: FnOnce(&SessionRepository) -> Result<T, RepositoryError> + Send + 'static,
{
    run_repository_with_timeout(repository, operation, REPOSITORY_COMMAND_TIMEOUT).await
}

async fn run_repository_with_timeout<T, F>(
    repository: Arc<SessionRepository>,
    operation: F,
    timeout: Duration,
) -> Result<T, StorageCommandError>
where
    T: Send + 'static,
    F: FnOnce(&SessionRepository) -> Result<T, RepositoryError> + Send + 'static,
{
    let task = tauri::async_runtime::spawn_blocking(move || operation(&repository));
    tokio::time::timeout(timeout, task)
        .await
        .map_err(|_| StorageCommandError::timed_out())?
        .map_err(|_| StorageCommandError::task_failed())?
        .map_err(StorageCommandError::from_repository)
}

fn timeline_envelope(event: StoredTimelineEvent) -> Result<Option<Envelope>, StorageCommandError> {
    let kind = match event.kind {
        TimelineEventKind::TranscriptFinal => EventKind::TranscriptUpdated,
        TimelineEventKind::SuggestionCompleted => EventKind::SuggestionCompleted,
        TimelineEventKind::SessionState => EventKind::SessionState,
        TimelineEventKind::TranscriptPartial
        | TimelineEventKind::SuggestionChunk
        | TimelineEventKind::Note
        | TimelineEventKind::Pin => return Ok(None),
    };
    let Value::Object(payload) = event.payload else {
        return Err(StorageCommandError::from_repository(
            RepositoryError::ConnectionUnavailable,
        ));
    };
    let sequence = u64::try_from(event.host_sequence).map_err(|_| {
        StorageCommandError::from_repository(RepositoryError::ConnectionUnavailable)
    })?;
    let timestamp_ms = u64::try_from(event.timestamp_ms).map_err(|_| {
        StorageCommandError::from_repository(RepositoryError::ConnectionUnavailable)
    })?;
    Ok(Some(Envelope {
        version: PROTOCOL_VERSION,
        id: event.event_id,
        session_id: Some(event.session_id),
        sequence,
        timestamp_ms,
        kind: ProtocolKind::Event(kind),
        payload,
        correlation_id: event.correlation_id,
    }))
}

fn now_ms() -> Result<i64, StorageCommandError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StorageCommandError::task_failed())?
        .as_millis();
    i64::try_from(millis).map_err(|_| StorageCommandError::task_failed())
}

fn validate_uuid(value: &str) -> Result<(), StorageCommandError> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| StorageCommandError::invalid("Identifier must be a canonical UUID."))?;
    if parsed.hyphenated().to_string() != value {
        return Err(StorageCommandError::invalid(
            "Identifier must be a canonical UUID.",
        ));
    }
    Ok(())
}

fn validate_mode(value: &str) -> Result<(), StorageCommandError> {
    if !is_supported_session_mode(value) {
        return Err(StorageCommandError::invalid("Session mode is invalid."));
    }
    Ok(())
}

fn validate_language_tag(value: &str, allow_auto: bool) -> Result<(), StorageCommandError> {
    if value == "auto" {
        return if allow_auto {
            Ok(())
        } else {
            Err(StorageCommandError::invalid("Language tag is invalid."))
        };
    }
    if value.is_empty() || value.len() > MAX_LANGUAGE_TAG_BYTES {
        return Err(StorageCommandError::invalid("Language tag is invalid."));
    }
    let mut subtags = value.split('-');
    let primary = subtags.next().unwrap_or_default();
    if !(2..=8).contains(&primary.len()) || !primary.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return Err(StorageCommandError::invalid("Language tag is invalid."));
    }
    if subtags.any(|subtag| {
        subtag.is_empty()
            || subtag.len() > 8
            || !subtag.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        return Err(StorageCommandError::invalid("Language tag is invalid."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde_json::{json, Map, Value};
    use tokio::sync::{oneshot, Mutex as AsyncMutex};

    use crate::capture_protection::{
        CaptureProtectionController, CaptureProtectionFailure, CaptureProtectionTarget,
    };
    use crate::protocol::{
        validate_command, CommandKind, Envelope, ProtocolKind, PROTOCOL_VERSION,
    };
    use crate::storage::{
        QueryDispatchAuthorization, RepositoryError, SessionRepository, SessionStatus,
        StoredSession, StoredSessionBrief, StoredTimelineEvent, TimelineEventKind,
    };

    use super::{
        command_requires_capture_protection, dispatch_with_capture_protection,
        ensure_sidecar_command_authorized, run_repository_with_timeout, timeline_envelope,
        validate_language_tag, validate_mode, validate_uuid, AssociateRequestWithTurnInput,
        CompletionStatusInput, CreateSessionInput, DurableAuthorization,
        DurableAuthorizationRequest, SaveSessionBriefInput, SessionRecordDto, StorageCommandError,
        MAX_BRIEF_BYTES,
    };

    const WORKSPACE_ID: &str = "018f0000-0000-7000-8000-000000000099";
    const SESSION_ID: &str = "018f0000-0000-7000-8000-000000000001";
    const REQUEST_ID: &str = "018f0000-0000-7000-8000-000000000002";
    const TURN_ID: &str = "018f0000-0000-7000-8000-000000000003";

    fn query_command() -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: REQUEST_ID.into(),
            session_id: Some(SESSION_ID.into()),
            sequence: 1,
            timestamp_ms: 1,
            kind: ProtocolKind::Command(CommandKind::QueryTrigger),
            payload: Map::from_iter([
                ("text".into(), Value::String("Explain the trade-off".into())),
                ("answer_format".into(), Value::String("chat".into())),
            ]),
            correlation_id: None,
        }
    }

    #[tokio::test]
    async fn valid_query_proves_the_request_turn_before_dispatch() {
        let caller_thread = std::thread::current().id();
        let observed = Arc::new(Mutex::new(None));
        let captured = observed.clone();

        ensure_sidecar_command_authorized(query_command(), WORKSPACE_ID.into(), move |request| {
            let DurableAuthorizationRequest::Query {
                workspace_id,
                session_id,
                request_id,
            } = request
            else {
                panic!("query command produced the wrong authorization request")
            };
            *captured.lock().unwrap() = Some((
                workspace_id,
                session_id,
                request_id,
                std::thread::current().id(),
            ));
            Ok(DurableAuthorization::Query(
                QueryDispatchAuthorization::Authorized,
            ))
        })
        .await
        .unwrap();

        let observed = observed.lock().unwrap().take().unwrap();
        assert_eq!(observed.0, WORKSPACE_ID);
        assert_eq!(observed.1, SESSION_ID);
        assert_eq!(observed.2, REQUEST_ID);
        assert_ne!(observed.3, caller_thread);
    }

    #[tokio::test]
    async fn malformed_query_is_rejected_before_repository_lookup() {
        let mut command = query_command();
        command.payload.remove("answer_format");

        let error = ensure_sidecar_command_authorized(
            command,
            WORKSPACE_ID.into(),
            |_| -> Result<_, RepositoryError> {
                panic!("invalid commands must not reach durable storage")
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "invalid_command_payload");
    }

    #[tokio::test]
    async fn query_maps_stale_and_incomplete_durable_proofs_to_stable_redacted_errors() {
        let cases = [
            (
                QueryDispatchAuthorization::SessionMissing,
                "query_session_stale",
            ),
            (
                QueryDispatchAuthorization::SessionInactive,
                "query_session_stale",
            ),
            (
                QueryDispatchAuthorization::AssociationMissing,
                "query_association_missing",
            ),
            (
                QueryDispatchAuthorization::TranscriptMissing,
                "query_turn_not_durable",
            ),
        ];

        for (authorization, expected_code) in cases {
            let error = ensure_sidecar_command_authorized(
                query_command(),
                WORKSPACE_ID.into(),
                move |_| Ok(DurableAuthorization::Query(authorization)),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), expected_code);
            let serialized = serde_json::to_string(&error).unwrap();
            for forbidden in [WORKSPACE_ID, SESSION_ID, REQUEST_ID, TURN_ID, "SELECT"] {
                assert!(!serialized.contains(forbidden));
            }
        }
    }

    fn session_start_command() -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            id: "018f0000-0000-7000-8000-000000000020".into(),
            session_id: Some(SESSION_ID.into()),
            sequence: 1,
            timestamp_ms: 1,
            kind: ProtocolKind::Command(CommandKind::SessionStart),
            payload: Map::from_iter([
                ("mode".into(), json!("interview")),
                ("input_language".into(), json!("auto")),
                ("response_language".into(), json!("en")),
                ("review_language".into(), json!("en")),
                ("you_source".into(), json!("mic")),
                ("brief_id".into(), json!(TURN_ID)),
            ]),
            correlation_id: None,
        }
    }

    fn command_with_kind(kind: CommandKind, payload: Map<String, Value>) -> Envelope {
        let mut command = query_command();
        command.kind = ProtocolKind::Command(kind);
        command.payload = payload;
        command
    }

    fn listening_command(enabled: bool) -> Envelope {
        command_with_kind(
            CommandKind::ListeningSet,
            Map::from_iter([("enabled".into(), Value::Bool(enabled))]),
        )
    }

    fn audio_system_command(enabled: bool) -> Envelope {
        command_with_kind(
            CommandKind::AudioSystemSet,
            Map::from_iter([("enabled".into(), Value::Bool(enabled))]),
        )
    }

    fn session_stop_command() -> Envelope {
        command_with_kind(CommandKind::SessionStop, Map::new())
    }

    #[test]
    fn capture_sensitive_commands_require_fresh_protection() {
        let cases = [
            ("session.start", session_start_command(), true),
            ("query.trigger", query_command(), true),
            ("listening.set enabled", listening_command(true), true),
            ("audio.system.set enabled", audio_system_command(true), true),
            ("listening.set disabled", listening_command(false), false),
            (
                "audio.system.set disabled",
                audio_system_command(false),
                false,
            ),
            ("session.stop", session_stop_command(), false),
        ];

        for (name, command, expected) in cases {
            assert_eq!(
                command_requires_capture_protection(&command),
                expected,
                "{name}"
            );
        }
    }

    struct FailingCaptureProtectionTarget;

    impl CaptureProtectionTarget for FailingCaptureProtectionTarget {
        fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure> {
            Err(CaptureProtectionFailure::VerificationFailed)
        }
    }

    struct FakeCaptureProtectionTarget {
        outcomes: Mutex<VecDeque<Result<(), CaptureProtectionFailure>>>,
    }

    impl FakeCaptureProtectionTarget {
        fn new(outcomes: impl IntoIterator<Item = Result<(), CaptureProtectionFailure>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
            }
        }
    }

    impl CaptureProtectionTarget for FakeCaptureProtectionTarget {
        fn apply_and_verify(&self) -> Result<(), CaptureProtectionFailure> {
            self.outcomes
                .lock()
                .expect("capture protection fake lock must not be poisoned")
                .pop_front()
                .expect("capture protection fake must have an outcome")
        }
    }

    #[tokio::test]
    async fn capture_protection_failure_never_reaches_authorization_or_dispatch() {
        let controller =
            CaptureProtectionController::with_target(Arc::new(FailingCaptureProtectionTarget));

        let error = dispatch_with_capture_protection(&query_command(), &controller, || async {
            panic!("unprotected commands must not reach authorization or dispatch")
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "capture_protection_required");
        assert_eq!(
            error.to_string(),
            "Live mode is unavailable because screen capture protection could not be confirmed."
        );
    }

    #[tokio::test]
    async fn capture_protection_lease_blocks_reapply_until_dispatch_completes() {
        let controller = Arc::new(CaptureProtectionController::with_target(Arc::new(
            FakeCaptureProtectionTarget::new([
                Ok(()),
                Err(CaptureProtectionFailure::VerificationFailed),
            ]),
        )));
        let (dispatch_started_sender, dispatch_started) = oneshot::channel();
        let (complete_dispatch, complete_dispatch_receiver) = oneshot::channel();
        let dispatch_controller = controller.clone();
        let command = query_command();
        let dispatch = tokio::spawn(async move {
            dispatch_with_capture_protection(&command, dispatch_controller.as_ref(), || async {
                dispatch_started_sender
                    .send(())
                    .expect("dispatch start receiver must remain available");
                complete_dispatch_receiver
                    .await
                    .expect("dispatch completion sender must remain available");
                Ok(())
            })
            .await
        });

        dispatch_started
            .await
            .expect("protected dispatch must reach its closure");
        assert_eq!(
            controller.status().state,
            crate::capture_protection::CaptureProtectionState::Protected
        );

        let reapply_controller = controller.clone();
        let mut reapply =
            tokio::task::spawn_blocking(move || reapply_controller.reapply_and_verify());
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut reapply)
                .await
                .is_err(),
            "reapply must not transition protection while dispatch is incomplete"
        );
        assert_eq!(
            controller.status().state,
            crate::capture_protection::CaptureProtectionState::Protected
        );

        complete_dispatch
            .send(())
            .expect("dispatch completion receiver must remain available");
        dispatch
            .await
            .expect("protected dispatch task must not panic")
            .expect("protected dispatch must complete");

        assert_eq!(
            reapply.await.expect("reapply task must not panic").state,
            crate::capture_protection::CaptureProtectionState::Unavailable
        );
    }

    #[tokio::test]
    async fn non_query_dispatch_does_not_consult_request_turn_storage() {
        let mut command = query_command();
        command.kind = ProtocolKind::Command(CommandKind::ListeningSet);
        command.payload = Map::from_iter([("enabled".into(), Value::Bool(true))]);

        ensure_sidecar_command_authorized(
            command,
            WORKSPACE_ID.into(),
            |_| -> Result<_, RepositoryError> {
                panic!("non-query commands must bypass association storage")
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn session_start_requires_an_active_session_in_the_same_workspace() {
        let observed = Arc::new(Mutex::new(None));
        let captured = observed.clone();

        ensure_sidecar_command_authorized(
            session_start_command(),
            WORKSPACE_ID.into(),
            move |request| {
                let DurableAuthorizationRequest::SessionStart {
                    workspace_id,
                    session_id,
                } = request
                else {
                    panic!("session.start produced the wrong authorization request")
                };
                *captured.lock().unwrap() = Some((workspace_id, session_id));
                Ok(DurableAuthorization::SessionStart(Some(
                    SessionStatus::Active,
                )))
            },
        )
        .await
        .unwrap();

        let observed = observed.lock().unwrap().take().unwrap();
        assert_eq!(observed.0, WORKSPACE_ID);
        assert_eq!(observed.1, SESSION_ID);
    }

    #[tokio::test]
    async fn session_start_rejects_missing_completed_and_deleted_sessions_with_redacted_error() {
        for status in [
            None,
            Some(SessionStatus::Completed),
            Some(SessionStatus::Interrupted),
        ] {
            let error = ensure_sidecar_command_authorized(
                session_start_command(),
                WORKSPACE_ID.into(),
                move |_| Ok(DurableAuthorization::SessionStart(status)),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), "session_start_not_active");
            let serialized = serde_json::to_string(&error).unwrap();
            for forbidden in [WORKSPACE_ID, SESSION_ID, REQUEST_ID, TURN_ID, "SELECT"] {
                assert!(!serialized.contains(forbidden));
            }
        }
    }

    #[tokio::test]
    async fn repository_timeout_releases_the_session_operation_gate() {
        let temp = tempfile::tempdir().unwrap();
        let repository =
            Arc::new(SessionRepository::open(temp.path().join("timeout.db"), &[0x41; 32]).unwrap());
        let gate = Arc::new(AsyncMutex::new(()));
        let (started_sender, started_receiver) = oneshot::channel();
        let operation = tokio::spawn({
            let gate = gate.clone();
            async move {
                let _session_operation = gate.lock_owned().await;
                run_repository_with_timeout(
                    repository,
                    move |_| {
                        let _ = started_sender.send(());
                        std::thread::sleep(Duration::from_millis(100));
                        Ok(())
                    },
                    Duration::from_millis(5),
                )
                .await
            }
        });
        started_receiver.await.unwrap();

        let error = operation.await.unwrap().unwrap_err();
        assert_eq!(error.code, "storage_timeout");
        let _released = tokio::time::timeout(Duration::from_millis(25), gate.lock())
            .await
            .expect("storage timeout must release the session operation gate");
    }

    #[test]
    fn tauri_input_dtos_accept_exact_snake_case_shapes() {
        let create: CreateSessionInput = serde_json::from_value(json!({
            "mode": "interview",
            "ui_language": "en",
            "input_language": "auto",
            "response_language": "ur",
            "review_language": "en"
        }))
        .unwrap();
        assert!(create.validate().is_ok());

        let association: AssociateRequestWithTurnInput = serde_json::from_value(json!({
            "session_id": "018f0000-0000-7000-8000-000000000001",
            "request_id": "018f0000-0000-7000-8000-000000000002",
            "turn_id": "018f0000-0000-7000-8000-000000000003"
        }))
        .unwrap();
        assert!(association.validate().is_ok());
        assert!(
            serde_json::from_value::<AssociateRequestWithTurnInput>(json!({
                "sessionId": "018f0000-0000-7000-8000-000000000001",
                "request_id": "018f0000-0000-7000-8000-000000000002",
                "turn_id": "018f0000-0000-7000-8000-000000000003"
            }))
            .is_err()
        );
        assert!(matches!(
            serde_json::from_value::<CompletionStatusInput>(json!("interrupted")).unwrap(),
            CompletionStatusInput::Interrupted
        ));
    }

    #[test]
    fn storage_errors_serialize_without_repository_context() {
        let serialized =
            serde_json::to_string(&StorageCommandError::invalid("Invalid input.")).unwrap();

        assert_eq!(
            serialized,
            r#"{"code":"invalid_storage_command","message":"Invalid input."}"#
        );
        for forbidden in ["workspace", "session_id", "request_id", "turn_id", "SELECT"] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn repository_conflicts_map_to_stable_redacted_command_errors() {
        let cases = [
            (
                RepositoryError::RequestTurnAssociationConflict,
                "request_turn_conflict",
            ),
            (
                RepositoryError::SessionCompletionConflict,
                "session_completion_conflict",
            ),
        ];

        for (repository_error, expected_code) in cases {
            let value =
                serde_json::to_value(StorageCommandError::from_repository(repository_error))
                    .unwrap();
            assert_eq!(value["code"], expected_code);
            let serialized = value.to_string();
            for forbidden in ["workspace", "session_id", "request_id", "turn_id", "SELECT"] {
                assert!(!serialized.contains(forbidden));
            }
        }
    }

    #[test]
    fn command_validation_rejects_noncanonical_ids_and_malformed_languages() {
        assert!(validate_uuid("018f0000-0000-7000-8000-000000000001").is_ok());
        assert!(validate_uuid("018F0000-0000-7000-8000-000000000001").is_err());
        assert!(validate_uuid("not-a-uuid").is_err());
        for language in ["en", "en-US", "zh-Hans-CN", "ar-XB"] {
            assert!(validate_language_tag(language, false).is_ok());
        }
        assert!(validate_language_tag("auto", true).is_ok());
        assert!(validate_language_tag("auto", false).is_err());
        for language in ["e", "en--US", "en_XX", "en-123456789"] {
            assert!(validate_language_tag(language, false).is_err());
        }
    }

    #[test]
    fn mode_and_json_brief_bounds_are_enforced() {
        for mode in ["interview", "sales", "meeting", "presentation", "classroom"] {
            assert!(validate_mode(mode).is_ok());
        }
        assert!(validate_mode("").is_err());
        assert!(validate_mode("Interview").is_err());
        assert!(validate_mode("custom-mode").is_err());
        let input = SaveSessionBriefInput {
            session_id: "018f0000-0000-7000-8000-000000000001".into(),
            brief: Map::from_iter([("notes".into(), Value::String("x".repeat(MAX_BRIEF_BYTES)))]),
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn create_session_and_session_start_share_the_closed_mode_contract() {
        let start_fixture = include_str!("../../../protocol/v1/fixtures/session-start.json");

        for mode in ["interview", "sales", "meeting", "presentation", "classroom"] {
            let input: CreateSessionInput = serde_json::from_value(json!({
                "mode": mode,
                "ui_language": "en",
                "input_language": "auto",
                "response_language": "en",
                "review_language": "en"
            }))
            .unwrap();
            let mut command: Envelope = serde_json::from_str(start_fixture).unwrap();
            command.payload.insert("mode".into(), mode.into());

            assert!(input.validate().is_ok(), "create_session rejected {mode}");
            assert!(
                validate_command(&command).is_ok(),
                "session.start rejected {mode}"
            );
        }

        for mode in ["unknown", "custom-mode"] {
            let input: CreateSessionInput = serde_json::from_value(json!({
                "mode": mode,
                "ui_language": "en",
                "input_language": "auto",
                "response_language": "en",
                "review_language": "en"
            }))
            .unwrap();
            let mut command: Envelope = serde_json::from_str(start_fixture).unwrap();
            command.payload.insert("mode".into(), mode.into());

            assert!(input.validate().is_err(), "create_session accepted {mode}");
            assert!(
                validate_command(&command).is_err(),
                "session.start accepted {mode}"
            );
        }
    }

    #[test]
    fn session_dto_uses_exact_task_ten_shape_and_json_brief() {
        let dto = SessionRecordDto::from(StoredSession {
            workspace_id: "workspace".into(),
            session_id: "018f0000-0000-7000-8000-000000000001".into(),
            mode: "interview".into(),
            status: SessionStatus::Active,
            ui_language: Some("en".into()),
            input_language: "auto".into(),
            response_language: "ur".into(),
            review_language: "en".into(),
            started_at_ms: 1,
            completed_at_ms: None,
            brief: Some(StoredSessionBrief {
                brief: json!({"role": "engineer"}),
                updated_at_ms: 2,
            }),
        });
        assert_eq!(
            serde_json::to_value(dto).unwrap(),
            json!({
                "id": "018f0000-0000-7000-8000-000000000001",
                "mode": "interview",
                "status": "active",
                "ui_language": "en",
                "input_language": "auto",
                "response_language": "ur",
                "review_language": "en",
                "brief": {"role": "engineer"}
            })
        );
    }

    #[test]
    fn persisted_timeline_uses_host_sequence_and_closed_protocol_payload() {
        let envelope = timeline_envelope(StoredTimelineEvent {
            workspace_id: "workspace".into(),
            session_id: "018f0000-0000-7000-8000-000000000001".into(),
            event_id: "018f0000-0000-7000-8000-000000000002".into(),
            host_sequence: 7,
            source_generation: 3,
            source_sequence: 99,
            timestamp_ms: 123,
            kind: TimelineEventKind::SuggestionCompleted,
            correlation_id: Some("018f0000-0000-7000-8000-000000000003".into()),
            request_id: Some("018f0000-0000-7000-8000-000000000003".into()),
            turn_id: Some("018f0000-0000-7000-8000-000000000004".into()),
            payload: json!({
                "suggestion_id": "018f0000-0000-7000-8000-000000000005",
                "text": "answer"
            }),
        })
        .unwrap()
        .unwrap();

        assert_eq!(envelope.sequence, 7);
        assert_eq!(envelope.timestamp_ms, 123);
        assert_eq!(envelope.payload.len(), 2);
        assert!(!envelope.payload.contains_key("turn_id"));
    }

    #[test]
    fn note_and_pin_rows_do_not_mutate_closed_protocol_v1() {
        for kind in [TimelineEventKind::Note, TimelineEventKind::Pin] {
            let event = StoredTimelineEvent {
                workspace_id: "workspace".into(),
                session_id: "session".into(),
                event_id: "event".into(),
                host_sequence: 1,
                source_generation: 1,
                source_sequence: 1,
                timestamp_ms: 1,
                kind,
                correlation_id: None,
                request_id: None,
                turn_id: None,
                payload: json!({}),
            };
            assert!(timeline_envelope(event).unwrap().is_none());
        }
    }

    #[test]
    fn webview_capability_exposes_no_shell_plugin_permissions() {
        let capability = include_str!("../capabilities/default.json");

        assert!(!capability.contains("shell:"));
        assert!(capability.contains("core:default"));
    }
}
