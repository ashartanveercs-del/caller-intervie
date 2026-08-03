import type { Envelope } from "../shared/protocol";

export type SidecarState = "starting" | "ready" | "restarting" | "stopped" | "failed";

export type SidecarStatus = {
  state: SidecarState;
  restartCount: number;
  diagnostics: string[];
};

export type SessionStatus = "active" | "completed" | "interrupted";

export type SessionBrief = Record<string, unknown>;

export type CreateSessionInput = {
  mode: string;
  uiLanguage?: string;
  inputLanguage: string;
  responseLanguage: string;
  reviewLanguage: string;
};

export type SaveSessionBriefInput = {
  sessionId: string;
  brief: SessionBrief;
};

export type SessionRecord = {
  id: string;
  mode: string;
  status: SessionStatus;
  uiLanguage?: string;
  inputLanguage: string;
  responseLanguage: string;
  reviewLanguage: string;
  brief?: SessionBrief;
};

export interface PlatformApi {
  sidecarStatus(): Promise<SidecarStatus>;
  send(command: Envelope): Promise<void>;
  restartSidecar(): Promise<SidecarStatus>;
  subscribe(listener: (event: Envelope) => void): Promise<() => void>;
  createSession(input: CreateSessionInput): Promise<SessionRecord>;
  saveSessionBrief(input: SaveSessionBriefInput): Promise<void>;
  completeSession(sessionId: string, status: "completed" | "interrupted"): Promise<void>;
  listSessions(limit: number): Promise<SessionRecord[]>;
  getSession(sessionId: string): Promise<SessionRecord>;
  getTimeline(sessionId: string): Promise<Envelope[]>;
  restoreActiveSession(): Promise<SessionRecord | null>;
  deleteSession(sessionId: string): Promise<void>;
}
