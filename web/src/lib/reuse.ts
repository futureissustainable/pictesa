import { api } from "./api";
import { loadModels, peekModels } from "./catalog";
import { composer, loadSettings, placeAsset, selectModel } from "./composer";
import { kindLabel, roleLabel } from "./format";
import { notify } from "./notices";
import { navigate } from "./router";
import type { Asset, AssetDetail, InputRole, MediaKind, ModelSpec } from "./types";

// Moves library items into Create: reuse a job's settings, or use an asset as an input.

const fits = (model: ModelSpec, role: InputRole, kind: MediaKind) => model.inputs.find((slot) => slot.role === role && slot.kind === kind);

/** Every (mode, role) pair some model accepts for this asset kind, for the "Use as" menu. */
export function inputUses(kind: MediaKind): { mode: MediaKind; role: InputRole; label: string }[] {
  const uses = new Map<string, { mode: MediaKind; role: InputRole; label: string }>();
  for (const mode of ["image", "video", "audio"] as const) {
    for (const model of peekModels(mode) ?? []) {
      for (const slot of model.inputs) {
        if (slot.kind !== kind) continue;
        const key = `${mode}:${slot.role}`;
        if (!uses.has(key)) uses.set(key, { mode, role: slot.role, label: `${roleLabel[slot.role]} for ${kindLabel[mode].toLowerCase()}` });
      }
    }
  }
  return [...uses.values()];
}

/** Puts the asset into a slot, switching to a model that has the slot when the current one does not. */
export async function sendToCreate(asset: Asset, mode: MediaKind, role: InputRole) {
  await loadModels(mode);
  const models = peekModels(mode) ?? [];
  const currentId = composer.get().modelByMode[mode];
  const current = models.find((model) => model.id === currentId);
  const candidates = models.filter((model) => fits(model, role, asset.kind));
  const target =
    (current && fits(current, role, asset.kind) ? current : undefined) ??
    candidates.find((model) => model.configured && model.tags?.includes("featured")) ??
    candidates.find((model) => model.configured) ??
    candidates[0];
  if (!target) {
    notify(`No ${kindLabel[mode].toLowerCase()} model takes a ${roleLabel[role].toLowerCase()} yet.`, { tone: "error" });
    return;
  }
  if (target.id !== current?.id) {
    selectModel(mode, target, current);
    notify(`Switched to ${target.name}, which takes a ${roleLabel[role].toLowerCase()}.`);
  }
  placeAsset(mode, role, asset, fits(target, role, asset.kind)?.max ?? 1);
  navigate("/", { replace: true });
}

/** "Reuse settings": model, prompt, params, preset, count and the original inputs. */
export async function reuseSettings(detail: AssetDetail) {
  const job = detail.job;
  if (!job) return;
  await loadModels(job.output);
  const model = (peekModels(job.output) ?? []).find((item) => item.id === job.model) ?? null;
  const results = await Promise.allSettled((job.inputs ?? []).map((input) => api.asset(input.asset_id).then((asset) => ({ role: input.role, asset: asset as Asset }))));
  const inputs = results.flatMap((result) => (result.status === "fulfilled" && !result.value.asset.deleted_at ? [result.value] : []));
  loadSettings({
    mode: job.output,
    model,
    prompt: job.prompt ?? "",
    negativePrompt: job.negative_prompt,
    params: job.params ?? {},
    inputs,
    preset: job.preset,
    count: job.count,
  });
  if (!model) notify(`${job.model_name} is not available any more. The prompt and inputs are loaded.`, { tone: "error" });
  else if (inputs.length < (job.inputs?.length ?? 0)) notify("Settings loaded. Some original inputs are no longer in the library.");
  navigate("/", { replace: true });
}
