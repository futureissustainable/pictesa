import { createElement, useMemo, useSyncExternalStore, type AnchorHTMLAttributes, type MouseEvent } from "react";

// A minimal History API router. The server answers index.html for every UI path.

const listeners = new Set<() => void>();
const notify = () => listeners.forEach((listener) => listener());
if (typeof window !== "undefined") window.addEventListener("popstate", notify);

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
const snapshot = () => location.pathname + location.search;

export interface NavigateOptions {
  replace?: boolean;
  /** Stored in history.state, read back with `historyState()`. */
  state?: Record<string, unknown>;
}

export function navigate(to: string, options: NavigateOptions = {}) {
  const current = snapshot();
  const method = options.replace ? "replaceState" : "pushState";
  if (to === current && !options.state) return;
  history[method](options.state ?? null, "", to);
  notify();
  if (!options.replace && new URL(to, location.origin).pathname !== new URL(current, location.origin).pathname) window.scrollTo(0, 0);
}

export function historyState(): Record<string, unknown> {
  return (history.state as Record<string, unknown> | null) ?? {};
}

export function useLocation(): URL {
  const href = useSyncExternalStore(subscribe, snapshot, snapshot);
  return useMemo(() => new URL(href, location.origin), [href]);
}

/** Builds the current path with some search params changed (null removes one). */
export function withParams(changes: Record<string, string | null | undefined>, base?: URL): string {
  const url = new URL(base ?? location.href);
  for (const [key, value] of Object.entries(changes)) {
    if (value === null || value === undefined || value === "") url.searchParams.delete(key);
    else url.searchParams.set(key, value);
  }
  const search = url.searchParams.toString();
  return url.pathname + (search ? `?${search}` : "");
}

type LinkProps = AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; replace?: boolean };

export function Link({ to, replace, onClick, ...props }: LinkProps) {
  return createElement("a", {
    ...props,
    href: to,
    onClick(event: MouseEvent<HTMLAnchorElement>) {
      onClick?.(event);
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || props.target) return;
      event.preventDefault();
      navigate(to, { replace });
    },
  });
}
