import { type LucideIcon } from "lucide-react";
import { useId } from "react";

export type IconButtonProps = {
  label: string;
  tooltip?: string;
  pressed?: boolean;
  disabled?: boolean;
  icon: LucideIcon;
  onClick?: () => void;
};

export function IconButton({
  label,
  tooltip,
  pressed,
  disabled = false,
  icon: Icon,
  onClick,
}: IconButtonProps) {
  const tooltipId = useId();
  const tooltipText = tooltip ?? label;

  return (
    <span className="icon-button">
      <button
        aria-describedby={tooltipId}
        aria-label={label}
        aria-pressed={pressed}
        className="icon-button__control"
        disabled={disabled}
        onClick={onClick}
        type="button"
      >
        <Icon aria-hidden="true" size={18} strokeWidth={1.8} />
      </button>
      <span className="icon-button__tooltip" id={tooltipId} role="tooltip">
        {tooltipText}
      </span>
    </span>
  );
}
