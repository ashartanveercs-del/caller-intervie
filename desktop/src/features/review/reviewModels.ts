import type { RequestTurnAssociation, SessionRecord } from "../../platform";
import { EventKind, type Envelope } from "../../shared/protocol";
import { parseReviewArtifacts, type ReviewNote, type ReviewPin } from "./reviewArtifacts";

export type ReviewTranscriptTurn = {
  id: string;
  text: string;
  speaker: string;
  language?: string;
  sequence: number;
  timestampMs: number;
};

export type ReviewSuggestion = {
  id: string;
  text: string;
  question?: string;
  sequence: number;
};

export type ReviewTranslation = {
  id: string;
  original: string;
  translated: string;
  sourceLanguage?: string;
  targetLanguage?: string;
  sequence: number;
};

export type ReviewInterruption = {
  id: string;
  message: string;
  sequence: number;
  timestampMs: number;
};

export type ReviewModel = {
  session: SessionRecord;
  role?: string;
  company?: string;
  transcript: ReviewTranscriptTurn[];
  suggestions: ReviewSuggestion[];
  translations: ReviewTranslation[];
  interruptions: ReviewInterruption[];
  notes: ReviewNote[];
  pins: ReviewPin[];
};

export function buildReviewModel(
  session: SessionRecord,
  timeline: readonly Envelope[],
  associations: readonly RequestTurnAssociation[],
): ReviewModel {
  const ordered = [...timeline].sort(compareEnvelope);
  const transcriptById = new Map<string, ReviewTranscriptTurn>();
  const translationsById = new Map<string, ReviewTranslation>();

  for (const event of ordered) {
    if (event.kind !== EventKind.TRANSCRIPT_UPDATED || event.payload.is_final !== true) continue;
    const id = stringField(event.payload, "turn_id");
    const text = stringField(event.payload, "text");
    if (!id || text === undefined) continue;
    transcriptById.set(id, {
      id,
      text,
      speaker: stringField(event.payload, "speaker_role") ?? "Speaker",
      language: stringField(event.payload, "language"),
      sequence: event.sequence,
      timestampMs: event.timestamp_ms,
    });

    const translated = stringField(event.payload, "translated_text");
    if (translated) {
      translationsById.set(id, {
        id,
        original: text,
        translated,
        sourceLanguage: stringField(event.payload, "language"),
        targetLanguage: stringField(event.payload, "translated_language"),
        sequence: event.sequence,
      });
    }
  }

  const transcript = [...transcriptById.values()].sort(compareSequence);
  const turnById = new Map(transcript.map((turn) => [turn.id, turn]));
  const turnByRequest = new Map(associations.map((item) => [item.requestId, item.turnId]));
  const suggestionById = new Map<string, ReviewSuggestion>();
  const interruptions: ReviewInterruption[] = [];

  for (const event of ordered) {
    if (event.kind === EventKind.SUGGESTION_COMPLETED) {
      const id = stringField(event.payload, "suggestion_id");
      const text = stringField(event.payload, "text");
      if (!id || text === undefined) continue;
      const turnId = event.correlation_id ? turnByRequest.get(event.correlation_id) : undefined;
      suggestionById.set(id, {
        id,
        text,
        question: turnId ? turnById.get(turnId)?.text : undefined,
        sequence: event.sequence,
      });
    }

    if (event.kind === EventKind.RUNTIME_ERROR) {
      const message = stringField(event.payload, "message");
      if (message) {
        interruptions.push({
          id: event.id,
          message,
          sequence: event.sequence,
          timestampMs: event.timestamp_ms,
        });
      }
    }
  }

  const context = recordField(session.brief, "context");
  const artifacts = parseReviewArtifacts(session.brief);
  return {
    session,
    role: stringField(context, "role"),
    company: stringField(context, "company"),
    transcript,
    suggestions: [...suggestionById.values()].sort(compareSequence),
    translations: [...translationsById.values()].sort(compareSequence),
    interruptions: interruptions.sort(compareSequence),
    notes: artifacts.filter((artifact): artifact is ReviewNote => artifact.kind === "note"),
    pins: artifacts.filter((artifact): artifact is ReviewPin => artifact.kind === "pin"),
  };
}

function compareEnvelope(left: Envelope, right: Envelope): number {
  return left.sequence - right.sequence || left.timestamp_ms - right.timestamp_ms;
}

function compareSequence(left: { sequence: number }, right: { sequence: number }): number {
  return left.sequence - right.sequence;
}

function stringField(value: unknown, key: string): string | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const field = (value as Record<string, unknown>)[key];
  return typeof field === "string" ? field : undefined;
}

function recordField(value: unknown, key: string): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const field = (value as Record<string, unknown>)[key];
  return field && typeof field === "object" && !Array.isArray(field)
    ? field as Record<string, unknown>
    : undefined;
}
