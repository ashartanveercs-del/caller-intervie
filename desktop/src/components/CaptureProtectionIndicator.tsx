import { RotateCw, ShieldAlert, ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { CaptureProtectionStatus } from "../platform";

export type CaptureProtectionIndicatorProps = {
  captureProtection: CaptureProtectionStatus;
  onRetry(): void;
};

export function CaptureProtectionIndicator({ captureProtection, onRetry }: CaptureProtectionIndicatorProps) {
  const { t } = useTranslation();
  const { state } = captureProtection;
  const label = state === "protected"
    ? t("captureProtection.protected")
    : state === "applying"
      ? t("captureProtection.applying")
      : state === "unsupported"
        ? t("captureProtection.desktopAppRequired")
        : t("captureProtection.unavailable");
  const Icon = state === "protected" ? ShieldCheck : ShieldAlert;

  return (
    <div aria-label={label} className={`capture-protection capture-protection--${state}`} role="status">
      <Icon aria-hidden="true" size={16} strokeWidth={1.8} />
      <span className="capture-protection__label">{label}</span>
      {state === "unavailable" ? (
        <button
          className="capture-protection__retry"
          onClick={onRetry}
          title={t("captureProtection.retry")}
          type="button"
        >
          <RotateCw aria-hidden="true" size={14} strokeWidth={1.8} />
          {t("captureProtection.retry")}
        </button>
      ) : null}
    </div>
  );
}
