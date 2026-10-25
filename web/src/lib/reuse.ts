import { api } from "./api";
import { loadFullModel, loadModels, peekModels } from "./catalog";
import { composer, loadSettings, placeAsset, selectModel, setCharacter, setMode, takesCharacter } from "./composer";
import { kindLabel, roleLabel } from "./format";
import { notify } from "./notices";
import { navigate } from "./router";
import type { Asset, AssetDetail, Character, InputRole, MediaKind, ModelSpec, ParamValues } from "./types";

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
  const listed = models.find((model) => model.id === currentId);
  // The current model may be a listed-only spec; its slots are known once the full spec loads.
  const current = (listed && (await loadFullModel(listed))) ?? undefined;
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
  if (!job) {
    if (!detail.model) return;
    await loadModels(detail.kind);
    const model = (peekModels(detail.kind) ?? []).find((item) => item.id === detail.model);
    if (!model) return notify("This model is not available in Pictesa.", { tone: "error" });
    const params = Object.fromEntries(Object.entries(detail.params ?? {}).filter(([, value]) => value === null || ["string", "number", "boolean"].includes(typeof value))) as ParamValues;
    loadSettings({ mode: detail.kind, model, prompt: detail.prompt ?? "", negativePrompt: detail.negative_prompt ?? null, params, inputs: [], preset: null, count: 1 });
    navigate("/", { replace: true });
    return;
  }
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

/** "Use in Create": picks the character for images, switching to a model that takes references if needed. */
export async function sendCharacterToCreate(character: Character) {
  const mode: MediaKind = "image";
  await loadModels(mode);
  const models = peekModels(mode) ?? [];
  const listed = models.find((model) => model.id === composer.get().modelByMode[mode]);
  const current = (listed && (await loadFullModel(listed))) ?? undefined;
  if (!takesCharacter(current)) {
    const usable = models.filter((model) => model.configured && takesCharacter(model));
    const target = usable.find((model) => model.tags?.includes("featured")) ?? usable[0];
    if (!target) {
      notify("None of your image models takes reference images yet. Set one up in Settings > Providers to use characters.");
      return;
    }
    selectModel(mode, target, current);
    notify(`Switched to ${target.name}, which takes reference images.`);
  }
  setCharacter(mode, character.id);
  setMode(mode);
  navigate("/");
}
