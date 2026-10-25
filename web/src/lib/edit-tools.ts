import type { IconName } from "../ui/icon";
import type { InputRole, ModelSpec, Padding, ParamValues } from "./types";

// The edit view's tools and every rule that decides which models fit each one.

export type EditTool = "inpaint" | "expand" | "relight" | "remove_background";

export interface ToolInfo {
  id: EditTool;
  label: string;
  icon: IconName;
  /** What a fitting model must do, for the "none set up" message. */
  needs: string;
}

export const editTools: ToolInfo[] = [
  { id: "inpaint", label: "Inpaint", icon: "brush", needs: "an inpainting model (one that takes a mask), such as FLUX Fill on fal, GPT Image or a ComfyUI inpainting workflow" },
  { id: "expand", label: "Expand", icon: "expand_content", needs: "a model that takes a mask, such as FLUX Fill on fal or a ComfyUI inpainting workflow" },
  { id: "relight", label: "Relight", icon: "light_mode", needs: "a relight model such as IC-Light on fal, or an image editing model" },
  { id: "remove_background", label: "Remove background", icon: "background_replace", needs: "a background removal model such as BiRefNet or rembg on fal" },
];

// Gateway models are listed before their inputs are known (`complete: false`). Until the full spec
// loads, well-known ids decide; a loaded spec's inputs always win.
const ids = {
  removeBackground: /birefnet|rembg|remove[-_ ]?(?:bg|background)|background[-_ ]?remov|bg[-_ ]?remov|matting/,
  relight: /relight|ic-?light/,
  maskEdit: /inpaint|(?:^|[/:_\s-])fill(?:$|[/_\s-])/,
  eraser: /eras(?:e|er)\b|eraser|object[-_ ]?remov/,
  /** Specialised image-to-image models that are no use as a general editor. */
  special: /upscal|eras|remov|birefnet|rembg|matting|depth|pose|canny|segment|mask|inpaint|fill|relight|ic-?light|lora|train|sketch|control|vector|layer/,
};

const imageSlot = (model: ModelSpec) => model.inputs.find((slot) => slot.kind === "image" && slot.role !== "mask");
const takesMask = (model: ModelSpec) => model.inputs.some((slot) => slot.kind === "image" && slot.role === "mask");

/**
 * How well a model fits a tool: null when it does not fit, higher is better. The best fit is the
 * default; ties go to featured models.
 */
export function fitScore(tool: EditTool, model: ModelSpec): number | null {
  if (model.output !== "image") return null;
  const text = `${model.id} ${model.name}`.toLowerCase();
  const known = model.complete !== false;
  const featured = model.tags?.includes("featured") ? 1 : 0;
  switch (tool) {
    case "inpaint":
    case "expand": {
      if (ids.eraser.test(text) || ids.removeBackground.test(text)) return null;
      // Inpainting needs words for the painted area; expanding can fill without them.
      if (known) return takesMask(model) && imageSlot(model) && (tool === "expand" || model.prompt) ? 4 + (ids.maskEdit.test(text) ? 2 : 0) + featured : null;
      return ids.maskEdit.test(text) ? 3 + featured : null;
    }
    case "relight": {
      if (ids.relight.test(text)) return !known || imageSlot(model) ? 10 : null;
      if (!model.tasks.includes("image_edit") || ids.special.test(text)) return null;
      // A general editor follows a written lighting instruction. Unloaded gateway models qualify
      // only when featured, so the list stays short and trustworthy.
      if (known) return imageSlot(model) && model.prompt ? 2 + featured : null;
      return featured ? 2 : null;
    }
    case "remove_background":
      return ids.removeBackground.test(text) && (!known || imageSlot(model)) ? 10 : null;
  }
}

/** Configured models that fit a tool, best first. */
export function toolModels(tool: EditTool, models: ModelSpec[]): ModelSpec[] {
  return models
    .filter((model) => model.configured)
    .map((model) => ({ model, score: fitScore(tool, model) }))
    .filter((item): item is { model: ModelSpec; score: number } => item.score !== null)
    .sort((a, b) => b.score - a.score || a.model.name.localeCompare(b.model.name))
    .map((item) => item.model);
}

/** Whether a relight run uses a dedicated relight model (true) or a general editor (false). */
export const isRelightModel = (model: ModelSpec) => ids.relight.test(`${model.id} ${model.name}`.toLowerCase());

/** The role the edited image goes in: the model's first image slot that is not the mask. */
export const imageRole = (model: ModelSpec): InputRole => imageSlot(model)?.role ?? "reference";

/** Why a loaded model cannot run a tool, or null when it can. */
export function unfitReason(tool: EditTool, model: ModelSpec): string | null {
  if (model.complete === false || fitScore(tool, model) !== null) return null;
  if ((tool === "inpaint" || tool === "expand") && !takesMask(model)) return `${model.name} does not take a mask. Choose another model.`;
  if (!imageSlot(model)) return `${model.name} does not take an image. Choose another model.`;
  return `${model.name} does not fit this tool. Choose another model.`;
}

// ---------- requests ----------

/** Keeps the result at the image's shape: "auto" or "match input" when offered, else the closest ratio. */
export function shapeParams(model: ModelSpec, width: number | null, height: number | null): ParamValues {
  const spec = model.params.find((param) => param.key === "aspect_ratio");
  if (!spec || spec.type !== "enum" || !width || !height) return {};
  const values = spec.options.map((option) => option.value).filter((value): value is string => typeof value === "string");
  const same = values.find((value) => /^(auto|match|original|input|same)/i.test(value));
  if (same) return { aspect_ratio: same };
  const target = Math.log(width / height);
  let best: string | null = null;
  let distance = Infinity;
  for (const value of values) {
    const match = /^(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/.exec(value);
    if (!match) continue;
    const gap = Math.abs(Math.log(Number(match[1]) / Number(match[2])) - target);
    if (gap < distance) {
      distance = gap;
      best = value;
    }
  }
  return best ? { aspect_ratio: best } : {};
}

export type LightDirection = "top_left" | "top" | "top_right" | "left" | "front" | "right" | "bottom_left" | "bottom" | "bottom_right";

/** The 3 x 3 pad, row by row. */
export const lightDirections: { id: LightDirection; label: string }[] = [
  { id: "top_left", label: "top left" },
  { id: "top", label: "top" },
  { id: "top_right", label: "top right" },
  { id: "left", label: "left" },
  { id: "front", label: "front" },
  { id: "right", label: "right" },
  { id: "bottom_left", label: "bottom left" },
  { id: "bottom", label: "bottom" },
  { id: "bottom_right", label: "bottom right" },
];

export const lightMoods = ["Soft daylight", "Golden hour", "Studio softbox", "Overcast", "Neon night", "Candlelight", "Dramatic contrast", "Moonlight"];

/** A lighting description that relight models and general editors both understand. */
export function relightPrompt(model: ModelSpec, direction: LightDirection, mood: string, custom: string): string {
  const where = lightDirections.find((item) => item.id === direction)!.label;
  const light = [custom.trim(), mood.trim()].filter(Boolean).join(", ") || "Natural light";
  if (isRelightModel(model)) return `${light}, light from the ${where}`;
  return `Relight this photo with ${light.charAt(0).toLowerCase()}${light.slice(1)}, the main light coming from the ${where}. Keep the subject, composition and every detail the same.`;
}

/** Relight models that take a light direction get it as a setting too (IC-Light's "Left", "Right", ...). */
export function lightParams(model: ModelSpec, direction: LightDirection): ParamValues {
  const spec = model.params.find((param) => param.type === "enum" && /light|direction|initial_latent/i.test(param.key));
  if (!spec || spec.type !== "enum") return {};
  const words = direction === "front" ? ["none", "front"] : direction.split("_").reverse();
  const option = spec.options.find((item) => words.some((word) => String(item.value).toLowerCase() === word))
    ?? spec.options.find((item) => words.every((word) => String(item.value).toLowerCase().includes(word)));
  return option ? { [spec.key]: option.value } : {};
}

export const expandRatios = ["1:1", "4:5", "3:4", "2:3", "9:16", "16:9", "3:2", "4:3", "21:9"];

/** Space to add so the image reaches `ratio` ("16:9"), split evenly around it. Never crops. */
export function paddingForRatio(width: number, height: number, ratio: string): Padding {
  const [a, b] = ratio.split(":").map(Number);
  const target = a! / b!;
  if (width / height < target) {
    const extra = Math.max(0, Math.round(height * target) - width);
    return { left: Math.floor(extra / 2), right: Math.ceil(extra / 2), top: 0, bottom: 0 };
  }
  const extra = Math.max(0, Math.round(width / target) - height);
  return { left: 0, right: 0, top: Math.floor(extra / 2), bottom: Math.ceil(extra / 2) };
}

export const EXPAND_PROMPT = "Continue the scene naturally beyond the original edges, matching its light, perspective and style.";
