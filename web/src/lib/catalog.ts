import { useEffect } from "react";
import { api, errorMessage } from "./api";
import { createStore, useStore } from "./store";
import type { MediaKind, ModelSpec, Preset, ProviderState } from "./types";

// Cached server catalogs: models and presets per output kind, and the provider list.

interface Loadable<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
}

const empty = <T,>(): Loadable<T> => ({ data: null, error: null, loading: false });

function cachedLoader<T>(fetcher: () => Promise<T>) {
  const store = createStore<Loadable<T>>(empty());
  let inflight: Promise<void> | null = null;
  function load(force = false): Promise<void> {
    const state = store.get();
    if (inflight) return inflight;
    if (state.data && !force) return Promise.resolve();
    store.set({ ...state, loading: true, error: null });
    inflight = fetcher()
      .then((data) => store.set({ data, error: null, loading: false }))
      .catch((error: unknown) => store.set({ ...store.get(), error: errorMessage(error), loading: false }))
      .finally(() => {
        inflight = null;
      });
    return inflight;
  }
  return { store, load };
}

const modelLoaders = new Map<string, ReturnType<typeof cachedLoader<ModelSpec[]>>>();
function modelLoader(output: MediaKind | "all") {
  let loader = modelLoaders.get(output);
  if (!loader) {
    loader = cachedLoader(() => api.models(output === "all" ? {} : { output }).then((answer) => answer.models ?? []));
    modelLoaders.set(output, loader);
  }
  return loader;
}

export function useModels(output: MediaKind | "all", enabled = true) {
  const loader = modelLoader(output);
  const state = useStore(loader.store);
  useEffect(() => {
    if (enabled) void loader.load();
  }, [loader, enabled]);
  return state;
}

export const loadModels = (output: MediaKind | "all", force = false) => modelLoader(output).load(force);
export const peekModels = (output: MediaKind | "all") => modelLoader(output).store.get().data;

/** Provider settings changed: `configured` flags and discovered models may differ now. */
export function invalidateModels() {
  for (const [, loader] of modelLoaders) {
    if (loader.store.get().data) void loader.load(true);
  }
}

const presetLoaders = new Map<MediaKind, ReturnType<typeof cachedLoader<Preset[]>>>();
export function usePresets(output: MediaKind) {
  let loader = presetLoaders.get(output);
  if (!loader) {
    loader = cachedLoader(() => api.presets(output).then((answer) => answer.presets ?? []));
    presetLoaders.set(output, loader);
  }
  const state = useStore(loader.store);
  const current = loader;
  useEffect(() => {
    void current.load();
  }, [current]);
  return state;
}

const providerLoader = cachedLoader(() => api.providers().then((answer) => answer.providers ?? []));
export const providersStore = providerLoader.store;
export const loadProviders = providerLoader.load;

export function useProviders() {
  const state = useStore(providerLoader.store);
  useEffect(() => {
    void providerLoader.load();
  }, []);
  return state;
}

export function replaceProvider(next: ProviderState) {
  providerLoader.store.set((state) => ({ ...state, data: state.data?.map((item) => (item.id === next.id ? next : item)) ?? [next] }));
}

/** Default model for an output: a configured featured one, else any configured one, else the first. */
export function defaultModel(models: ModelSpec[]): ModelSpec | undefined {
  const configured = models.filter((model) => model.configured);
  const pool = configured.length ? configured : models;
  return pool.find((model) => model.tags?.includes("featured")) ?? pool[0];
}

/** Readable name for a model id when its spec is not loaded: "fal:fal-ai/flux-pro/v1.1" -> "flux-pro/v1.1". */
export function fallbackModelName(id: string): string {
  const remote = id.includes(":") ? id.slice(id.indexOf(":") + 1) : id;
  const parts = remote.split("/").filter(Boolean);
  return parts.slice(-2).join("/") || id;
}

/** Model names for the given outputs, keyed by id. */
export function useModelNames(outputs: readonly MediaKind[]): (id: string | null | undefined) => string {
  const image = useModels("image", outputs.includes("image"));
  const video = useModels("video", outputs.includes("video"));
  const audio = useModels("audio", outputs.includes("audio"));
  const lists = { image, video, audio };
  const names = new Map<string, string>();
  for (const output of outputs) for (const model of lists[output].data ?? []) names.set(model.id, model.name);
  return (id) => (id ? (names.get(id) ?? fallbackModelName(id)) : "");
}
