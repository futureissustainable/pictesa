import { useSyncExternalStore } from "react";

// Small persistence helpers. Storage can be unavailable (private mode, quotas); the UI keeps working.

export function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

export function writeJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage is optional.
  }
}

function mediaStore(query: string) {
  return {
    subscribe(listener: () => void) {
      const list = window.matchMedia(query);
      list.addEventListener("change", listener);
      return () => list.removeEventListener("change", listener);
    },
    get: () => window.matchMedia(query).matches,
  };
}

const mediaStores = new Map<string, ReturnType<typeof mediaStore>>();

export function useMediaQuery(query: string): boolean {
  let store = mediaStores.get(query);
  if (!store) {
    store = mediaStore(query);
    mediaStores.set(query, store);
  }
  return useSyncExternalStore(store.subscribe, store.get, () => false);
}

export const PHONE = "(max-width: 560px)";
export const REDUCED_MOTION = "(prefers-reduced-motion: reduce)";
export const prefersReducedMotion = () => window.matchMedia(REDUCED_MOTION).matches;
