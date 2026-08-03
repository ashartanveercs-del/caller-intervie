import { decodeEnvelope, type Envelope } from "../shared/protocol";
import type {
  PlatformApi,
  SessionRecord,
  SidecarStatus,
} from "./types";

export type BrowserPlatform = PlatformApi & {
  emit(event: Envelope): void;
  sentCommands(): readonly Envelope[];
};

const initialStatus: SidecarStatus = { state: "ready", restartCount: 0, diagnostics: [] };

export function createBrowserPlatform(): BrowserPlatform {
  const sessions = new Map<string, SessionRecord>();
  const timelines = new Map<string, Envelope[]>();
  const listeners = new Set<(event: Envelope) => void>();
  const commands: Envelope[] = [];
  let status = initialStatus;
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

  return {
    async sidecarStatus() {
      return { ...status, diagnostics: [...status.diagnostics] };
    },
    async send(command) {
      commands.push(decodeEnvelope(command));
    },
    async restartSidecar() {
      status = { state: "ready", restartCount: status.restartCount + 1, diagnostics: [] };
      return { ...status, diagnostics: [] };
    },
    async subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
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
      const active = [...sessions.values()].find((session) => session.status === "active");
      return active ? cloneSession(active) : null;
    },
    async deleteSession(sessionId) {
      requireSession(sessions, sessionId);
      sessions.delete(sessionId);
      timelines.delete(sessionId);
    },
    emit,
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
