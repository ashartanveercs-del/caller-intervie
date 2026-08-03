import { createBrowserRouter, Outlet, useParams } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { AppShell } from "./AppShell";

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

export const router = createBrowserRouter([
  {
    element: <AppLayout />,
    children: [
      { index: true, element: <HomeRoute /> },
      { path: "/prepare/:mode", element: <PrepareRoute /> },
      { path: "/live/:sessionId", element: <LiveRoute /> },
      { path: "/review/:sessionId", element: <ReviewRoute /> },
    ],
  },
]);
