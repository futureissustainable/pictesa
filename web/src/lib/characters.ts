import { useEffect } from "react";
import { api, errorMessage } from "./api";
import { composer } from "./composer";
import { onReconnect, subscribeEvents } from "./events";
import { createStore, useStore } from "./store";
import type { Character } from "./types";

// Characters: named sets of reference images. Kept live through `event: character`.

export const MAX_CHARACTER_IMAGES = 14;

const store = createStore<{ data: Character[] | null; error: string | null }>({ data: null, error: null });
let loading: Promise<void> | null = null;

export function loadCharacters(force = false): Promise<void> {
  if (loading) return loading;
  if (store.get().data && !force) return Promise.resolve();
  loading = api
    .characters()
    .then(({ characters }) => {
      store.set({ data: characters ?? [], error: null });
      forgetMissing(characters ?? []);
    })
    .catch((error: unknown) => store.set({ data: store.get().data, error: errorMessage(error) }))
    .finally(() => {
      loading = null;
    });
  return loading;
}

export function useCharacters() {
  const state = useStore(store);
  useEffect(() => {
    void loadCharacters();
  }, []);
  return state;
}

/** Applies a saved character locally (also used right after our own requests). */
export function upsertCharacter(character: Character) {
  store.set((state) => {
    const list = state.data ?? [];
    const rest = list.filter((item) => item.id !== character.id);
    return { ...state, data: [character, ...rest].sort((a, b) => b.updated_at - a.updated_at) };
  });
}

export function removeCharacter(id: string) {
  store.set((state) => ({ ...state, data: state.data?.filter((item) => item.id !== id) ?? null }));
  forgetMissing(store.get().data ?? []);
}

/** A deleted character leaves the Create draft too, so runs do not ask for it. */
function forgetMissing(list: Character[]) {
  const ids = new Set(list.map((item) => item.id));
  const chosen = composer.get().characterByMode;
  if (!Object.values(chosen).some((id) => id && !ids.has(id))) return;
  composer.set((state) => ({
    ...state,
    characterByMode: Object.fromEntries(Object.entries(state.characterByMode).map(([mode, id]) => [mode, id && ids.has(id) ? id : null])),
  }));
}

subscribeEvents({
  character: (change) => {
    if ("deleted" in change) removeCharacter(change.id);
    else upsertCharacter(change);
  },
});
onReconnect(() => {
  if (store.get().data) void loadCharacters(true);
});
