import { ArrowLeft, Languages, MessageSquareText, Pin, StickyNote, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate, useParams } from "react-router-dom";
import { useRuntime } from "../../app/RuntimeProvider";
import { buildReviewModel, type ReviewModel } from "./reviewModels";
import "./review.css";

type ReviewState =
  | { state: "loading" }
  | { state: "ready"; model: ReviewModel }
  | { state: "error"; message: string };

export function ReviewPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { sessionId } = useParams();
  const { platform } = useRuntime();
  const [review, setReview] = useState<ReviewState>({ state: "loading" });

  useEffect(() => {
    let active = true;
    if (!sessionId) {
      setReview({ state: "error", message: t("review.errors.missingSession", { defaultValue: "This review link is missing a session." }) });
      return () => { active = false; };
    }

    setReview({ state: "loading" });
    void Promise.all([
      platform.getSession(sessionId),
      platform.getTimeline(sessionId),
      platform.getRequestTurnAssociations(sessionId),
    ])
      .then(([session, timeline, associations]) => {
        if (active) setReview({ state: "ready", model: buildReviewModel(session, timeline, associations) });
      })
      .catch(() => {
        if (active) {
          setReview({
            state: "error",
            message: t("review.errors.load", { defaultValue: "The saved session could not be loaded." }),
          });
        }
      });

    return () => { active = false; };
  }, [platform, sessionId, t]);

  if (review.state === "loading") {
    return <p aria-live="polite" className="review-page__loading">{t("review.loading", { defaultValue: "Loading review..." })}</p>;
  }

  if (review.state === "error") {
    return (
      <section className="review-page__error" role="alert">
        <h2>{t("review.errors.title", { defaultValue: "Review unavailable" })}</h2>
        <p>{review.message}</p>
        <button onClick={() => navigate("/")} type="button">{t("review.backHome", { defaultValue: "Back to Home" })}</button>
      </section>
    );
  }

  const { model } = review;
  return (
    <section aria-labelledby="review-page-title" className="review-page">
      <header className="review-page__header">
        <button
          aria-label={t("review.backHome", { defaultValue: "Back to Home" })}
          className="review-page__back"
          onClick={() => navigate("/")}
          title={t("review.backHome", { defaultValue: "Back to Home" })}
          type="button"
        >
          <ArrowLeft aria-hidden="true" size={18} strokeWidth={1.8} />
        </button>
        <div className="review-page__title">
          <p>{t("review.eyebrow", { defaultValue: "Saved session" })}</p>
          <h2 id="review-page-title">{t("review.title", { defaultValue: "Interview review" })}</h2>
          {model.role || model.company ? (
            <p>{[model.role, model.company].filter(Boolean).join(" / ")}</p>
          ) : null}
        </div>
        <span className={`review-page__status review-page__status--${model.session.status}`}>
          {statusLabel(model.session.status, t)}
        </span>
      </header>

      <dl className="review-page__metadata">
        <div>
          <dt>{t("review.metadata.mode", { defaultValue: "Mode" })}</dt>
          <dd>{model.session.mode}</dd>
        </div>
        <div>
          <dt>{t("review.metadata.spokenLanguage", { defaultValue: "Spoken language" })}</dt>
          <dd>{model.session.inputLanguage}</dd>
        </div>
        <div>
          <dt>{t("review.metadata.suggestionLanguage", { defaultValue: "Suggestion language" })}</dt>
          <dd>{model.session.responseLanguage}</dd>
        </div>
        <div>
          <dt>{t("review.metadata.reviewLanguage", { defaultValue: "Review language" })}</dt>
          <dd>{model.session.reviewLanguage}</dd>
        </div>
      </dl>

      <div className="review-page__primary">
        <section aria-label={t("review.transcript.title", { defaultValue: "Original transcript" })} className="review-page__section">
          <header>
            <MessageSquareText aria-hidden="true" size={18} strokeWidth={1.8} />
            <div>
              <h3>{t("review.transcript.title", { defaultValue: "Original transcript" })}</h3>
              <p>{t("review.transcript.description", { defaultValue: "What was heard, in its original language." })}</p>
            </div>
          </header>
          {model.transcript.length > 0 ? (
            <ol className="review-page__timeline">
              {model.transcript.map((turn) => (
                <li key={turn.id}>
                  <div>
                    <strong>{speakerLabel(turn.speaker)}</strong>
                    {turn.language ? <span>{turn.language}</span> : null}
                  </div>
                  <p>{turn.text}</p>
                </li>
              ))}
            </ol>
          ) : <p className="review-page__empty">{t("review.transcript.empty", { defaultValue: "No final transcript was saved." })}</p>}
        </section>

        <section aria-label={t("review.suggestions.title", { defaultValue: "AI suggestions" })} className="review-page__section">
          <header>
            <MessageSquareText aria-hidden="true" size={18} strokeWidth={1.8} />
            <div>
              <h3>{t("review.suggestions.title", { defaultValue: "AI suggestions" })}</h3>
              <p>{t("review.suggestions.description", { defaultValue: "Generated guidance, kept separate from the original transcript." })}</p>
            </div>
          </header>
          {model.suggestions.length > 0 ? (
            <ol className="review-page__suggestions">
              {model.suggestions.map((suggestion) => (
                <li key={suggestion.id}>
                  {suggestion.question ? <h4>{suggestion.question}</h4> : null}
                  <p>{suggestion.text}</p>
                </li>
              ))}
            </ol>
          ) : <p className="review-page__empty">{t("review.suggestions.empty", { defaultValue: "No completed suggestions were saved." })}</p>}
        </section>
      </div>

      {model.notes.length > 0 || model.pins.length > 0 ? (
        <div className="review-page__artifacts">
          {model.notes.length > 0 ? (
            <section
              aria-label={t("review.notes.title", { defaultValue: "Session notes" })}
              className="review-page__section"
            >
              <header>
                <StickyNote aria-hidden="true" size={18} strokeWidth={1.8} />
                <div>
                  <h3>{t("review.notes.title", { defaultValue: "Session notes" })}</h3>
                  <p>{t("review.notes.description", { defaultValue: "Notes you saved while the conversation was live." })}</p>
                </div>
              </header>
              <ol className="review-page__notes">
                {model.notes.map((note) => <li key={note.id}>{note.text}</li>)}
              </ol>
            </section>
          ) : null}

          {model.pins.length > 0 ? (
            <section
              aria-label={t("review.pins.title", { defaultValue: "Pinned guidance" })}
              className="review-page__section"
            >
              <header>
                <Pin aria-hidden="true" size={18} strokeWidth={1.8} />
                <div>
                  <h3>{t("review.pins.title", { defaultValue: "Pinned guidance" })}</h3>
                  <p>{t("review.pins.description", { defaultValue: "Questions and completed guidance you marked for follow-up." })}</p>
                </div>
              </header>
              <ol className="review-page__pins">
                {model.pins.map((pin) => (
                  <li key={pin.id}>
                    <h4>{pin.question}</h4>
                    <p>{pin.suggestion}</p>
                  </li>
                ))}
              </ol>
            </section>
          ) : null}
        </div>
      ) : null}

      {model.translations.length > 0 ? (
        <section aria-label={t("review.translations.title", { defaultValue: "Generated translations" })} className="review-page__section review-page__section--full">
          <header>
            <Languages aria-hidden="true" size={18} strokeWidth={1.8} />
            <div>
              <h3>{t("review.translations.title", { defaultValue: "Generated translations" })}</h3>
              <p>{t("review.translations.description", { defaultValue: "Generated translation for review; the original remains unchanged above." })}</p>
            </div>
          </header>
          <ol className="review-page__translations">
            {model.translations.map((translation) => (
              <li key={translation.id}>
                <span>{[translation.sourceLanguage, translation.targetLanguage].filter(Boolean).join(" to ")}</span>
                <p>{translation.translated}</p>
              </li>
            ))}
          </ol>
        </section>
      ) : null}

      {model.interruptions.length > 0 ? (
        <section aria-label={t("review.interruptions.title", { defaultValue: "Runtime interruptions" })} className="review-page__section review-page__section--warning">
          <header>
            <TriangleAlert aria-hidden="true" size={18} strokeWidth={1.8} />
            <div>
              <h3>{t("review.interruptions.title", { defaultValue: "Runtime interruptions" })}</h3>
              <p>{t("review.interruptions.description", { defaultValue: "Issues recorded while the session was active." })}</p>
            </div>
          </header>
          <ul>
            {model.interruptions.map((interruption) => <li key={interruption.id}>{interruption.message}</li>)}
          </ul>
        </section>
      ) : null}
    </section>
  );
}

type Translate = ReturnType<typeof useTranslation>["t"];

function statusLabel(status: ReviewModel["session"]["status"], t: Translate): string {
  switch (status) {
    case "active": return t("review.status.active", { defaultValue: "Active" });
    case "completed": return t("review.status.completed", { defaultValue: "Completed" });
    case "interrupted": return t("review.status.interrupted", { defaultValue: "Interrupted" });
  }
}

function speakerLabel(speaker: string): string {
  return speaker.replace(/[_-]+/g, " ").replace(/^./, (character) => character.toUpperCase());
}
