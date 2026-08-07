import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { decodeEnvelope } from "../shared/protocol";
import type {
  CreateSessionInput,
  AssociateRequestWithTurnInput,
  PlatformApi,
  RequestTurnAssociation,
  SaveSessionBriefInput,
  SessionBrief,
  SessionRecord,
  SessionStatus,
  SidecarState,
  SidecarStatus,
  StorageHealth,
  StorageHealthStatus,
} from "./types";

type SidecarStatusDto = {
  state: SidecarState;
  restartCount: number;
  diagnostics: string[];
};

type StorageHealthDto = {
  status: StorageHealthStatus;
  code?: string;
  message?: string;
  recoverable: boolean;
};

type RequestTurnAssociationDto = {
  request_id: string;
  turn_id: string;
};

// These DTOs deliberately stay in the adapter until Task 9's native types land.
type SessionDto = {
  id?: string;
  session_id?: string;
  mode: string;
  status: SessionStatus;
  ui_language?: string;
  input_language: string;
  response_language: string;
  review_language: string;
  brief?: SessionBrief;
};

function mapStatus(dto: SidecarStatusDto): SidecarStatus {
  return {
    state: dto.state,
    restartCount: dto.restartCount,
    diagnostics: dto.diagnostics,
  };
}

function mapStorageHealth(dto: StorageHealthDto): StorageHealth {
  return {
    status: dto.status,
    ...(typeof dto.code === "string" ? { code: dto.code } : {}),
    ...(typeof dto.message === "string" ? { message: dto.message } : {}),
    recoverable: dto.recoverable === true,
  };
}

function mapRequestTurnAssociation(dto: RequestTurnAssociationDto): RequestTurnAssociation {
  return { requestId: dto.request_id, turnId: dto.turn_id };
}

function mapSession(dto: SessionDto): SessionRecord {
  const id = dto.id ?? dto.session_id;
  if (!id) {
    throw new Error("storage command returned a session without an id");
  }
  return {
    id,
    mode: dto.mode,
    status: dto.status,
    uiLanguage: dto.ui_language,
    inputLanguage: dto.input_language,
    responseLanguage: dto.response_language,
    reviewLanguage: dto.review_language,
    brief: dto.brief,
  };
}

export function createDesktopPlatform(): PlatformApi {
  return {
    async sidecarStatus() {
      return mapStatus(await invoke<SidecarStatusDto>("sidecar_status"));
    },
    async storageHealth() {
      return mapStorageHealth(await invoke<StorageHealthDto>("storage_health"));
    },
    async send(command) {
      await invoke("send_sidecar_command", { command: decodeEnvelope(command) });
    },
    async restartSidecar() {
      return mapStatus(await invoke<SidecarStatusDto>("restart_sidecar"));
    },
    async subscribe(listener) {
      return listen<unknown>("sidecar://event", (event) => listener(decodeEnvelope(event.payload)));
    },
    async subscribeStorageHealth(listener) {
      return listen<StorageHealthDto>("storage://health", (event) => listener(mapStorageHealth(event.payload)));
    },
    async associateRequestWithTurn(input) {
      await invoke("associate_request_with_turn", { input: toRequestTurnAssociationDto(input) });
    },
    async getRequestTurnAssociations(sessionId) {
      const associations = await invoke<RequestTurnAssociationDto[]>("get_request_turn_associations", { sessionId });
      return associations.map(mapRequestTurnAssociation);
    },
    async createSession(input) {
      const dto = await invoke<SessionDto>("create_session", { input: toCreateSessionDto(input) });
      return mapSession(dto);
    },
    async saveSessionBrief(input) {
      await invoke("save_session_brief", { input: toSaveSessionBriefDto(input) });
    },
    async completeSession(sessionId, status) {
      await invoke("complete_session", { sessionId, status });
    },
    async listSessions(limit) {
      const sessions = await invoke<SessionDto[]>("list_sessions", { limit });
      return sessions.map(mapSession);
    },
    async getSession(sessionId) {
      return mapSession(await invoke<SessionDto>("get_session", { sessionId }));
    },
    async getTimeline(sessionId) {
      const timeline = await invoke<unknown[]>("get_timeline", { sessionId });
      return timeline.map(decodeEnvelope);
    },
    async restoreActiveSession() {
      const session = await invoke<SessionDto | null>("restore_active_session");
      return session === null ? null : mapSession(session);
    },
    async deleteSession(sessionId) {
      await invoke("delete_session", { sessionId });
    },
  };
}

function toCreateSessionDto(input: CreateSessionInput) {
  return {
    mode: input.mode,
    ui_language: input.uiLanguage,
    input_language: input.inputLanguage,
    response_language: input.responseLanguage,
    review_language: input.reviewLanguage,
  };
}

function toSaveSessionBriefDto(input: SaveSessionBriefInput) {
  return { session_id: input.sessionId, brief: input.brief };
}

function toRequestTurnAssociationDto(input: AssociateRequestWithTurnInput) {
  return {
    session_id: input.sessionId,
    request_id: input.requestId,
    turn_id: input.turnId,
  };
}
