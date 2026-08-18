import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

export type LiveAnswer = {
  id: string;
  isComplete: boolean;
  text: string;
};

export type AnswerScaffoldProps = {
  announce?: boolean;
  answer?: LiveAnswer;
  expandLabel?: string;
  heading?: string;
  waitingForQuestion?: boolean;
};

export function AnswerScaffold({
  announce = false,
  answer,
  expandLabel,
  heading,
  waitingForQuestion = false,
}: AnswerScaffoldProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [announcement, setAnnouncement] = useState("");
  const points = useMemo(() => answerPoints(answer?.text ?? ""), [answer?.text]);

  useEffect(() => {
    setExpanded(false);
  }, [answer?.id]);

  useEffect(() => {
    if (!announce) return;
    setAnnouncement(answer?.isComplete ? t("live.answer.ready") : "");
  }, [announce, answer?.id, answer?.isComplete, t]);

  const visiblePoints = expanded ? points : points.slice(0, 3);
  const resolvedHeading = heading ?? t("live.answer.suggested");
  const resolvedExpandLabel = expandLabel ?? t("live.answer.expandDetails");

  return (
    <section aria-labelledby={`answer-${answer?.id ?? "pending"}`} className="answer-scaffold">
      <div className="answer-scaffold__heading">
        <h3 id={`answer-${answer?.id ?? "pending"}`}>{resolvedHeading}</h3>
        {answer && !answer.isComplete ? (
          <span className="answer-scaffold__streaming">{t("live.answer.generating")}</span>
        ) : null}
      </div>
      <div aria-live="off" className="answer-scaffold__body">
        {visiblePoints.length > 0 ? (
          <ul className="answer-scaffold__points">
            {visiblePoints.map((point, index) => <li key={`${answer?.id}-${index}`}>{point}</li>)}
          </ul>
        ) : (
          <p className="answer-scaffold__empty">
            {waitingForQuestion ? t("live.answer.waitingForQuestion") : t("live.answer.preparing")}
          </p>
        )}
        {points.length > 3 ? (
          <button
            aria-expanded={expanded}
            className="answer-scaffold__expand"
            onClick={() => setExpanded((value) => !value)}
            type="button"
          >
            {expanded ? t("live.answer.showConcise") : resolvedExpandLabel}
          </button>
        ) : null}
      </div>
      {announce ? (
        <span
          aria-label={t("live.answer.updates")}
          aria-live="polite"
          aria-atomic="true"
          className="live-page__visually-hidden"
          role="status"
        >
          {announcement}
        </span>
      ) : null}
    </section>
  );
}

function answerPoints(text: string): string[] {
  const lines = text
    .split(/\r?\n/)
    .map((line) => cleanPoint(line))
    .filter((line): line is string => Boolean(line));

  if (lines.length !== 1) return lines;

  const sentences = lines[0]
    .split(/(?<=[.!?])\s+/)
    .map((sentence) => sentence.trim())
    .filter(Boolean);
  return sentences.length > 1 ? sentences : lines;
}

function cleanPoint(line: string): string {
  return line
    .trim()
    .replace(/^#{1,6}\s+/, "")
    .replace(/^[-*+]\s+/, "")
    .replace(/^\d+[.)]\s+/, "")
    .trim();
}
