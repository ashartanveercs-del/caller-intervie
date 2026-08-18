import type { SessionBrief } from "../../platform";

export type ReviewNote = {
  id: string;
  kind: "note";
  text: string;
  createdAtMs: number;
};

export type ReviewPin = {
  id: string;
  kind: "pin";
  turnId: string;
  question: string;
  suggestion: string;
  createdAtMs: number;
};

export type ReviewArtifact = ReviewNote | ReviewPin;

export function parseReviewArtifacts(brief: unknown): ReviewArtifact[] {
  if (!isRecord(brief) || !Array.isArray(brief.reviewArtifacts)) return [];
  return brief.reviewArtifacts.flatMap((value) => {
    const artifact = parseReviewArtifact(value);
    return artifact ? [artifact] : [];
  });
}

export function briefWithReviewArtifacts(
  brief: SessionBrief | undefined,
  artifacts: readonly ReviewArtifact[],
): SessionBrief {
  return {
    ...(brief ?? {}),
    reviewArtifacts: artifacts.map((artifact) => ({ ...artifact })),
  };
}

function parseReviewArtifact(value: unknown): ReviewArtifact | undefined {
  if (!isRecord(value)) return undefined;
  const id = nonEmptyString(value.id);
  const createdAtMs = finiteNumber(value.createdAtMs);
  if (!id || createdAtMs === undefined) return undefined;

  if (value.kind === "note") {
    const text = nonEmptyString(value.text);
    return text ? { id, kind: "note", text, createdAtMs } : undefined;
  }

  if (value.kind === "pin") {
    const turnId = nonEmptyString(value.turnId);
    const question = nonEmptyString(value.question);
    const suggestion = nonEmptyString(value.suggestion);
    return turnId && question && suggestion
      ? { id, kind: "pin", turnId, question, suggestion, createdAtMs }
      : undefined;
  }

  return undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value : undefined;
}

function finiteNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
}
