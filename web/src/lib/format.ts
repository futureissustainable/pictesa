import type { IconName } from "../ui/icon-names";
import type { InputRole, MediaKind, ModelSpec, ParamSpec, ParamValue, ParamValues, Pricing, ProviderKind, Task } from "./types";

const dayFormat = new Intl.DateTimeFormat("en-GB", { day: "2-digit", month: "2-digit", year: "numeric" });
const timeFormat = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });

/** 24.10.2026 */
export const formatDate = (ms: number) => dayFormat.format(new Date(ms)).replaceAll("/", ".");
/** 24.10.2026, 18:20 */
export const formatDateTime = (ms: number) => `${formatDate(ms)}, ${timeFormat.format(new Date(ms))}`;

/** 0:05, 12:40, 1:02:03 */
export function clock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = String(total % 60).padStart(2, "0");
  return hours ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}` : `${minutes}:${seconds}`;
}

export function bytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${Math.round(size / 1024)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/** Totals: $0.56. */
export function money(usd: number): string {
  if (usd > 0 && usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(2)}`;
}

/** Unit prices keep their precision: $0.112, $0.0016, $0.50. */
export function unitMoney(usd: number): string {
  const cents = usd * 100;
  if (Math.abs(cents - Math.round(cents)) < 1e-9) return `$${usd.toFixed(2)}`;
  return `$${parseFloat(usd.toFixed(4))}`;
}

const units: Record<string, string> = { per_run: "training", per_step: "step", per_image: "image", per_second: "s", per_video: "video", per_megapixel: "MP" };

/** Short price hint for the model picker, or null when the provider publishes none. */
export function priceHint(pricing: Pricing | null | undefined): string | null {
  if (!pricing) return null;
  if (pricing.unit === "local") return "Local";
  if (pricing.unit === "variable") return pricing.note || "Varies";
  return `${unitMoney(pricing.usd)}/${units[pricing.unit]}`;
}

function numeric(value: ParamValue | undefined): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = parseFloat(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

export type Estimate = { kind: "total"; usd: number } | { kind: "unit"; text: string };

/**
 * Cost estimate from the model's published pricing. Only computed when every factor is known;
 * otherwise the unit price is returned so nothing is invented.
 */
export function estimateCost(model: ModelSpec, values: ParamValues, count: number): Estimate | null {
  const standard = model.pricing;
  if (!standard || standard.unit === "local" || standard.unit === "variable") return null;
  const pricing = model.meta?.batch === true && values.speed === "batch" ? { ...standard, usd: standard.usd * 0.5 } : standard;
  const unitText = priceHint(pricing) ?? "";
  switch (pricing.unit) {
    case "per_run": return { kind: "total", usd: pricing.usd };
    case "per_step": return { kind: "unit", text: unitText };
    case "per_image":
    case "per_video":
      return { kind: "total", usd: pricing.usd * count };
    case "per_second": {
      const seconds = numeric(values.duration);
      return seconds ? { kind: "total", usd: pricing.usd * seconds * count } : { kind: "unit", text: unitText };
    }
    case "per_megapixel": {
      const width = numeric(values.width);
      const height = numeric(values.height);
      return width && height ? { kind: "total", usd: pricing.usd * ((width * height) / 1e6) * count } : { kind: "unit", text: unitText };
    }
  }
}

export const kindLabel: Record<MediaKind, string> = { image: "Image", video: "Video", audio: "Audio", lora: "LoRA" };
export const kindPlural: Record<MediaKind, string> = { image: "images", video: "videos", audio: "tracks", lora: "LoRAs" };
export const kindIcon: Record<MediaKind, IconName> = { image: "image", video: "movie", audio: "graphic_eq", lora: "tune" };

export const roleLabel: Record<InputRole, string> = {
  reference: "Reference",
  start_frame: "Start frame",
  end_frame: "End frame",
  video: "Video",
  audio: "Audio",
  mask: "Mask",
};
export const roleIcon: Record<InputRole, IconName> = {
  reference: "add_photo_alternate",
  start_frame: "first_page",
  end_frame: "last_page",
  video: "movie",
  audio: "graphic_eq",
  mask: "ink_eraser",
};

export const taskLabel: Record<Task, string> = {
  text_to_image: "Text to image",
  image_edit: "Image edit",
  text_to_video: "Text to video",
  image_to_video: "Image to video",
  video_to_video: "Video to video",
  video_to_audio: "Video to audio",
  upscale: "Upscale",
  lip_sync: "Lip sync",
  text_to_audio: "Text to audio",
  training: "Training",
};
export const taskName = (task: string) => taskLabel[task as Task] ?? task.replaceAll("_", " ");

export const providerKindIcon: Record<ProviderKind, IconName> = { local: "computer", gateway: "hub", cloud: "cloud" };

/** Icons for the shared parameter vocabulary (crates/pictesa-core keys) and common gateway names. */
const paramIcons: Record<string, IconName> = {
  aspect_ratio: "aspect_ratio",
  image_size: "aspect_ratio",
  size: "aspect_ratio",
  resolution: "high_quality",
  quality: "high_quality",
  duration: "timer",
  fps: "speed",
  seed: "casino",
  guidance: "tune",
  guidance_scale: "tune",
  cfg_scale: "tune",
  steps: "stairs",
  num_inference_steps: "stairs",
  strength: "opacity",
  output_format: "draft",
  generate_audio: "volume_up",
  camera_motion: "videocam",
  enhance_prompt: "auto_awesome",
  style: "palette",
  scale: "open_in_full",
  loop: "repeat",
  width: "width",
  height: "height",
};

export const paramIcon = (key: string): IconName | undefined => paramIcons[key];

export function optionLabel(spec: ParamSpec, value: ParamValue | undefined): string {
  if (value === null || value === undefined || value === "") return "Default";
  if (spec.type === "enum") {
    const option = spec.options.find((item) => sameValue(item.value, value));
    if (option) return option.label;
  }
  if (spec.type === "bool") return value ? "On" : "Off";
  if (spec.key === "duration" && typeof value === "number") return `${value}s`;
  return String(value);
}

/** Enum values may be numbers on one side and strings on the other ("5" and 5). */
export function sameValue(a: ParamValue | undefined, b: ParamValue | undefined): boolean {
  if (a === b) return true;
  if (a === null || b === null || a === undefined || b === undefined) return false;
  return String(a) === String(b);
}

export const isRatio = (text: string) => /^\d+(\.\d+)?\s*[:x×]\s*\d+(\.\d+)?$/.test(text.trim());

/** "16:9" -> 1.777; null when the text is not a ratio. */
export function ratioOf(text: string): number | null {
  const match = /^(\d+(?:\.\d+)?)\s*[:x×]\s*(\d+(?:\.\d+)?)$/.exec(text.trim());
  if (!match) return null;
  const ratio = Number(match[1]) / Number(match[2]);
  return Number.isFinite(ratio) && ratio > 0 ? ratio : null;
}

export function plural(count: number, one: string, many: string) {
  return `${count} ${count === 1 ? one : many}`;
}

export function dimensions(width: number | null, height: number | null): string | null {
  return width && height ? `${width} × ${height}` : null;
}

/** "landscape_16_9" -> 16/9, "portrait_4_3" -> 3/4, "square_hd" -> 1 (gateway size names). */
function namedRatio(text: string): number | null {
  const match = /(landscape|portrait)_(\d+)_(\d+)/.exec(text);
  if (match) {
    const a = Number(match[2]);
    const b = Number(match[3]);
    return match[1] === "landscape" ? a / b : b / a;
  }
  return /square/.test(text) ? 1 : null;
}

/** Width over height a run asked for, from its params; 1 when they do not say. */
export function requestedRatio(params: ParamValues): number {
  const width = numeric(params.width);
  const height = numeric(params.height);
  if (width && height) return width / height;
  for (const key of ["aspect_ratio", "image_size", "size", "ratio"]) {
    const value = params[key];
    if (typeof value !== "string") continue;
    const ratio = ratioOf(value) ?? namedRatio(value);
    if (ratio) return ratio;
  }
  return 1;
}
