import { Clipboard, FilePlus2, Pause, Pin, PinOff, Play, Square } from "lucide-react";
import { useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { IconButton } from "../../../components/IconButton";

export type AnswerFormat = "chat" | "summary";

export type LiveToolbarProps = {
  answerFormat: AnswerFormat;
  artifactBusy: boolean;
  canCopy: boolean;
  canPin: boolean;
  ending: boolean;
  listening: boolean;
  noteOpen: boolean;
  pinned: boolean;
  onAnswerFormatChange: (format: AnswerFormat) => void;
  onCopy: () => void;
  onEnd: () => void;
  onManualPrompt: (text: string) => Promise<void>;
  onNote: () => void;
  onPin: () => void;
  onSetListening: (enabled: boolean) => Promise<void>;
};

export function LiveToolbar({
  answerFormat,
  artifactBusy,
  canCopy,
  canPin,
  ending,
  listening,
  noteOpen,
  pinned,
  onAnswerFormatChange,
  onCopy,
  onEnd,
  onManualPrompt,
  onNote,
  onPin,
  onSetListening,
}: LiveToolbarProps) {
  const { t } = useTranslation();
  const [manualPrompt, setManualPrompt] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [submissionError, setSubmissionError] = useState("");

  const setListeningState = async (enabled: boolean) => {
    setSubmissionError("");
    try {
      await onSetListening(enabled);
    } catch {
      setSubmissionError(t("live.errors.listeningControl"));
    }
  };

  const submitPrompt = async (event: FormEvent) => {
    event.preventDefault();
    const prompt = manualPrompt.trim();
    if (!prompt || submitting) return;
    setSubmitting(true);
    setSubmissionError("");
    try {
      await onManualPrompt(prompt);
      setManualPrompt("");
    } catch {
      setSubmissionError(t("live.errors.manualPrompt"));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div aria-label={t("live.toolbar.label")} className="live-toolbar" role="toolbar">
      <IconButton
        disabled={ending}
        icon={listening ? Pause : Play}
        label={listening ? t("live.toolbar.pause") : t("live.toolbar.resume")}
        onClick={() => { void setListeningState(!listening); }}
      />
      <form className="live-toolbar__prompt" onSubmit={(event) => void submitPrompt(event)}>
        <label className="live-page__visually-hidden" htmlFor="live-manual-prompt">
          {t("live.toolbar.manual.label")}
        </label>
        <input
          aria-label={t("live.toolbar.manual.label")}
          disabled={ending || submitting}
          id="live-manual-prompt"
          onChange={(event) => setManualPrompt(event.target.value)}
          placeholder={t("live.toolbar.manual.placeholder")}
          type="text"
          value={manualPrompt}
        />
        <button disabled={ending || submitting || !manualPrompt.trim()} type="submit">
          {t("live.toolbar.manual.submit")}
        </button>
      </form>
      <div aria-label={t("live.toolbar.answerFormat.label")} className="live-toolbar__segments" role="radiogroup">
        <button
          aria-checked={answerFormat === "chat"}
          className={answerFormat === "chat" ? "live-toolbar__segment live-toolbar__segment--active" : "live-toolbar__segment"}
          disabled={ending}
          onClick={() => onAnswerFormatChange("chat")}
          role="radio"
          type="button"
        >
          {t("live.toolbar.answerFormat.concise")}
        </button>
        <button
          aria-checked={answerFormat === "summary"}
          className={answerFormat === "summary" ? "live-toolbar__segment live-toolbar__segment--active" : "live-toolbar__segment"}
          disabled={ending}
          onClick={() => onAnswerFormatChange("summary")}
          role="radio"
          type="button"
        >
          {t("live.toolbar.answerFormat.detailed")}
        </button>
      </div>
      <div className="live-toolbar__actions">
        <IconButton
          disabled={artifactBusy || ending}
          icon={FilePlus2}
          label={noteOpen ? t("live.toolbar.closeNote") : t("live.toolbar.addNote")}
          onClick={onNote}
          pressed={noteOpen}
        />
        <IconButton
          disabled={artifactBusy || ending || !canPin}
          icon={pinned ? PinOff : Pin}
          label={pinned ? t("live.toolbar.unpin") : t("live.toolbar.pin")}
          onClick={onPin}
          pressed={pinned}
        />
        <IconButton
          disabled={!canCopy || ending}
          icon={Clipboard}
          label={t("live.toolbar.copy")}
          onClick={onCopy}
        />
        <IconButton
          disabled={artifactBusy || ending}
          icon={Square}
          label={ending ? t("live.toolbar.ending") : t("live.toolbar.end")}
          onClick={onEnd}
        />
      </div>
      {submissionError ? (
        <p aria-label={t("live.status.actionFailed")} className="live-toolbar__error" role="alert">
          {submissionError}
        </p>
      ) : null}
    </div>
  );
}
