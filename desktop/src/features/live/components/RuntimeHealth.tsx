import { useTranslation } from "react-i18next";
import type {
  DependencyHealth,
  RuntimeHealth as RuntimeHealthState,
  RuntimeHealthStatus,
} from "../../../stores/sessionStore";

export type RuntimeHealthProps = {
  health: RuntimeHealthState;
  onRetry: () => void;
};

const dependencies: Array<{
  key: "microphone" | "systemAudio" | "speechProvider" | "modelProvider";
  labelKey: string;
}> = [
  { key: "microphone", labelKey: "live.health.dependencies.microphone" },
  { key: "systemAudio", labelKey: "live.health.dependencies.systemAudio" },
  { key: "speechProvider", labelKey: "live.health.dependencies.transcription" },
  { key: "modelProvider", labelKey: "live.health.dependencies.aiProvider" },
];

export function RuntimeHealth({ health, onRetry }: RuntimeHealthProps) {
  const { t } = useTranslation();
  const retryable = [health.sidecar, health.speechProvider, health.modelProvider].some(isRetryable);
  const issue = dependencies
    .map(({ key, labelKey }) => ({ label: t(labelKey), value: health[key] }))
    .find(({ value }) => value.status === "error" || value.status === "offline" || value.status === "degraded");

  return (
    <section aria-label={t("live.health.label")} className="runtime-health">
      <div className="runtime-health__items">
        {dependencies.map(({ key, labelKey }) => (
          <HealthState key={key} label={t(labelKey)} value={health[key]} />
        ))}
      </div>
      {issue ? (
        <div className="runtime-health__issue" role="alert">
          <span>
            {t("live.health.issue.status", {
              dependency: issue.label,
              status: statusDescription(issue.value.status, t),
            })}
          </span>
          {retryable ? (
            <button onClick={onRetry} type="button">{t("live.health.retry")}</button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function HealthState({ label, value }: { label: string; value: DependencyHealth }) {
  const { t } = useTranslation();
  const state = statusLabel(value.status, t);
  const accessibleLabel = t("live.health.accessible", { dependency: label, status: state });
  return (
    <span
      aria-label={accessibleLabel}
      className={`runtime-health__item runtime-health__item--${statusTone(value.status)}`}
      role="status"
    >
      <span aria-hidden="true" className="runtime-health__dot" />
      <span>{label}</span>
      <span className="runtime-health__state">{state}</span>
    </span>
  );
}

function isRetryable(value: DependencyHealth): boolean {
  return value.status === "error" || value.status === "offline";
}

type Translate = ReturnType<typeof useTranslation>["t"];

function statusLabel(status: RuntimeHealthStatus, t: Translate): string {
  return t(`live.health.status.${status}`);
}

function statusDescription(status: RuntimeHealthStatus, t: Translate): string {
  return t(`live.health.statusDescription.${status}`);
}

function statusTone(status: RuntimeHealthStatus): "healthy" | "pending" | "warning" | "error" {
  switch (status) {
    case "ready": return "healthy";
    case "pending":
    case "unknown": return "pending";
    case "degraded": return "warning";
    case "error":
    case "offline": return "error";
  }
}
