import { CommandKind, decodeEnvelope, type Envelope } from "../shared/protocol";
import type {
  AssociateRequestWithTurnInput,
  CaptureProtectionStatus,
  PlatformApi,
  SessionRecord,
  SidecarStatus,
  StorageHealth,
} from "./types";

export type BrowserPlatform = PlatformApi & {
  emit(event: Envelope): void;
  emitStorageHealth(health: StorageHealth): void;
  sentCommands(): readonly Envelope[];
};

const initialStatus: SidecarStatus = { state: "ready", restartCount: 0, diagnostics: [] };
const initialStorageHealth: StorageHealth = { status: "ready", recoverable: false };
const browserPreviewCaptureProtection: CaptureProtectionStatus = {
  state: "unsupported",
  code: "browser_preview",
};
const captureProtectionRequiredMessage =
  "Live mode is unavailable because screen capture protection could not be confirmed.";

export function createBrowserPlatform(): BrowserPlatform {
  const sessions = new Map<string, SessionRecord>();
  const sessionCreationOrder = new Map<string, number>();
  const timelines = new Map<string, Envelope[]>();
  const requestTurnAssociations = new Map<string, Map<string, string>>();
  const listeners = new Set<(event: Envelope) => void>();
  const storageHealthListeners = new Set<(health: StorageHealth) => void>();
  const commands: Envelope[] = [];
  let status = initialStatus;
  let storageHealth = initialStorageHealth;
  let nextSession = 1;

  const emit = (event: Envelope) => {
    const decoded = decodeEnvelope(event);
    if (decoded.session_id) {
      const timeline = timelines.get(decoded.session_id) ?? [];
      if (!timeline.some((item) => item.id === decoded.id)) {
        timelines.set(decoded.session_id, [...timeline, decoded]);
      }
    }
    for (const listener of listeners) {
      listener(decoded);
    }
  };

  const emitStorageHealth = (health: StorageHealth) => {
    storageHealth = cloneStorageHealth(health);
    for (const listener of storageHealthListeners) {
      listener(cloneStorageHealth(storageHealth));
    }
  };

  return {
    async sidecarStatus() {
      return { ...status, diagnostics: [...status.diagnostics] };
    },
    async storageHealth() {
      return cloneStorageHealth(storageHealth);
    },
    async captureProtectionStatus() {
      return { ...browserPreviewCaptureProtection };
    },
    async send(command) {
      const decoded = decodeEnvelope(command);
      if (commandRequiresCaptureProtection(decoded)) {
        throw captureProtectionRequiredError();
      }
      commands.push(decoded);
    },
    async restartSidecar() {
      status = { state: "ready", restartCount: status.restartCount + 1, diagnostics: [] };
      return { ...status, diagnostics: [] };
    },
    async retryCaptureProtection() {
      return { ...browserPreviewCaptureProtection };
    },
    async subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async subscribeStorageHealth(listener) {
      storageHealthListeners.add(listener);
      return () => storageHealthListeners.delete(listener);
    },
    async subscribeCaptureProtection(_listener) {
      return () => undefined;
    },
    async associateRequestWithTurn(input) {
      requireSession(sessions, input.sessionId);
      associateRequestWithTurn(requestTurnAssociations, input);
    },
    async getRequestTurnAssociations(sessionId) {
      requireSession(sessions, sessionId);
      return [...(requestTurnAssociations.get(sessionId) ?? new Map<string, string>())]
        .map(([requestId, turnId]) => ({ requestId, turnId }));
    },
    async createSession(input) {
      const id = deterministicUuid(nextSession++);
      const session: SessionRecord = {
        id,
        mode: input.mode,
        status: "active",
        uiLanguage: input.uiLanguage,
        inputLanguage: input.inputLanguage,
        responseLanguage: input.responseLanguage,
        reviewLanguage: input.reviewLanguage,
      };
      sessions.set(id, session);
      sessionCreationOrder.set(id, nextSession - 1);
      return cloneSession(session);
    },
    async saveSessionBrief(input) {
      const session = requireSession(sessions, input.sessionId);
      sessions.set(input.sessionId, { ...session, brief: structuredClone(input.brief) });
    },
    async completeSession(sessionId, sessionStatus) {
      const session = requireSession(sessions, sessionId);
      sessions.set(sessionId, { ...session, status: sessionStatus });
    },
    async listSessions(limit) {
      return [...sessions.values()].slice(0, limit).map(cloneSession);
    },
    async getSession(sessionId) {
      return cloneSession(requireSession(sessions, sessionId));
    },
    async getTimeline(sessionId) {
      requireSession(sessions, sessionId);
      return [...(timelines.get(sessionId) ?? [])];
    },
    async restoreActiveSession() {
      const active = [...sessions.values()]
        .filter((session) => session.status === "active")
        .reduce<SessionRecord | null>((newest, candidate) => {
          if (!newest) return candidate;
          return (sessionCreationOrder.get(candidate.id) ?? -1) > (sessionCreationOrder.get(newest.id) ?? -1)
            ? candidate
            : newest;
        }, null);
      return active ? cloneSession(active) : null;
    },
    async deleteSession(sessionId) {
      requireSession(sessions, sessionId);
      sessions.delete(sessionId);
      sessionCreationOrder.delete(sessionId);
      timelines.delete(sessionId);
      requestTurnAssociations.delete(sessionId);
    },
    emit,
    emitStorageHealth,
    sentCommands() {
      return [...commands];
    },
  };
}

function deterministicUuid(sequence: number) {
  return `018f0000-0000-7000-8000-${String(sequence).padStart(12, "0")}`;
}

function requireSession(sessions: Map<string, SessionRecord>, sessionId: string): SessionRecord {
  const session = sessions.get(sessionId);
  if (!session) {
    throw new Error(`session not found: ${sessionId}`);
  }
  return session;
}

function cloneSession(session: SessionRecord): SessionRecord {
  return { ...session, brief: session.brief ? structuredClone(session.brief) : undefined };
}

function cloneStorageHealth(health: StorageHealth): StorageHealth {
  return {
    status: health.status,
    ...(health.code ? { code: health.code } : {}),
    ...(health.message ? { message: health.message } : {}),
    recoverable: health.recoverable,
  };
}

function commandRequiresCaptureProtection(command: Envelope): boolean {
  return command.kind === CommandKind.SESSION_START
    || command.kind === CommandKind.QUERY_TRIGGER
    || ((command.kind === CommandKind.LISTENING_SET || command.kind === CommandKind.AUDIO_SYSTEM_SET)
      && command.payload.enabled === true);
}

function captureProtectionRequiredError(): Error & { code: "capture_protection_required" } {
  return Object.assign(new Error(captureProtectionRequiredMessage), {
    code: "capture_protection_required" as const,
  });
}

function associateRequestWithTurn(
  associationsBySession: Map<string, Map<string, string>>,
  input: AssociateRequestWithTurnInput,
) {
  const associations = associationsBySession.get(input.sessionId) ?? new Map<string, string>();
  const existingTurnId = associations.get(input.requestId);
  if (existingTurnId && existingTurnId !== input.turnId) {
    throw new Error(`request association conflict for ${input.requestId}`);
  }
  associations.set(input.requestId, input.turnId);
  associationsBySession.set(input.sessionId, associations);
}
