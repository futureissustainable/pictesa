import { useEffect } from "react";
import { api, ApiError, errorMessage } from "./api";
import { createStore, useStore } from "./store";
import type { MediaKind, ModelSpec, Preset, ProviderState } from "./types";

// Cached server catalogs: models and presets per output kind, and the provider list.

interface Loadable<T> {
  data: T | null;
  error: string | null;
  /** HTTP status of the failure (403 when the account may not see it). */
  status?: number;
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
      .catch((error: unknown) => store.set({ ...store.get(), error: errorMessage(error), status: error instanceof ApiError ? error.status : 0, loading: false }))
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

/**
 * Default model for an output: a configured one that makes something from a prompt (not an
 * upscaler or an editor that needs an input), featured first; else any configured one; else the first.
 */
export function defaultModel(models: ModelSpec[]): ModelSpec | undefined {
  const configured = models.filter((model) => model.configured);
  const pool = configured.length ? configured : models;
  const fromPrompt = pool.filter((model) => model.tasks.some((task) => task.startsWith("text_to_")));
  const choices = fromPrompt.length ? fromPrompt : pool;
  return choices.find((model) => model.tags?.includes("featured")) ?? choices[0];
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

// Discovered gateway models arrive without params or inputs (`complete: false`). The full spec
// is fetched once, when a model is selected, and kept for the session.
const fullSpecs = createStore<Record<string, ModelSpec>>({});
const specErrors = createStore<Record<string, string>>({});
const pendingSpecs = new Map<string, Promise<ModelSpec | null>>();

export function loadFullModel(model: ModelSpec): Promise<ModelSpec | null> {
  if (model.complete !== false) return Promise.resolve(model);
  const cached = fullSpecs.get()[model.id];
  if (cached) return Promise.resolve(cached);
  let pending = pendingSpecs.get(model.id);
  if (!pending) {
    specErrors.set(({ [model.id]: _old, ...rest }) => rest);
    pending = api
      .model(model.id)
      .then((spec) => {
        const full: ModelSpec = { ...model, ...spec, complete: true };
        fullSpecs.set((specs) => ({ ...specs, [model.id]: full }));
        return full;
      })
      .catch((error: unknown) => {
        specErrors.set((errors) => ({ ...errors, [model.id]: errorMessage(error) }));
        return null;
      })
      .finally(() => pendingSpecs.delete(model.id));
    pendingSpecs.set(model.id, pending);
  }
  return pending;
}

export function useFullModel(model: ModelSpec | undefined): { spec: ModelSpec | undefined; loading: boolean; error: string | null; retry(): void } {
  const specs = useStore(fullSpecs);
  const errors = useStore(specErrors);
  useEffect(() => {
    if (model && model.complete === false) void loadFullModel(model);
  }, [model]);
  const retry = () => {
    if (model) void loadFullModel(model);
  };
  if (!model || model.complete !== false) return { spec: model, loading: false, error: null, retry };
  const full = specs[model.id];
  const error = errors[model.id] ?? null;
  return { spec: full, loading: !full && !error, error, retry };
}

// Hosted wallet. Only read in hosted mode; `enabled` decides whether anything is shown.
const billingLoader = cachedLoader(() => api.billing());
export const loadBilling = billingLoader.load;

export function useBilling(active: boolean) {
  const state = useStore(billingLoader.store);
  useEffect(() => {
    if (active) void billingLoader.load();
  }, [active]);
  return state;
}

let billingTimer: ReturnType<typeof setTimeout> | undefined;
/** Runs change the balance; refresh it shortly after one finishes. */
export function refreshBillingSoon() {
  if (!billingLoader.store.get().data?.enabled) return;
  clearTimeout(billingTimer);
  billingTimer = setTimeout(() => void billingLoader.load(true), 1200);
}

