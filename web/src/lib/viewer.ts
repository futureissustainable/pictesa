import { useEffect } from "react";
import { historyState, navigate, withParams } from "./router";
import { createStore } from "./store";
import type { Asset } from "./types";

// The detail viewer opens over any page with `?asset=<id>`. The page that opened it registers
// its list here so arrow keys step through the same order the grid shows.

export interface Sequence {
  items: Asset[];
  hasMore: boolean;
  loadMore(): void;
}

export const sequence = createStore<Sequence | null>(null);

/** Keeps the viewer's sequence in step with a page's live list. */
export function useSequence(items: Asset[], hasMore: boolean, loadMore: () => void) {
  useEffect(() => {
    sequence.set({ items, hasMore, loadMore });
  }, [items, hasMore, loadMore]);
}

export function openViewer(asset: Asset) {
  navigate(withParams({ asset: asset.id }), { state: { viewer: true } });
}

export function stepViewer(id: string) {
  navigate(withParams({ asset: id }), { replace: true, state: historyState() });
}

// Set while a history.back() from the viewer is on its way, so a second close does not leave the page.
let leaving = false;
if (typeof window !== "undefined") window.addEventListener("popstate", () => (leaving = false));

export function closeViewer() {
  if (leaving || !new URLSearchParams(location.search).has("asset")) return;
  // Opened from a page: go back so the phone's back gesture and Escape agree.
  if (historyState().viewer) {
    leaving = true;
    history.back();
  } else navigate(withParams({ asset: null }), { replace: true });
}
