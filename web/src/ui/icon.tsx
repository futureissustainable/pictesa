import type { IconName } from "./icon-names";

export type { IconName };

/** Material Symbols Outlined ligature. Decorative: give the surrounding control an accessible name. */
export function Icon({ name, className = "", filled = false }: { name: IconName; className?: string; filled?: boolean }) {
  return (
    <span aria-hidden="true" className={`icon ${filled ? "icon-filled" : ""} ${className}`}>
      {name}
    </span>
  );
}

export function Spinner({ className = "" }: { className?: string }) {
  return <Icon name="progress_activity" className={`spinner ${className}`} />;
}
