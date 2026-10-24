import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from "react";
import { Icon } from "./icon";

/**
 * A modal dialog on the native <dialog>: focus stays inside, Escape closes it and focus
 * returns to the control that opened it.
 */
export function Dialog({
  title,
  onClose,
  children,
  size = "default",
  className = "",
  hideTitle = false,
  onKeyDown,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  size?: "default" | "wide" | "full";
  className?: string;
  hideTitle?: boolean;
  onKeyDown?: (event: KeyboardEvent<HTMLDialogElement>) => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    const trigger = document.activeElement;
    const dialog = ref.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => {
      dialog?.close();
      if (trigger instanceof HTMLElement && trigger.isConnected) trigger.focus({ preventScroll: true });
    };
  }, []);

  return (
    <dialog
      ref={ref}
      aria-labelledby={titleId}
      className={`dialog dialog-${size} ${className}`}
      onCancel={(event) => {
        event.preventDefault();
        closeRef.current();
      }}
      onKeyDown={onKeyDown}
      onClick={(event) => {
        if (event.target !== event.currentTarget) return;
        const { left, right, top, bottom } = event.currentTarget.getBoundingClientRect();
        if (event.clientX < left || event.clientX > right || event.clientY < top || event.clientY > bottom) closeRef.current();
      }}
    >
      {hideTitle ? (
        <h2 id={titleId} className="sr-only">
          {title}
        </h2>
      ) : (
        <header className="dialog-header">
          <h2 id={titleId}>{title}</h2>
          <button type="button" className="icon-button" aria-label="Close" onClick={() => closeRef.current()}>
            <Icon name="close" />
          </button>
        </header>
      )}
      {children}
    </dialog>
  );
}
