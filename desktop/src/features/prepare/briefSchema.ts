import { z } from "zod";

export const interviewTypes = [
  "mixed",
  "behavioral",
  "technical",
  "coding",
  "system-design",
  "case",
  "phone",
  "panel",
  "one-way",
] as const;

export const answerStyles = [
  "concise",
  "natural",
  "star",
  "detailed",
  "technical",
  "coding",
  "system-design",
  "case",
  "executive",
] as const;

const languageCodeSchema = z.string().trim().min(2).max(35);
const optionalContextText = z.string().trim().min(1).max(160).optional();

export const documentReferenceSchema = z.object({
  name: z.string().trim().min(1).max(260),
  sizeBytes: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
  mediaType: z.string().trim().max(120).optional(),
  lastModifiedMs: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER).optional(),
}).strict();

export const interviewBriefSchema = z.object({
  schemaVersion: z.literal(1),
  mode: z.literal("interview"),
  context: z.object({
    role: optionalContextText,
    company: optionalContextText,
    stage: optionalContextText,
  }).strict(),
  documents: z.object({
    resume: documentReferenceSchema.optional(),
    jobDescription: documentReferenceSchema.optional(),
  }).strict(),
  storyIds: z.array(z.string().trim().min(1).max(120)).max(20)
    .refine((storyIds) => new Set(storyIds).size === storyIds.length, "Story references must be unique"),
  interviewType: z.enum(interviewTypes),
  answerStyle: z.enum(answerStyles),
  answerGuidance: z.string().trim().max(500).optional(),
  languages: z.object({
    ui: languageCodeSchema,
    input: languageCodeSchema,
    response: languageCodeSchema,
    review: languageCodeSchema,
  }).strict(),
}).strict();

export type DocumentReference = z.infer<typeof documentReferenceSchema>;
export type InterviewBrief = z.infer<typeof interviewBriefSchema>;
export type InterviewType = InterviewBrief["interviewType"];
export type AnswerStyle = InterviewBrief["answerStyle"];
export type InterviewLanguages = InterviewBrief["languages"];
