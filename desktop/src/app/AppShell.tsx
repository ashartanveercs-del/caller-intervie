import { Settings, UserRound } from "lucide-react";
import { type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { HealthIndicator } from "../components/HealthIndicator";
import { IconButton } from "../components/IconButton";
import "../i18n";

type AppShellProps = {
  children?: ReactNode;
};

const modes = [
  { key: "interview", href: "/" },
  { key: "sales", href: "/prepare/sales" },
  { key: "meeting", href: "/prepare/meeting" },
  { key: "presentation", href: "/prepare/presentation" },
] as const;

function isCurrentMode(href: string) {
  const pathname = window.location.pathname;
  return href === "/" ? pathname === "/" : pathname.startsWith(href);
}

export function AppShell({ children }: AppShellProps) {
  const { t } = useTranslation();

  return (
    <div className="app-shell">
      <header className="app-shell__topbar">
        <h1 className="app-shell__brand">{t("app.name")}</h1>
        <nav aria-label={t("app.navigation")} className="app-shell__modes">
          {modes.map(({ key, href }) => (
            <a
              aria-current={isCurrentMode(href) ? "page" : undefined}
              className="app-shell__mode-link"
              href={href}
              key={key}
            >
              {t(`modes.${key}`)}
            </a>
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
      </footer>
    </div>
  );
}
