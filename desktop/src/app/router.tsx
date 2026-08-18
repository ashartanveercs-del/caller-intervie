import { createBrowserRouter, Outlet, type RouteObject } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { RotateCw } from "lucide-react";
import { HomePage } from "../features/home/HomePage";
import { LivePage } from "../features/live/LivePage";
import { InterviewPreparePage } from "../features/prepare/InterviewPreparePage";
import { ReviewPage } from "../features/review/ReviewPage";
import { AppShell } from "./AppShell";
import { useRuntime } from "./RuntimeProvider";

function AppLayout() {
  return (
    <AppShell>
      <Outlet />
    </AppShell>
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

  return <LivePage />;
}

export const routes: RouteObject[] = [
  {
    element: <AppLayout />,
    children: [
      { index: true, element: <HomePage /> },
      { path: "/prepare/interview", element: <InterviewPreparePage /> },
      { path: "/live/:sessionId", element: <LiveRoute /> },
      { path: "/review/:sessionId", element: <ReviewPage /> },
    ],
  },
];

export const router = createBrowserRouter(routes);
