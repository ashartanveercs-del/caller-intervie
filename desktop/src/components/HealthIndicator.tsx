export type HealthState = "healthy" | "degraded" | "error";

export type HealthIndicatorProps = {
  state: HealthState;
  label: string;
};

export function HealthIndicator({ state, label }: HealthIndicatorProps) {
  return (
    <span className={`health-indicator health-indicator--${state}`} role="status">
      <span aria-hidden="true" className="health-indicator__dot" />
      {label}
    </span>
  );
}
