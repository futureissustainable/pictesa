import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { errorMessage } from "../lib/api";
import { createStore } from "../lib/store";
import { Icon, Spinner } from "./icon";

/** Open modal dialogs, newest last. Notices render inside the newest so they stay visible and usable. */
export const openDialogs = createStore<HTMLDialogElement[]>([]);

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
    if (dialog) openDialogs.set((list) => [...list, dialog]);
    return () => {
      if (dialog) openDialogs.set((list) => list.filter((item) => item !== dialog));
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

/** Asks before a destructive action. The dialog stays open with the error when the action fails. */
export function ConfirmDialog({
  title,
  children,
  confirm,
  onConfirm,
  onClose,
}: {
  title: string;
  children: ReactNode;
  /** The confirm button's label, naming the action ("Delete for good"). */
  confirm: string;
  onConfirm(): Promise<unknown>;
  onClose(): void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <Dialog title={title} onClose={() => !busy && onClose()}>
      <div className="library-form">
        {children}
        {error && (
          <p role="alert" className="notice error">
            {error}
          </p>
        )}
        <div className="dialog-actions">
          <button type="button" className="text-button" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="primary-button"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                await onConfirm();
                onClose();
              } catch (reason) {
                setError(errorMessage(reason));
                setBusy(false);
              }
            }}
          >
            {busy && <Spinner />}
            {confirm}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
