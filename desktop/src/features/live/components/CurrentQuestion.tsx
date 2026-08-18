import { useTranslation } from "react-i18next";

export type CurrentQuestionProps = {
  isFinal?: boolean;
  isManual?: boolean;
  language?: string;
  text?: string;
};

export function CurrentQuestion({
  isFinal = true,
  isManual = false,
  language,
  text,
}: CurrentQuestionProps) {
  const { t } = useTranslation();
  const question = text?.trim();

  return (
    <section aria-labelledby="live-current-question" className="current-question">
      <div className="current-question__meta">
        <span>{t("live.question.current")}</span>
        {isManual ? <span className="current-question__source">{t("live.question.manual")}</span> : null}
      </div>
      <div aria-atomic="true" aria-live={isFinal ? "polite" : "off"}>
        <h2 id="live-current-question" lang={language && language !== "und" ? language : undefined}>
          {question || t("live.question.waiting")}
        </h2>
      </div>
      {!isFinal && question ? <p className="current-question__listening">{t("live.question.listening")}</p> : null}
    </section>
  );
}
