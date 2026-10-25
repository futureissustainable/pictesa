import { useCallback, useEffect, useId, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Icon, type IconName } from "./icon";

type Placement = "auto" | "above" | "below";

export interface ControlProps {
  /** Accessible name of the trigger and title of the panel. */
  label: string;
  /** Visible trigger text, also announced as the description. */
  value?: ReactNode;
  icon?: IconName;
  children: ReactNode | ((close: () => void) => ReactNode);
  disabled?: boolean;
  /** 260px by default; wide panels hold lists and code. */
  size?: "default" | "wide" | "large";
  placement?: Placement;
  caret?: boolean;
  showTitle?: boolean;
  invalid?: boolean;
  className?: string;
  panelClassName?: string;
  /** Replaces the icon, value and caret inside the trigger. */
  trigger?: ReactNode;
  triggerProps?: ButtonHTMLAttributes<HTMLButtonElement> & Record<`data-${string}`, string | undefined>;
  onOpenChange?: (open: boolean) => void;
}

const widths = { default: 260, wide: 420, large: 520 };

/**
 * A setting on the control line: a button that opens a native popover. The platform handles
 * Escape, light dismiss and focus return; this places the panel next to its trigger.
 */
export function Control({
  label,
  value,
  icon,
  children,
  disabled = false,
  size = "default",
  placement = "auto",
  caret = true,
  showTitle = true,
  invalid = false,
  className = "",
  panelClassName = "",
  trigger,
  triggerProps,
  onOpenChange,
}: ControlProps) {
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);

  const place = useCallback(() => {
    if (!button.current || !panel.current) return;
    const rect = button.current.getBoundingClientRect();
    const width = Math.min(widths[size], window.innerWidth - 24);
    const above = rect.top;
    const below = window.innerHeight - rect.bottom;
    const upward = placement === "above" || (placement === "auto" && above > below && below < 360);
    Object.assign(panel.current.style, {
      width: `${width}px`,
      left: `${Math.max(12, Math.min(rect.left, window.innerWidth - width - 12))}px`,
      top: upward ? "auto" : `${rect.bottom + 8}px`,
      bottom: upward ? `${window.innerHeight - rect.top + 8}px` : "auto",
      maxHeight: `${Math.max(140, (upward ? above : below) - 20)}px`,
    });
  }, [placement, size]);

  useEffect(() => {
    if (!open) return;
    const move = () => place();
    window.addEventListener("resize", move);
    window.addEventListener("scroll", move, true);
    return () => {
      window.removeEventListener("resize", move);
      window.removeEventListener("scroll", move, true);
    };
  }, [open, place]);

  const close = useCallback(() => panel.current?.hidePopover(), []);
  const describedBy = value !== undefined && value !== null && !trigger ? `${id}-value` : undefined;

  return (
    <>
      <button
        {...triggerProps}
        ref={button}
        type="button"
        className={`control-trigger ${className}`}
        popoverTarget={id}
        aria-label={label}
        aria-describedby={describedBy}
        aria-expanded={open}
        aria-invalid={invalid || undefined}
        disabled={disabled}
      >
        {trigger ?? (
          <>
            {icon && <Icon name={icon} />}
            {value !== undefined && value !== null && (
              <span id={`${id}-value`} className="control-value">
                {value}
              </span>
            )}
            {caret && <Icon name="keyboard_arrow_down" className="control-caret" />}
          </>
        )}
      </button>
      <div
        ref={panel}
        id={id}
        popover="auto"
        role="dialog"
        aria-label={label}
        className={`control-panel ${panelClassName}`}
        onBeforeToggle={(event) => {
          if (event.newState === "open") place();
        }}
        onToggle={(event) => {
          const isOpen = event.newState === "open";
          setOpen(isOpen);
          onOpenChange?.(isOpen);
          if (isOpen) requestAnimationFrame(() => panel.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus());
        }}
        onClick={(event) => {
          if (event.target instanceof Element && event.target.closest("[data-choice]")) close();
        }}
      >
        {showTitle && <p className="control-title">{label}</p>}
        {open && (typeof children === "function" ? children(close) : children)}
      </div>
    </>
  );
}

/** One choice inside a control panel. Closes the panel when picked, unless `keepOpen` (several can be on). */
export function Choice({
  selected = false,
  onPick,
  children,
  icon,
  disabled,
  className = "",
  title,
  keepOpen = false,
}: {
  selected?: boolean;
  onPick: () => void;
  children: ReactNode;
  icon?: IconName;
  disabled?: boolean;
  className?: string;
  title?: string;
  keepOpen?: boolean;
}) {
  return (
    <button type="button" data-choice={keepOpen ? undefined : ""} className={`choice ${className}`} aria-pressed={selected} disabled={disabled} onClick={onPick} title={title}>
      {icon && <Icon name={icon} />}
      <span className="choice-body">{children}</span>
      {selected && <Icon name="check" className="choice-check" />}
    </button>
  );
}
