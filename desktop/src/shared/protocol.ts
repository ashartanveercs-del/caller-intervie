import { z } from "zod";

export const PROTOCOL_VERSION = 1 as const;

export enum CommandKind {
  HANDSHAKE_REQUEST = "handshake.request",
  SESSION_START = "session.start",
  SESSION_STOP = "session.stop",
  LISTENING_SET = "listening.set",
  YOU_SOURCE_SET = "you_source.set",
  QUERY_TRIGGER = "query.trigger",
  AUDIO_SYSTEM_SET = "audio.system.set",
  AUDIO_DEVICE_SET = "audio.device.set",
  KNOWLEDGE_INGEST = "knowledge.ingest",
  SESSION_SNAPSHOT_REQUEST = "session.snapshot.request",
}

export enum EventKind {
  SIDECAR_READY = "sidecar.ready",
  SESSION_STATE = "session.state",
  TRANSCRIPT_UPDATED = "transcript.updated",
  SUGGESTION_CHUNK = "suggestion.chunk",
  SUGGESTION_COMPLETED = "suggestion.completed",
  AUDIO_HEALTH = "audio.health",
  PROVIDER_HEALTH = "provider.health",
  KNOWLEDGE_STATE = "knowledge.state",
  RUNTIME_ERROR = "runtime.error",
}

const versionSchema = z
  .number()
  .int()
  .min(0)
  .max(65535)
  .refine((version) => version === PROTOCOL_VERSION, {
    message: "unsupported protocol version",
  });
const nonnegativeInteger = (field: string) =>
  z.number().int().nonnegative({ message: `${field} must be a nonnegative integer` });
const uuid = z
  .string()
  .regex(
    /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/,
    "must be a UUID string",
  );

const envelopeFields = {
  version: versionSchema,
  id: uuid,
  session_id: uuid.nullable(),
  sequence: nonnegativeInteger("sequence"),
  timestamp_ms: nonnegativeInteger("timestamp_ms"),
  payload: z.record(z.string(), z.unknown()),
  correlation_id: uuid.nullable(),
};

const commandEnvelopeSchema = z
  .object({ ...envelopeFields, kind: z.enum(CommandKind) })
  .strict();
const eventEnvelopeSchema = z.object({ ...envelopeFields, kind: z.enum(EventKind) }).strict();

export const EnvelopeSchema = z.discriminatedUnion("kind", [
  commandEnvelopeSchema,
  eventEnvelopeSchema,
]);

export type Envelope = z.infer<typeof EnvelopeSchema>;

export function decodeEnvelope(value: unknown): Envelope {
  return EnvelopeSchema.parse(value);
}
