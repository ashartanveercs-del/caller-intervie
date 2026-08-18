import {
  useEffect,
  useRef,
  useState,
  type ChangeEvent,
  type FormEvent,
} from "react";
import { ArrowLeft, FileText, Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { useRuntime } from "../../app/RuntimeProvider";
import type { SessionRecord } from "../../platform";
import { CommandKind, PROTOCOL_VERSION, type Envelope } from "../../shared/protocol";
import {
  answerStyles,
  interviewBriefSchema,
  interviewTypes,
  type AnswerStyle,
  type DocumentReference,
  type InterviewBrief,
  type InterviewLanguages,
  type InterviewType,
} from "./briefSchema";
import { LanguageControls } from "./components/LanguageControls";
import { ReadinessSummary } from "./components/ReadinessSummary";
import "./prepare.css";

export type StoryOption = {
  id: string;
  title: string;
  detail?: string;
};

export type InterviewPreparePageProps = {
  storyOptions?: readonly StoryOption[];
};

type InterviewContext = InterviewBrief["context"];
type InterviewDocuments = InterviewBrief["documents"];

const initialLanguages: InterviewLanguages = {
  ui: "en",
  input: "auto",
  response: "en",
  review: "en",
};

let lastCommandSequence = -1;

const interviewTypeLabels: Record<InterviewType, string> = {
  mixed: "Mixed interview",
  behavioral: "Behavioral",
  technical: "Technical",
  coding: "Coding",
  "system-design": "System design",
  case: "Case interview",
  phone: "Phone screen",
  panel: "Panel",
  "one-way": "One-way interview",
};

const answerStyleLabels: Record<AnswerStyle, string> = {
  concise: "Concise bullets",
  natural: "Natural spoken response",
  star: "STAR response",
  detailed: "Detailed response",
  technical: "Technical explanation",
  coding: "Coding plan",
  "system-design": "System-design structure",
  case: "Case framework",
  executive: "Executive summary",
};

export function InterviewPreparePage({ storyOptions = [] }: InterviewPreparePageProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { platform, send, store } = useRuntime();
  const [context, setContext] = useState<InterviewContext>({});
  const [documents, setDocuments] = useState<InterviewDocuments>({});
  const [storyIds, setStoryIds] = useState<string[]>([]);
  const [interviewType, setInterviewType] = useState<InterviewType>("mixed");
  const [answerStyle, setAnswerStyle] = useState<AnswerStyle>("concise");
  const [answerGuidance, setAnswerGuidance] = useState("");
  const [languages, setLanguages] = useState<InterviewLanguages>(initialLanguages);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const errorRef = useRef<HTMLDivElement>(null);

  const brief = createBrief({
    context,
    documents,
    storyIds,
    interviewType,
    answerStyle,
    answerGuidance,
    languages,
  });

  useEffect(() => {
    if (submitError) errorRef.current?.focus();
  }, [submitError]);

  const updateContext = (field: keyof InterviewContext, value: string) => {
    setContext((current) => ({ ...current, [field]: value || undefined }));
  };

  const updateDocument = (
    field: keyof InterviewDocuments,
    event: ChangeEvent<HTMLInputElement>,
  ) => {
    const file = event.currentTarget.files?.[0];
    setDocuments((current) => ({
      ...current,
      [field]: file ? documentReference(file) : undefined,
    }));
  };

  const toggleStory = (storyId: string, selected: boolean) => {
    setStoryIds((current) => selected
      ? [...current, storyId]
      : current.filter((candidate) => candidate !== storyId));
  };

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const parsed = interviewBriefSchema.safeParse(brief);
    if (!parsed.success) {
      setSubmitError(t("prepare.errors.invalidBrief", {
        defaultValue: "Review the highlighted preparation details and try again.",
      }));
      return;
    }

    setIsSubmitting(true);
    setSubmitError(null);
    let createdSession: SessionRecord | null = null;
    try {
      createdSession = await platform.createSession({
        mode: "interview",
        uiLanguage: parsed.data.languages.ui,
        inputLanguage: parsed.data.languages.input,
        responseLanguage: parsed.data.languages.response,
        reviewLanguage: parsed.data.languages.review,
      });
      await platform.saveSessionBrief({ sessionId: createdSession.id, brief: parsed.data });
      await send(buildSessionStartCommand(createdSession));
      store.getState().beginSession({ ...createdSession, brief: parsed.data });
      store.getState().setLanguages(parsed.data.languages);
      navigate(`/live/${createdSession.id}`);
    } catch (error) {
      if (createdSession) {
        try {
          await platform.deleteSession(createdSession.id);
        } catch {
          // The original startup error remains the actionable failure.
        }
      }
      setSubmitError(startupErrorMessage(error, t));
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <div className="prepare-page">
      <header className="prepare-page__header">
        <button
          aria-label={t("prepare.back", { defaultValue: "Back to Home" })}
          onClick={() => navigate("/")}
          title={t("prepare.back", { defaultValue: "Back to Home" })}
          type="button"
        >
          <ArrowLeft aria-hidden="true" size={18} strokeWidth={1.8} />
        </button>
        <div>
          <p>{t("prepare.interview.eyebrow", { defaultValue: "Interview" })}</p>
          <h2>{t("prepare.interview.title", { defaultValue: "Prepare for your interview" })}</h2>
          <p>{t("prepare.interview.description", {
            defaultValue: "Add what you know now. Every context field is optional for Quick Start.",
          })}</p>
        </div>
      </header>

      {submitError ? (
        <div className="prepare-page__error" ref={errorRef} role="alert" tabIndex={-1}>
          <strong>{t("prepare.errors.title", { defaultValue: "Interview could not start" })}</strong>
          <p>{submitError}</p>
        </div>
      ) : null}

      <form className="prepare-page__layout" onSubmit={(event) => void submit(event)}>
        <div className="prepare-page__form">
          <section aria-labelledby="prepare-context-title">
            <header>
              <p>{t("prepare.steps.context", { defaultValue: "1. Context" })}</p>
              <h3 id="prepare-context-title">{t("prepare.context.title", { defaultValue: "Interview context" })}</h3>
            </header>
            <div className="prepare-page__field-grid prepare-page__field-grid--three">
              <div className="prepare-page__field">
                <label htmlFor="prepare-role">{t("prepare.context.role", { defaultValue: "Target role" })}</label>
                <input
                  autoComplete="organization-title"
                  id="prepare-role"
                  maxLength={160}
                  onChange={(event) => updateContext("role", event.currentTarget.value)}
                  placeholder={t("prepare.context.rolePlaceholder", { defaultValue: "e.g. Senior product designer" })}
                  value={context.role ?? ""}
                />
              </div>

              <div className="prepare-page__field">
                <label htmlFor="prepare-company">{t("prepare.context.company", { defaultValue: "Company" })}</label>
                <input
                  autoComplete="organization"
                  id="prepare-company"
                  maxLength={160}
                  onChange={(event) => updateContext("company", event.currentTarget.value)}
                  placeholder={t("prepare.context.companyPlaceholder", { defaultValue: "e.g. Acme" })}
                  value={context.company ?? ""}
                />
              </div>

              <div className="prepare-page__field">
                <label htmlFor="prepare-stage">{t("prepare.context.stage", { defaultValue: "Interview stage" })}</label>
                <input
                  id="prepare-stage"
                  maxLength={160}
                  onChange={(event) => updateContext("stage", event.currentTarget.value)}
                  placeholder={t("prepare.context.stagePlaceholder", { defaultValue: "e.g. Hiring manager round" })}
                  value={context.stage ?? ""}
                />
              </div>
            </div>
          </section>

          <section aria-labelledby="prepare-knowledge-title">
            <header>
              <p>{t("prepare.steps.knowledge", { defaultValue: "2. Knowledge" })}</p>
              <h3 id="prepare-knowledge-title">{t("prepare.knowledge.title", { defaultValue: "Ground your answers" })}</h3>
            </header>
            <div className="prepare-page__document-grid">
              <div className="prepare-page__document">
                <label htmlFor="prepare-resume">
                  <FileText aria-hidden="true" size={16} strokeWidth={1.8} />
                  {t("prepare.knowledge.resume", { defaultValue: "Resume" })}
                </label>
                <input
                  accept=".pdf,.doc,.docx,.txt"
                  id="prepare-resume"
                  onChange={(event) => updateDocument("resume", event)}
                  type="file"
                />
              </div>

              <div className="prepare-page__document">
                <label htmlFor="prepare-job-description">
                  <FileText aria-hidden="true" size={16} strokeWidth={1.8} />
                  {t("prepare.knowledge.jobDescription", { defaultValue: "Job description" })}
                </label>
                <input
                  accept=".pdf,.doc,.docx,.txt"
                  id="prepare-job-description"
                  onChange={(event) => updateDocument("jobDescription", event)}
                  type="file"
                />
              </div>
            </div>

            <fieldset className="prepare-page__stories">
              <legend>{t("prepare.knowledge.stories", { defaultValue: "Story vault" })}</legend>
              {storyOptions.length === 0 ? (
                <p>{t("prepare.knowledge.noStories", {
                  defaultValue: "No saved stories are available yet. Quick Start still works.",
                })}</p>
              ) : (
                storyOptions.map((story) => (
                  <label key={story.id}>
                    <input
                      checked={storyIds.includes(story.id)}
                      onChange={(event) => toggleStory(story.id, event.currentTarget.checked)}
                      type="checkbox"
                    />
                    <span>
                      <strong>{story.title}</strong>
                      {story.detail ? <small>{story.detail}</small> : null}
                    </span>
                  </label>
                ))
              )}
            </fieldset>
          </section>

          <section aria-labelledby="prepare-briefing-title">
            <header>
              <p>{t("prepare.steps.briefing", { defaultValue: "3. Briefing" })}</p>
              <h3 id="prepare-briefing-title">{t("prepare.briefing.title", { defaultValue: "Shape your guidance" })}</h3>
            </header>

            <div className="prepare-page__field-grid prepare-page__field-grid--two">
              <div className="prepare-page__field">
                <label htmlFor="prepare-interview-type">
                  {t("prepare.briefing.interviewType", { defaultValue: "Interview type" })}
                </label>
                <select
                  id="prepare-interview-type"
                  onChange={(event) => setInterviewType(event.currentTarget.value as InterviewType)}
                  value={interviewType}
                >
                  {interviewTypes.map((type) => (
                    <option key={type} value={type}>
                      {t(`prepare.interviewTypes.${type}`, { defaultValue: interviewTypeLabels[type] })}
                    </option>
                  ))}
                </select>
              </div>

              <div className="prepare-page__field">
                <label htmlFor="prepare-answer-style">
                  {t("prepare.briefing.answerStyle", { defaultValue: "Answer style" })}
                </label>
                <select
                  id="prepare-answer-style"
                  onChange={(event) => setAnswerStyle(event.currentTarget.value as AnswerStyle)}
                  value={answerStyle}
                >
                  {answerStyles.map((style) => (
                    <option key={style} value={style}>
                      {t(`prepare.answerStyles.${style}`, { defaultValue: answerStyleLabels[style] })}
                    </option>
                  ))}
                </select>
              </div>
            </div>

            <div className="prepare-page__field prepare-page__field--wide">
              <label htmlFor="prepare-answer-guidance">
                {t("prepare.briefing.guidance", { defaultValue: "Custom guidance" })}
              </label>
              <textarea
                id="prepare-answer-guidance"
                maxLength={500}
                onChange={(event) => setAnswerGuidance(event.currentTarget.value)}
                placeholder={t("prepare.briefing.guidancePlaceholder", {
                  defaultValue: "Optional tone, structure, or topics to emphasize",
                })}
                rows={3}
                value={answerGuidance}
              />
            </div>

            <LanguageControls disabled={isSubmitting} onChange={setLanguages} value={languages} />
          </section>
        </div>

        <aside className="prepare-page__summary">
          <ReadinessSummary brief={brief} />
          <div>
            <Sparkles aria-hidden="true" size={18} strokeWidth={1.8} />
            <p>{t("prepare.quickStart", {
              defaultValue: "Quick Start uses your language and format choices even without added context.",
            })}</p>
          </div>
          <button disabled={isSubmitting} type="submit">
            {isSubmitting
              ? t("prepare.starting", { defaultValue: "Starting..." })
              : t("prepare.start", { defaultValue: "Start interview" })}
          </button>
        </aside>
      </form>
    </div>
  );
}

type BriefInput = {
  context: InterviewContext;
  documents: InterviewDocuments;
  storyIds: string[];
  interviewType: InterviewType;
  answerStyle: AnswerStyle;
  answerGuidance: string;
  languages: InterviewLanguages;
};

function createBrief(input: BriefInput): InterviewBrief {
  return {
    schemaVersion: 1,
    mode: "interview",
    context: compactContext(input.context),
    documents: input.documents,
    storyIds: input.storyIds,
    interviewType: input.interviewType,
    answerStyle: input.answerStyle,
    ...(input.answerGuidance.trim() ? { answerGuidance: input.answerGuidance.trim() } : {}),
    languages: input.languages,
  };
}

function compactContext(context: InterviewContext): InterviewContext {
  return {
    ...(context.role?.trim() ? { role: context.role.trim() } : {}),
    ...(context.company?.trim() ? { company: context.company.trim() } : {}),
    ...(context.stage?.trim() ? { stage: context.stage.trim() } : {}),
  };
}

function documentReference(file: File): DocumentReference {
  return {
    name: file.name,
    sizeBytes: file.size,
    ...(file.type ? { mediaType: file.type } : {}),
    ...(file.lastModified >= 0 ? { lastModifiedMs: file.lastModified } : {}),
  };
}

export function buildSessionStartCommand(session: SessionRecord): Envelope {
  const timestampMs = Date.now();
  lastCommandSequence = Math.max(timestampMs, lastCommandSequence + 1);
  return {
    version: PROTOCOL_VERSION,
    id: crypto.randomUUID(),
    session_id: session.id,
    sequence: lastCommandSequence,
    timestamp_ms: timestampMs,
    kind: CommandKind.SESSION_START,
    payload: {
      mode: session.mode,
      input_language: session.inputLanguage,
      response_language: session.responseLanguage,
      review_language: session.reviewLanguage,
      you_source: "mic",
      brief_id: session.id,
    },
    correlation_id: null,
  };
}

type Translate = ReturnType<typeof useTranslation>["t"];

function startupErrorMessage(error: unknown, t: Translate): string {
  const code = typeof error === "object" && error !== null && "code" in error
    ? String(error.code)
    : "";
  if (code === "capture_protection_required") {
    return t("prepare.errors.captureProtection", {
      defaultValue: "Confirm screen-capture protection in the desktop app, then try again.",
    });
  }
  return t("prepare.errors.runtime", {
    defaultValue: "Check the runtime connection and provider configuration, then try again.",
  });
}
