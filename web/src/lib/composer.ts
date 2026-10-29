import { sameValue } from "./format";
import { readJson, writeJson } from "./prefs";
import { createStore, useStore } from "./store";
import type { Asset, InputRole, InputSlot, MediaKind, ModelSpec, LoraSelection, ParamSpec, ParamValue, ParamValues, Preset } from "./types";

// The Create draft. It lives outside the page so the Library can load settings into it
// ("Reuse settings", "Animate", "Use as reference") and it survives reloads.

export interface PendingUpload {
  name: string;
  kind: MediaKind;
  /** 0..1 from the browser's upload events; null before the first event. */
  progress: number | null;
  preview: string | null;
}

export interface AttachedInput {
  key: string;
  role: InputRole;
  asset?: Asset;
  upload?: PendingUpload;
}

export interface ComposerState {
  mode: MediaKind;
  modelByMode: Partial<Record<MediaKind, string>>;
  prompt: string;
  negativePrompt: string;
  /** Explicit choices per model id. Missing keys fall back to the preset, then the model default. */
  paramsByModel: Record<string, ParamValues>;
  inputsByMode: Record<MediaKind, AttachedInput[]>;
  presetByMode: Partial<Record<MediaKind, string | null>>;
  /** Extra models for a side-by-side compare run, per mode. */
  compareByMode: Partial<Record<MediaKind, string[]>>;
  /** A character whose images join as references, per mode. */
  characterByMode: Partial<Record<MediaKind, string | null>>;
  loraByModel: Record<string, LoraSelection | null>;
  /** Outputs per run, per mode: 4 images do not turn into 4 videos. */
  countByMode: Partial<Record<MediaKind, number>>;
  /** Bumped when settings arrive from elsewhere, so the prompt can take focus. */
  revision: number;
}

const STORAGE_KEY = "pictesa:composer";
const initial: ComposerState = {
  mode: "image",
  modelByMode: {},
  prompt: "",
  negativePrompt: "",
  paramsByModel: {},
  inputsByMode: { image: [], video: [], audio: [], lora: [] },
  presetByMode: {},
  compareByMode: {},
  characterByMode: {},
  loraByModel: {},
  countByMode: {},
  revision: 0,
};

const validCount = (value: unknown): value is number => Number.isInteger(value) && (value as number) >= 1 && (value as number) <= 8;

function restore(): ComposerState {
  const saved = readJson<Partial<ComposerState> & { count?: number }>(STORAGE_KEY, {});
  const inputs = saved.inputsByMode ?? initial.inputsByMode;
  const keep = (list: AttachedInput[] | undefined) => (Array.isArray(list) ? list.filter((item) => item?.asset?.id && item.role) : []);
  const mode = saved.mode === "video" || saved.mode === "audio" ? saved.mode : "image";
  const { count: legacyCount, ...rest } = saved;
  const counts = Object.entries(saved.countByMode ?? (validCount(legacyCount) ? { [mode]: legacyCount } : {})).filter(([, value]) => validCount(value));
  return {
    ...initial,
    ...rest,
    mode,
    countByMode: Object.fromEntries(counts),
    inputsByMode: { image: keep(inputs.image), video: keep(inputs.video), audio: keep(inputs.audio), lora: [] },
    revision: 0,
  };
}

export const composer = createStore<ComposerState>(restore());
export const useComposer = () => useStore(composer);

let saveTimer: ReturnType<typeof setTimeout> | undefined;
composer.subscribe(() => {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const state = composer.get();
    const keep = (list: AttachedInput[]) => list.filter((item) => item.asset).map(({ key, role, asset }) => ({ key, role, asset }));
    // Keep the per-model memory small.
    const recent = Object.entries(state.paramsByModel).slice(-30);
    writeJson(STORAGE_KEY, {
      ...state,
      paramsByModel: Object.fromEntries(recent),
      inputsByMode: { image: keep(state.inputsByMode.image), video: keep(state.inputsByMode.video), audio: keep(state.inputsByMode.audio), lora: [] },
      revision: 0,
    });
  }, 250);
});

const update = (change: (state: ComposerState) => Partial<ComposerState>) => composer.set((state) => ({ ...state, ...change(state) }));

let keySeed = 0;
export const inputKey = () => `in-${Date.now().toString(36)}-${(keySeed++).toString(36)}`;

export function isValidParam(spec: ParamSpec, value: ParamValue | undefined): boolean {
  if (value === undefined) return false;
  if (value === null) return spec.type === "seed";
  switch (spec.type) {
    case "enum":
      return spec.options.some((option) => sameValue(option.value, value));
    case "int":
      return typeof value === "number" && Number.isInteger(value) && value >= spec.min && value <= spec.max;
    case "float":
      return typeof value === "number" && Number.isFinite(value) && value >= spec.min && value <= spec.max;
    case "bool":
      return typeof value === "boolean";
    case "text":
      return typeof value === "string" && (!spec.max_len || [...value].length <= spec.max_len);
    case "seed":
      return typeof value === "number" && Number.isInteger(value) && value >= 0;
    default:
      return typeof value === "string" || typeof value === "number" || typeof value === "boolean";
  }
}

/** Values the controls show: explicit choice, else preset value, else model default. */
export function effectiveParams(model: ModelSpec, explicit: ParamValues = {}, preset?: Preset | null): ParamValues {
  const values: ParamValues = {};
  for (const spec of model.params) {
    const chosen = explicit[spec.key];
    const fromPreset = preset?.params?.[spec.key];
    if (spec.key in explicit && isValidParam(spec, chosen)) values[spec.key] = chosen ?? null;
    else if (fromPreset !== undefined && isValidParam(spec, fromPreset)) values[spec.key] = fromPreset;
    else values[spec.key] = spec.default ?? null;
  }
  return values;
}

/** Params sent to the server: only explicit choices, so preset params still apply where the user chose nothing. */
export function requestParams(model: ModelSpec, explicit: ParamValues = {}): ParamValues {
  const values: ParamValues = {};
  for (const spec of model.params) {
    const value = explicit[spec.key];
    if (spec.key in explicit && value !== null && isValidParam(spec, value)) values[spec.key] = value;
  }
  return values;
}

export const inputKind = (input: AttachedInput): MediaKind | undefined => input.asset?.kind ?? input.upload?.kind;

/** Inputs that fit each of the model's slots. Others stay parked for when a fitting model returns. */
export function slotInputs(slot: InputSlot, inputs: AttachedInput[]): AttachedInput[] {
  return inputs.filter((input) => input.role === slot.role && inputKind(input) === slot.kind).slice(0, slot.max);
}

export function activeInputs(model: ModelSpec | undefined, inputs: AttachedInput[]): AttachedInput[] {
  if (!model) return [];
  return model.inputs.flatMap((slot) => slotInputs(slot, inputs));
}

export const setLora = (model: string, selection: LoraSelection | null) => update((state) => ({ loraByModel: { ...state.loraByModel, [model]: selection } }));

export const setMode = (mode: MediaKind) => update(() => ({ mode }));
export const setPrompt = (prompt: string) => update(() => ({ prompt }));
export const setNegativePrompt = (negativePrompt: string) => update(() => ({ negativePrompt }));
export const setCount = (mode: MediaKind, count: number) => update((state) => ({ countByMode: { ...state.countByMode, [mode]: count } }));
export const countOf = (state: ComposerState, mode: MediaKind) => state.countByMode[mode] ?? 1;
export const setPreset = (mode: MediaKind, preset: string | null) => update((state) => ({ presetByMode: { ...state.presetByMode, [mode]: preset } }));
export const setCompare = (mode: MediaKind, models: string[]) => update((state) => ({ compareByMode: { ...state.compareByMode, [mode]: models } }));
export const setCharacter = (mode: MediaKind, character: string | null) => update((state) => ({ characterByMode: { ...state.characterByMode, [mode]: character } }));

/** Whether a model can take a character: it needs an image reference slot. */
export const takesCharacter = (model: ModelSpec | undefined) => Boolean(model?.inputs.some((slot) => slot.role === "reference" && slot.kind === "image") || model?.meta?.supports_loras);

/** Selects a model. A model used for the first time inherits fitting choices from the previous one. */
export function selectModel(mode: MediaKind, next: ModelSpec, previous?: ModelSpec) {
  update((state) => {
    const paramsByModel = { ...state.paramsByModel };
    // A listed-only spec has no params yet; the carry-over happens once its full spec loads.
    if (!paramsByModel[next.id] && previous && previous.id !== next.id && next.complete !== false) {
      const carried: ParamValues = {};
      const old = paramsByModel[previous.id] ?? {};
      for (const spec of next.params) {
        if (spec.key in old && isValidParam(spec, old[spec.key])) carried[spec.key] = old[spec.key] ?? null;
      }
      paramsByModel[next.id] = carried;
    }
    return { modelByMode: { ...state.modelByMode, [mode]: next.id }, paramsByModel };
  });
}

export function setParam(modelId: string, key: string, value: ParamValue | undefined) {
  update((state) => {
    const current = { ...(state.paramsByModel[modelId] ?? {}) };
    if (value === undefined) delete current[key];
    else current[key] = value;
    // Re-insert so the most recently used models stay in the saved memory.
    const { [modelId]: _old, ...rest } = state.paramsByModel;
    return { paramsByModel: { ...rest, [modelId]: current } };
  });
}

export const resetParams = (modelId: string) => update((state) => ({ paramsByModel: { ...state.paramsByModel, [modelId]: {} } }));

export function addInput(mode: MediaKind, input: AttachedInput) {
  update((state) => ({ inputsByMode: { ...state.inputsByMode, [mode]: [...state.inputsByMode[mode], input] } }));
}

export function patchInput(key: string, patch: Partial<AttachedInput>) {
  update((state) => ({
    inputsByMode: Object.fromEntries(
      Object.entries(state.inputsByMode).map(([mode, list]) => [mode, list.map((item) => (item.key === key ? { ...item, ...patch } : item))]),
    ) as ComposerState["inputsByMode"],
  }));
}

export function removeInput(key: string) {
  update((state) => ({
    inputsByMode: Object.fromEntries(
      Object.entries(state.inputsByMode).map(([mode, list]) => [mode, list.filter((item) => item.key !== key)]),
    ) as ComposerState["inputsByMode"],
  }));
}

/** Puts an asset into a slot of the given mode, replacing the oldest input when the slot is full. */
export function placeAsset(mode: MediaKind, role: InputRole, asset: Asset, max = 1) {
  update((state) => {
    const list = state.inputsByMode[mode].filter((item) => !(item.role === role && item.asset?.id === asset.id));
    const same = list.filter((item) => item.role === role && inputKind(item) === asset.kind);
    const overflow = new Set(same.slice(0, Math.max(0, same.length - max + 1)).map((item) => item.key));
    const kept = list.filter((item) => !overflow.has(item.key));
    return { mode, inputsByMode: { ...state.inputsByMode, [mode]: [...kept, { key: inputKey(), role, asset }] }, revision: state.revision + 1 };
  });
}

export interface LoadedSettings {
  mode: MediaKind;
  model: ModelSpec | null;
  prompt: string;
  negativePrompt: string | null;
  params: ParamValues;
  inputs: { role: InputRole; asset: Asset }[];
  preset: string | null;
  count: number;
}

/** "Reuse settings": replaces the draft for that mode with a finished job's settings. */
export function loadSettings(settings: LoadedSettings) {
  update((state) => ({
    mode: settings.mode,
    modelByMode: settings.model ? { ...state.modelByMode, [settings.mode]: settings.model.id } : state.modelByMode,
    loraByModel: settings.model ? { ...state.loraByModel, [settings.model.id]: Array.isArray(settings.params.loras) ? settings.params.loras[0] ?? null : null } : state.loraByModel,
    prompt: settings.prompt,
    negativePrompt: settings.negativePrompt ?? "",
    paramsByModel: settings.model ? { ...state.paramsByModel, [settings.model.id]: { ...settings.params } } : state.paramsByModel,
    inputsByMode: { ...state.inputsByMode, [settings.mode]: settings.inputs.map((input) => ({ key: inputKey(), ...input })) },
    presetByMode: { ...state.presetByMode, [settings.mode]: settings.preset },
    countByMode: { ...state.countByMode, [settings.mode]: Math.max(1, Math.min(8, settings.count || 1)) },
    revision: state.revision + 1,
  }));
}
