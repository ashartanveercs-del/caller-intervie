import { createBrowserRouter, Outlet, type RouteObject, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { RotateCw } from "lucide-react";
import { AppShell } from "./AppShell";
import { useRuntime } from "./RuntimeProvider";

function AppLayout() {
  return (
    <AppShell>
      <Outlet />
    </AppShell>
  );
}

function HomeRoute() {
  const { t } = useTranslation();

  return <RouteView description={t("home.description")} title={t("home.title")} />;
}

function PrepareRoute() {
  const { mode = "interview" } = useParams();
  const { t } = useTranslation();

  return (
    <RouteView
      description={t("prepare.description")}
      title={t("prepare.title", { mode: t(`modes.${mode}`) })}
    />
  );
}

function LiveRoute() {
  const { t } = useTranslation();
  const { captureProtection, retryCaptureProtection } = useRuntime();

  if (captureProtection.state !== "protected") {
    const message = captureProtection.state === "unsupported"
      ? t("captureProtection.desktopAppRequired")
      : captureProtection.state === "unavailable"
        ? t("captureProtection.liveBlocked")
        : t("captureProtection.applying");

    return (
      <section className="live-blocker" role="alert">
        <p>{message}</p>
        {captureProtection.state === "unavailable" ? (
          <button
            onClick={() => void retryCaptureProtection()}
            title={t("captureProtection.retry")}
            type="button"
          >
            <RotateCw aria-hidden="true" size={16} strokeWidth={1.8} />
            {t("captureProtection.retry")}
          </button>
        ) : null}
      </section>
    );
  }

  return <RouteView description={t("live.description")} title={t("live.title")} />;
}

function ReviewRoute() {
  const { t } = useTranslation();

  return <RouteView description={t("review.description")} title={t("review.title")} />;
}

type RouteViewProps = {
  title: string;
  description: string;
};

function RouteView({ title, description }: RouteViewProps) {
  return (
    <section className="route-view">
      <h2>{title}</h2>
      <p>{description}</p>
    </section>
  );
}

export const routes: RouteObject[] = [
  {
    element: <AppLayout />,
    children: [
      { index: true, element: <HomeRoute /> },
      { path: "/prepare/:mode", element: <PrepareRoute /> },
      { path: "/live/:sessionId", element: <LiveRoute /> },
      { path: "/review/:sessionId", element: <ReviewRoute /> },
    ],
  },
];

export const router = createBrowserRouter(routes);
