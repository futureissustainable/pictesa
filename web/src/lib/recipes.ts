import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "./api";
import { onReconnect, subscribeEvents } from "./events";
import { notifyError } from "./notices";
import { createStore, useStore } from "./store";
import type { Asset, Recipe, RecipeField, RecipeRun, RecipeStep } from "./types";

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

/** The jobs whose outputs are a run's results: its final step (older servers: its last job). */
export const resultJobs = (run: RecipeRun) => (run.results?.length ? run.results : run.jobs.slice(-1));

/** Results of recent runs: newest run first, each run's set in step order, kept live. */
export function useRecipeResults(): { items: Asset[]; loading: boolean; error: string | null } {
  const [sets, setSets] = useState<{ run: string; assets: Asset[] }[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const outputs = async (run: RecipeRun) => {
      const jobs = await Promise.allSettled(resultJobs(run).map((id) => api.job(id)));
      return jobs.flatMap((result) => (result.status === "fulfilled" ? (result.value.outputs ?? []) : [])).filter((asset) => !asset.deleted_at);
    };
    const keep = (run: string, assets: Asset[]) =>
      alive && assets.length > 0 && setSets((current) => [...current.filter((set) => set.run !== run), { run, assets }].sort((a, b) => (a.run < b.run ? 1 : -1)));
    const editAssets = (change: (assets: Asset[]) => Asset[]) => setSets((current) => current.map((set) => ({ ...set, assets: change(set.assets) })).filter((set) => set.assets.length > 0));
    api
      .recipeRuns(30)
      .then(({ runs: list }) => Promise.all(list.filter((run) => run.status === "succeeded").map(async (run) => keep(run.id, await outputs(run)))))
      .catch((reason: unknown) => alive && setError(errorMessage(reason)))
      .finally(() => alive && setLoading(false));

    const off = subscribeEvents({
      recipe: (run) => {
        if (run.status === "succeeded") void outputs(run).then((assets) => keep(run.id, assets));
      },
      asset: (asset) => editAssets((assets) => (asset.deleted_at ? assets.filter((item) => item.id !== asset.id) : assets.map((item) => (item.id === asset.id ? asset : item)))),
      removed: (id) => editAssets((assets) => assets.filter((item) => item.id !== id)),
    });
    return () => {
      alive = false;
      off();
    };
  }, []);

  const items = useMemo(() => sets.flatMap((set) => set.assets), [sets]);
  return { items, loading, error };
}

export type DraftValue = string | string[];

export interface RecipeDraft {
  values: Record<string, DraftValue>;
  inputs: Record<string, { key: string; asset?: Asset; name?: string; progress?: number | null; preview?: string | null }[]>;
  models: Record<string, string>;
  /** A character or brand kit for every step, when the recipe offers one. */
  character?: string | null;
}

/** Drafts per recipe for this session, so switching recipes keeps what was typed. */
export const recipeDrafts = createStore<Record<string, RecipeDraft>>({});

export function draftFor(recipe: Recipe): RecipeDraft {
  return (
    recipeDrafts.get()[recipe.id] ?? {
      values: Object.fromEntries(
        recipe.fields.filter((field) => field.default !== undefined && field.default !== null).map((field) => [field.key, Array.isArray(field.default) ? [...field.default] : String(field.default)]),
      ),
      inputs: {},
      models: {},
    }
  );
}

export function updateDraft(recipe: Recipe, change: (draft: RecipeDraft) => RecipeDraft) {
  recipeDrafts.set((drafts) => ({ ...drafts, [recipe.id]: change(drafts[recipe.id] ?? draftFor(recipe)) }));
}

/** Text of a draft value: lists join with commas, as the server does. */
export const valueText = (value: DraftValue | undefined) => (Array.isArray(value) ? value.join(", ") : (value ?? ""));

export const isFilled = (value: DraftValue | undefined) => (Array.isArray(value) ? value.length > 0 : Boolean(value?.trim()));

/** Replaces `{{field}}` with the form's values, as the server does for step prompts and params. */
export const fillTemplate = (text: string, values: Record<string, DraftValue>) => text.replace(/\{\{\s*([\w-]+)\s*\}\}/g, (_, key: string) => valueText(values[key]));

const loneKey = (entry: string) => /^\{\{\s*([\w-]+)\s*\}\}$/.exec(entry.trim())?.[1];

/** How many times a step runs: once, or once per item of its `each` list (mirrors the server). */
export function stepRuns(step: RecipeStep, values: Record<string, DraftValue>): number {
  if (!step.each?.length) return 1;
  let count = 0;
  for (const entry of step.each) {
    const key = loneKey(entry);
    if (!key) {
      count += fillTemplate(entry, values).trim() ? 1 : 0;
      continue;
    }
    const value = values[key];
    count += Array.isArray(value) ? value.filter((item) => item.trim()).length : (value ?? "").split("\n").filter((line) => line.replace(/^\s*(?:[-*•]|\d+[.)])\s+/, "").trim()).length;
  }
  return count;
}

/** Whether a field must be filled in now: always, or unless its partner input or field has a value. */
export function fieldNeeded(field: RecipeField, draft: RecipeDraft): boolean {
  if (field.required) return true;
  const other = field.required_without;
  if (!other) return false;
  const files = (draft.inputs[other] ?? []).some((item) => item.asset);
  return !files && !isFilled(draft.values[other]);
}

export async function startRun(recipe: Recipe, body: { values: Record<string, DraftValue>; inputs: Record<string, string[]>; models?: Record<string, string>; character?: string }) {
  const run = await api.runRecipe(recipe.id, body);
  upsertRun(run, true);
  return run;
}
