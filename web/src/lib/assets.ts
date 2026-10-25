import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorMessage, isAbort } from "./api";
import { emitLocal, onReconnect, subscribeEvents } from "./events";
import { notify, notifyError } from "./notices";
import type { Asset, AssetQuery } from "./types";

const PAGE = 60;

/** Whether an asset belongs in a list. Text search runs on the server, so `q` lists never take new items. */
export function matchesQuery(asset: Asset, query: AssetQuery): boolean {
  if (query.kind && asset.kind !== query.kind) return false;
  if (query.favorite && !asset.favorite) return false;
  if (query.source ? asset.source !== query.source : asset.source === "edit") return false;
  if (query.model && asset.model !== query.model) return false;
  return Boolean(query.trash) === Boolean(asset.deleted_at);
}

const isOlder = (item: Asset, than: Asset) => item.created_at < than.created_at || (item.created_at === than.created_at && item.id < than.id);

interface ListState {
  key: string;
  items: Asset[];
  next: string | null;
  loading: boolean;
  error: string | null;
  loaded: boolean;
  /** Ids that arrived live, so their cards animate in once. */
  fresh: ReadonlySet<string>;
}

export interface AssetList {
  items: Asset[];
  loading: boolean;
  loaded: boolean;
  error: string | null;
  hasMore: boolean;
  fresh: ReadonlySet<string>;
  loadMore(): void;
  reload(): void;
}

const noFresh: ReadonlySet<string> = new Set();
const start = (key: string): ListState => ({ key, items: [], next: null, loading: true, error: null, loaded: false, fresh: noFresh });

/** A paged, live asset list: cursor pagination plus merges from server events. */
export function useAssetList(query: AssetQuery): AssetList {
  const key = JSON.stringify(query);
  const [state, setState] = useState<ListState>(() => start(key));
  const queryRef = useRef(query);
  queryRef.current = query;
  const abortRef = useRef<AbortController | null>(null);
  const stateRef = useRef(state);
  stateRef.current = state;

  const fetchPage = useCallback(
    (before: string | null) => {
      abortRef.current?.abort();
      const controller = new AbortController();
      abortRef.current = controller;
      setState((current) => (current.key === key ? { ...current, loading: true, error: null } : current));
      api
        .assets({ ...queryRef.current, limit: PAGE, before }, controller.signal)
        .then(({ assets, next }) => {
          setState((current) => {
            if (current.key !== key) return current;
            const known = new Set(before ? current.items.map((item) => item.id) : []);
            const items = before ? [...current.items, ...assets.filter((item) => !known.has(item.id))] : assets;
            return { ...current, items, next: next ?? null, loading: false, loaded: true };
          });
        })
        .catch((error: unknown) => {
          if (isAbort(error)) return;
          setState((current) => (current.key === key ? { ...current, loading: false, loaded: true, error: errorMessage(error) } : current));
        });
    },
    [key],
  );

  useEffect(() => {
    setState(start(key));
    fetchPage(null);
    return () => abortRef.current?.abort();
  }, [key, fetchPage]);

  useEffect(() => {
    const merge = (asset: Asset) =>
      setState((list) => {
        if (list.key !== key || !list.loaded) return list;
        const current = queryRef.current;
        const fits = matchesQuery(asset, current);
        const index = list.items.findIndex((item) => item.id === asset.id);
        if (index >= 0) {
          if (!fits) return { ...list, items: list.items.filter((item) => item.id !== asset.id) };
          const items = list.items.slice();
          items[index] = asset;
          return { ...list, items };
        }
        if (!fits || current.q) return list;
        const at = list.items.findIndex((item) => isOlder(item, asset));
        // Older than everything loaded while more pages exist: a later page brings it.
        if (at === -1 && list.next) return list;
        const items = list.items.slice();
        items.splice(at === -1 ? items.length : at, 0, asset);
        return { ...list, items, fresh: new Set(list.fresh).add(asset.id) };
      });
    const remove = (id: string) => setState((list) => ({ ...list, items: list.items.filter((item) => item.id !== id) }));
    const offEvents = subscribeEvents({
      asset: merge,
      removed: remove,
      job: (job) => {
        if (job.status === "succeeded") job.outputs?.forEach(merge);
      },
    });
    const offReconnect = onReconnect(() => fetchPage(null));
    return () => {
      offEvents();
      offReconnect();
    };
  }, [key, fetchPage]);

  // Fresh ids only matter for the entrance animation.
  useEffect(() => {
    if (!state.fresh.size) return;
    const timer = setTimeout(() => setState((list) => ({ ...list, fresh: noFresh })), 1200);
    return () => clearTimeout(timer);
  }, [state.fresh]);

  const loadMore = useCallback(() => {
    const current = stateRef.current;
    if (current.key === key && !current.loading && current.next && !current.error) fetchPage(current.next);
  }, [fetchPage, key]);

  const reload = useCallback(() => fetchPage(null), [fetchPage]);
  const same = state.key === key;

  return {
    items: same ? state.items : [],
    loading: !same || state.loading,
    loaded: same && state.loaded,
    error: same ? state.error : null,
    hasMore: same && Boolean(state.next),
    fresh: same ? state.fresh : noFresh,
    loadMore,
    reload,
  };
}

/** Favorite toggle with an instant local update that every list sees. */
export async function toggleFavorite(asset: Asset) {
  const optimistic = { ...asset, favorite: !asset.favorite };
  emitLocal({ asset: optimistic });
  try {
    emitLocal({ asset: await api.patchAsset(asset.id, { favorite: optimistic.favorite }) });
  } catch (error) {
    emitLocal({ asset });
    notifyError(errorMessage(error));
  }
}

export async function trashAsset(asset: Asset): Promise<boolean> {
  try {
    await api.deleteAsset(asset.id);
    const trashed = { ...asset, deleted_at: Date.now() };
    emitLocal({ asset: trashed });
    notify("Moved to trash.", { action: { label: "Undo", run: () => void restoreAsset(trashed, true) } });
    return true;
  } catch (error) {
    notifyError(errorMessage(error));
    return false;
  }
}

export async function restoreAsset(asset: Asset, quiet = false): Promise<boolean> {
  try {
    emitLocal({ asset: await api.restoreAsset(asset.id) });
    if (!quiet) notify("Restored.");
    return true;
  } catch (error) {
    notifyError(errorMessage(error));
    return false;
  }
}

export async function deleteForever(asset: Asset): Promise<boolean> {
  try {
    await api.deleteAsset(asset.id, true);
    emitLocal({ removed: asset.id });
    notify("Deleted permanently.");
    return true;
  } catch (error) {
    notifyError(errorMessage(error));
    return false;
  }
}
