import { Settings, UserRound } from "lucide-react";
import { type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { NavLink } from "react-router-dom";
import { HealthIndicator } from "../components/HealthIndicator";
import { CaptureProtectionIndicator } from "../components/CaptureProtectionIndicator";
import { IconButton } from "../components/IconButton";
import "../i18n";
import { useRuntime } from "./RuntimeProvider";

type AppShellProps = {
  children?: ReactNode;
};

const modes = [
  { key: "interview", href: "/", releaseState: "available" },
  { key: "sales", href: "/prepare/sales", releaseState: "planned" },
  { key: "meeting", href: "/prepare/meeting", releaseState: "planned" },
  { key: "presentation", href: "/prepare/presentation", releaseState: "planned" },
] as const;

export function AppShell({ children }: AppShellProps) {
  const { t } = useTranslation();
  const { captureProtection, retryCaptureProtection } = useRuntime();

  return (
    <div className="app-shell">
      <header className="app-shell__topbar">
        <h1 className="app-shell__brand">{t("app.name")}</h1>
        <nav aria-label={t("app.navigation")} className="app-shell__modes">
          {modes.map(({ key, href, releaseState }) => releaseState === "available" ? (
            <NavLink className="app-shell__mode-link" key={key} to={href}>
              {t(`modes.${key}`)}
            </NavLink>
          ) : (
            <span
              aria-disabled="true"
              className="app-shell__mode-link app-shell__mode-link--disabled"
              key={key}
              role="link"
              title={t("home.planned.label")}
            >
              {t(`modes.${key}`)}
            </span>
          ))}
        </nav>
        <div className="app-shell__actions">
          <IconButton
            icon={UserRound}
            label={t("app.account")}
            tooltip={t("app.accountTooltip")}
          />
          <IconButton
            icon={Settings}
            label={t("app.settings")}
            tooltip={t("app.settingsTooltip")}
          />
        </div>
      </header>
      <main className="app-shell__content">
        {children ?? (
          <section className="route-view">
            <h2>{t("home.title")}</h2>
            <p>{t("home.description")}</p>
          </section>
        )}
      </main>
      <footer className="app-shell__statusbar">
        <HealthIndicator label={t("health.connected")} state="healthy" />
        <CaptureProtectionIndicator
          captureProtection={captureProtection}
          onRetry={() => void retryCaptureProtection()}
        />
      </footer>
    </div>
  );
}
