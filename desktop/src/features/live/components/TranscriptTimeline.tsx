import { ChevronDown, ChevronUp } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { TranscriptTurn } from "../../../stores/sessionStore";

export type TranscriptTimelineProps = {
  turns: TranscriptTurn[];
};

export function TranscriptTimeline({ turns }: TranscriptTimelineProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const toggleLabel = expanded ? t("live.transcript.hide") : t("live.transcript.show");

  return (
    <section className={`transcript-timeline${expanded ? " transcript-timeline--expanded" : ""}`}>
      <button
        aria-label={toggleLabel}
        aria-controls="live-transcript-timeline"
        aria-expanded={expanded}
        className="transcript-timeline__toggle"
        onClick={() => setExpanded((value) => !value)}
        type="button"
      >
        {expanded ? <ChevronDown aria-hidden="true" size={17} /> : <ChevronUp aria-hidden="true" size={17} />}
        {toggleLabel}
        <span className="transcript-timeline__count">{turns.length}</span>
      </button>
      <div
        aria-label={t("live.transcript.timeline")}
        className="transcript-timeline__content"
        hidden={!expanded}
        id="live-transcript-timeline"
        role="region"
      >
        {turns.length > 0 ? (
          <ol className="transcript-timeline__list">
            {turns.map((turn) => (
              <li className="transcript-timeline__turn" key={turn.id}>
                <span className="transcript-timeline__speaker">{speakerLabel(turn.speakerRole, t)}</span>
                <p lang={turn.language && turn.language !== "und" ? turn.language : undefined}>{turn.text}</p>
                {!turn.isFinal ? <span className="transcript-timeline__partial">{t("live.transcript.live")}</span> : null}
              </li>
            ))}
          </ol>
        ) : (
          <p className="transcript-timeline__empty">{t("live.transcript.empty")}</p>
        )}
      </div>
    </section>
  );
}

type Translate = ReturnType<typeof useTranslation>["t"];

function speakerLabel(role: string | undefined, t: Translate): string {
  switch (role?.toLowerCase()) {
    case "interviewer": return t("live.transcript.speakers.interviewer");
    case "interviewee":
    case "candidate":
    case "you": return t("live.transcript.speakers.you");
    case "manual": return t("live.transcript.speakers.manual");
    default: return t("live.transcript.speakers.default");
  }
}
