import { useEffect, useRef, useState, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";
import { useRuntime } from "../../app/RuntimeProvider";
import type { SessionRecord } from "../../platform";
import "./home.css";
import { modeDefinitions, type ModeDefinition } from "./modes";

const startLabelFallbacks = {
  interview: "Start Interview",
  sales: "Start Sales Call",
  meeting: "Start Meeting",
  presentation: "Start Presentation",
} as const;

const plannedTitleFallbacks = {
  interview: "Interview is available",
  sales: "Sales Call is coming soon",
  meeting: "Meeting mode is coming soon",
  presentation: "Presentation mode is coming soon",
} as const;

export function HomePage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { platform } = useRuntime();
  const [plannedMode, setPlannedMode] = useState<ModeDefinition | null>(null);
  const [recentSessions, setRecentSessions] = useState<SessionRecord[]>([]);
  const [recentState, setRecentState] = useState<"loading" | "ready" | "error">("loading");
  const [recentRevision, setRecentRevision] = useState(0);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (!plannedMode) return;
    closeButtonRef.current?.focus();
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") closePlannedMode();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [plannedMode]);

  useEffect(() => {
    let active = true;
    setRecentState("loading");
    void platform.listSessions(5)
      .then((sessions) => {
        if (!active) return;
        setRecentSessions(sessions);
        setRecentState("ready");
      })
      .catch(() => {
        if (active) setRecentState("error");
      });
    return () => {
      active = false;
    };
  }, [platform, recentRevision]);

  const closePlannedMode = () => {
    setPlannedMode(null);
    window.setTimeout(() => triggerRef.current?.focus(), 0);
  };

  const activateMode = (mode: ModeDefinition, event: MouseEvent<HTMLButtonElement>) => {
    if (mode.releaseState === "available") {
      navigate(mode.prepareRoute);
      return;
    }
    triggerRef.current = event.currentTarget;
    setPlannedMode(mode);
  };

  return (
    <section aria-labelledby="home-title" className="home-page">
      <header className="home-page__header">
        <p className="home-page__eyebrow">{t("home.eyebrow", { defaultValue: "Conversation workspace" })}</p>
        <h2 id="home-title">{t("home.title", { defaultValue: "Choose how you want help" })}</h2>
        <p>{t("home.description", { defaultValue: "Prepare context, then get focused guidance while you talk." })}</p>
      </header>

      <div
        aria-label={t("home.modeActions", { defaultValue: "Start a conversation" })}
        className="home-page__modes"
      >
        {modeDefinitions.map((mode) => {
          const Icon = mode.icon;
          return (
            <div className="home-page__mode-item" key={mode.id}>
              <button
                className={`home-page__mode home-page__mode--${mode.releaseState}`}
                onClick={(event) => activateMode(mode, event)}
                type="button"
              >
                <span className="home-page__mode-icon">
                  <Icon aria-hidden="true" size={20} strokeWidth={1.8} />
                </span>
                <span>{t(mode.startLabelKey, { defaultValue: startLabelFallbacks[mode.id] })}</span>
              </button>
              <small>
                {mode.releaseState === "available"
                  ? t("home.available.label", { defaultValue: "Available" })
                  : t("home.planned.label", { defaultValue: "Planned" })}
              </small>
            </div>
          );
        })}
      </div>

      <section aria-labelledby="recent-sessions-title" className="home-page__recent">
        <div className="home-page__section-heading">
          <h3 id="recent-sessions-title">
            {t("home.recent.title", { defaultValue: "Recent sessions" })}
          </h3>
          <p>{t("home.recent.description", { defaultValue: "Resume active work or review a finished session." })}</p>
        </div>
        {recentState === "loading" ? (
          <p aria-live="polite">{t("home.recent.loading", { defaultValue: "Loading recent sessions..." })}</p>
        ) : recentState === "error" ? (
          <div className="home-page__recent-error" role="alert">
            <p>{t("home.recent.error", { defaultValue: "Recent sessions could not be loaded." })}</p>
            <button onClick={() => setRecentRevision((revision) => revision + 1)} type="button">
              {t("common.retry", { defaultValue: "Retry" })}
            </button>
          </div>
        ) : recentSessions.length === 0 ? (
          <p>{t("home.recent.empty", { defaultValue: "Start an interview to create your first session." })}</p>
        ) : (
          <ul className="home-page__session-list">
            {recentSessions.map((session) => {
              const mode = modeDefinitions.find((candidate) => candidate.id === session.mode);
              const modeName = mode
                ? t(mode.labelKey, { defaultValue: fallbackModeName(mode.id) })
                : session.mode;
              const isActive = session.status === "active";
              const action = isActive
                ? t("home.recent.resume", { defaultValue: "Resume" })
                : t("home.recent.review", { defaultValue: "Review" });
              const actionLabel = isActive
                ? t("home.recent.resumeLabel", { mode: modeName, defaultValue: `Resume ${modeName} session` })
                : t("home.recent.reviewLabel", { mode: modeName, defaultValue: `Review ${modeName} session` });
              return (
                <li key={session.id}>
                  <div>
                    <strong>{modeName}</strong>
                    <span className="home-page__status">
                      {t(`home.recent.status.${session.status}`, { defaultValue: session.status })}
                    </span>
                  </div>
                  <button
                    aria-label={actionLabel}
                    onClick={() => navigate(isActive ? `/live/${session.id}` : `/review/${session.id}`)}
                    type="button"
                  >
                    {action}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {plannedMode ? (
        <div className="home-page__sheet-backdrop" role="presentation">
          <aside
            aria-labelledby="planned-mode-title"
            aria-modal="true"
            className="home-page__sheet"
            role="dialog"
          >
            <h3 id="planned-mode-title">
              {t(`home.planned.${plannedMode.id}.title`, {
                defaultValue: plannedTitleFallbacks[plannedMode.id],
              })}
            </h3>
            <p>
              {t("home.planned.description", {
                defaultValue: "Interview is available now. This workflow is being prepared for a later release.",
              })}
            </p>
            <button onClick={closePlannedMode} ref={closeButtonRef} type="button">
              {t("common.close", { defaultValue: "Close" })}
            </button>
          </aside>
        </div>
      ) : null}
    </section>
  );
}

function fallbackModeName(mode: ModeDefinition["id"]): string {
  switch (mode) {
    case "interview": return "Interview";
    case "sales": return "Sales";
    case "meeting": return "Meeting";
    case "presentation": return "Presentation";
  }
}
