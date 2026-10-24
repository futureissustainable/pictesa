import { useEffect, useState } from "react";
import { api, errorMessage } from "./api";
import { onReconnect, subscribeEvents } from "./events";
import { notifyError } from "./notices";
import { createStore, useStore } from "./store";
import type { Asset, Recipe, RecipeRun } from "./types";

// Recipes are multi-step pipelines. Each step is an ordinary job (job events, tray cards);
// a run ties the steps together and fires `recipe` events.

const recipesStore = createStore<{ data: Recipe[] | null; error: string | null }>({ data: null, error: null });
let recipesLoading: Promise<void> | null = null;

export function loadRecipes(force = false): Promise<void> {
  if (recipesLoading) return recipesLoading;
  if (recipesStore.get().data && !force) return Promise.resolve();
  recipesLoading = api
    .recipes()
    .then(({ recipes }) => recipesStore.set({ data: recipes ?? [], error: null }))
    .catch((error: unknown) => recipesStore.set({ data: recipesStore.get().data, error: errorMessage(error) }))
    .finally(() => {
      recipesLoading = null;
    });
  return recipesLoading;
}

export function useRecipes() {
  const state = useStore(recipesStore);
  useEffect(() => {
    void loadRecipes();
  }, []);
  return state;
}

/** Runs shown in the tray: running ones, fresh failures and just-finished ones. */
export const runs = createStore<RecipeRun[]>([]);
const dismissedRuns = new Set<string>();
const runTimers = new Map<string, ReturnType<typeof setTimeout>>();
const READY_MS = 5000;

const isRunning = (run: RecipeRun) => run.status === "running";

export function upsertRun(run: RecipeRun, track = false) {
  if (dismissedRuns.has(run.id)) return;
  const list = runs.get();
  const index = list.findIndex((item) => item.id === run.id);
  if (index === -1) {
    if (!track && !isRunning(run)) return;
    runs.set([run, ...list]);
  } else {
    const current = list[index]!;
    if (current.status !== "running" && run.status === "running") return;
    if (run.updated_at < current.updated_at) return;
    runs.set(list.map((item) => (item.id === run.id ? run : item)));
  }
  if (run.status === "succeeded" && !runTimers.has(run.id)) {
    runTimers.set(
      run.id,
      setTimeout(() => {
        runTimers.delete(run.id);
        runs.set((items) => items.filter((item) => item.id !== run.id));
      }, READY_MS),
    );
  }
}

export function dismissRun(id: string) {
  dismissedRuns.add(id);
  runs.set((list) => list.filter((run) => run.id !== id));
}

export async function cancelRun(run: RecipeRun) {
  try {
    upsertRun(await api.cancelRun(run.id));
  } catch (error) {
    notifyError(errorMessage(error));
  }
}

/** Job ids that belong to a run in the tray, so their cards are not shown twice. */
export function jobsInRuns(list: RecipeRun[]): Set<string> {
  return new Set(list.flatMap((run) => run.jobs));
}

export async function loadActiveRuns() {
  try {
    const { runs: list } = await api.recipeRuns(30);
    for (const run of [...list].reverse()) if (isRunning(run)) upsertRun(run);
  } catch {
    // Older servers have no recipes.
  }
}

subscribeEvents({ recipe: (run) => upsertRun(run) });
onReconnect(() => void loadActiveRuns());

/** Final outputs of recent runs, newest first, kept live. */
export function useRecipeResults(): { items: Asset[]; loading: boolean; error: string | null } {
  const [items, setItems] = useState<Asset[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const finalJob = (run: RecipeRun) => run.jobs[run.jobs.length - 1];
    const addOutputs = (assets: Asset[]) =>
      setItems((current) => {
        const known = new Set(current.map((asset) => asset.id));
        const fresh = assets.filter((asset) => !known.has(asset.id) && !asset.deleted_at);
        return fresh.length ? [...fresh, ...current].sort((a, b) => b.created_at - a.created_at) : current;
      });
    api
      .recipeRuns(30)
      .then(async ({ runs: list }) => {
        const ids = list.filter((run) => run.status === "succeeded").map(finalJob).filter((id): id is string => Boolean(id));
        const jobs = await Promise.allSettled(ids.map((id) => api.job(id)));
        if (!alive) return;
        addOutputs(jobs.flatMap((result) => (result.status === "fulfilled" ? (result.value.outputs ?? []) : [])));
      })
      .catch((reason: unknown) => alive && setError(errorMessage(reason)))
      .finally(() => alive && setLoading(false));

    const off = subscribeEvents({
      recipe: (run) => {
        const id = finalJob(run);
        if (run.status === "succeeded" && id) void api.job(id).then((job) => alive && addOutputs(job.outputs ?? []));
      },
      asset: (asset) =>
        setItems((current) =>
          current.some((item) => item.id === asset.id)
            ? asset.deleted_at
              ? current.filter((item) => item.id !== asset.id)
              : current.map((item) => (item.id === asset.id ? asset : item))
            : current,
        ),
      removed: (id) => setItems((current) => current.filter((item) => item.id !== id)),
    });
    return () => {
      alive = false;
      off();
    };
  }, []);

  return { items, loading, error };
}

export interface RecipeDraft {
  values: Record<string, string>;
  inputs: Record<string, { key: string; asset?: Asset; name?: string; progress?: number | null; preview?: string | null }[]>;
  models: Record<string, string>;
}

/** Drafts per recipe for this session, so switching recipes keeps what was typed. */
export const recipeDrafts = createStore<Record<string, RecipeDraft>>({});

export function draftFor(recipe: Recipe): RecipeDraft {
  return (
    recipeDrafts.get()[recipe.id] ?? {
      values: Object.fromEntries(recipe.fields.filter((field) => field.default !== undefined && field.default !== null).map((field) => [field.key, String(field.default)])),
      inputs: {},
      models: {},
    }
  );
}

export function updateDraft(recipe: Recipe, change: (draft: RecipeDraft) => RecipeDraft) {
  recipeDrafts.set((drafts) => ({ ...drafts, [recipe.id]: change(drafts[recipe.id] ?? draftFor(recipe)) }));
}

/** Replaces `{{field}}` with the form's values, as the server does for step prompts and params. */
export const fillTemplate = (text: string, values: Record<string, string>) => text.replace(/\{\{\s*([\w-]+)\s*\}\}/g, (_, key: string) => values[key] ?? "");

export async function startRun(recipe: Recipe, body: { values: Record<string, string>; inputs: Record<string, string[]>; models?: Record<string, string> }) {
  const run = await api.runRecipe(recipe.id, body);
  upsertRun(run, true);
  return run;
}
