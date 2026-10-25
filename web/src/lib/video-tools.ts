import type { IconName } from "../ui/icon";
import type { ModelSpec } from "./types";

export type VideoTool = "trim" | "join" | "frame" | "smooth" | "add-audio" | "extend" | "upscale" | "sound";
export const videoTools: {
  id: VideoTool;
  label: string;
  icon: IconName;
  needs: string;
}[] = [
  {
    id: "trim",
    label: "Trim",
    icon: "edit",
    needs: "ffmpeg and ffprobe installed on PATH",
  },
  {
    id: "join",
    label: "Join clips",
    icon: "movie",
    needs: "ffmpeg and ffprobe installed on PATH",
  },
  {
    id: "frame",
    label: "Grab frame",
    icon: "camera",
    needs: "ffmpeg and ffprobe installed on PATH",
  },
  {
    id: "smooth",
    label: "Smooth frames",
    icon: "animation",
    needs: "ffmpeg and ffprobe installed on PATH",
  },
  {
    id: "add-audio",
    label: "Add audio",
    icon: "volume_up",
    needs: "ffmpeg and ffprobe installed on PATH",
  },
  {
    id: "extend",
    label: "Extend",
    icon: "arrow_forward",
    needs: "a video extension model, such as fal extend-video",
  },
  {
    id: "upscale",
    label: "Upscale",
    icon: "high_quality",
    needs: "a video upscaler, such as Topaz or SeedVR on fal",
  },
  {
    id: "sound",
    label: "Create sound",
    icon: "graphic_eq",
    needs: "a video-to-audio model, such as MMAudio on fal",
  },
];

/** All video tool matching lives here. Resolved input slots take priority over catalog names. */
export function videoFit(tool: VideoTool, model: ModelSpec): number | null {
  if (!["extend", "upscale", "sound"].includes(tool)) return model.id === `local:${tool}` ? 100 : null;
  const known = model.complete !== false;
  const text = `${model.id} ${model.name}`.toLowerCase();
  const videoSlot = model.inputs.find((slot) => slot.kind === "video");
  if (known && (!videoSlot || videoSlot.min > 1 || model.inputs.some((slot) => slot !== videoSlot && slot.min > 0))) return null;
  const soundModel = /video[-_/ ]to[-_/ ]audio|mmaudio|foley|thinksound/.test(text);
  if (tool === "sound") {
    if (model.output !== "audio" && !(model.output === "video" && soundModel)) return null;
  } else if (model.output !== "video") return null;
  let score: number;
  switch (tool) {
    case "extend":
      if (!/extend|extension/.test(text)) return null;
      score = /extend[-_/ ]video|video[-_/ ]extend/.test(text) ? 12 : 8;
      break;
    case "upscale":
      if (!/upscal|seedvr/.test(text)) return null;
      score = /topaz|seedvr/.test(text) ? 12 : 8;
      break;
    case "sound":
      if (!known && !soundModel) return null;
      score = /mmaudio|video[-_/ ]to[-_/ ]audio/.test(text) ? 12 : 8;
      break;
    default:
      return null;
  }
  return score + (model.tags?.includes("featured") ? 1 : 0);
}
export const videoToolModels = (tool: VideoTool, models: ModelSpec[]) =>
  models
    .filter((model) => model.configured && videoFit(tool, model) !== null)
    .sort((a, b) => videoFit(tool, b)! - videoFit(tool, a)! || a.name.localeCompare(b.name));
