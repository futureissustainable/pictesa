// Mock catalog for the dev server: providers, models and presets shaped like the Rust server's
// answers (crates/pictesa-core). Model names are only realistic examples for screens.

export type Value = string | number | boolean | null;

export interface Field {
  key: string;
  label: string;
  env: string[];
  secret: boolean;
  optional: boolean;
  default?: string;
  placeholder?: string;
}

export interface ProviderSeed {
  id: string;
  name: string;
  kind: "cloud" | "gateway" | "local";
  fields: Field[];
  signup_url?: string;
  docs_url?: string;
  /** Simulated environment variables and stored values. */
  env: Record<string, string>;
  stored: Record<string, string>;
  /** Local servers: whether something answers at the URL. */
  reachable?: boolean;
  progress: boolean;
}

const apiKey = (env: string[]): Field => ({ key: "api_key", label: "API key", env, secret: true, optional: false });
const baseUrl = (value: string, env: string[]): Field => ({ key: "base_url", label: "Server URL", env, secret: false, optional: true, default: value, placeholder: value });

export const providers: ProviderSeed[] = [
  { id: "comfyui", name: "ComfyUI", kind: "local", fields: [baseUrl("http://127.0.0.1:8188", ["COMFYUI_URL"])], docs_url: "https://docs.comfy.org", env: {}, stored: {}, reachable: true, progress: true },
  { id: "sdcpp", name: "stable-diffusion.cpp", kind: "local", fields: [baseUrl("http://127.0.0.1:7860", ["SDCPP_URL"])], docs_url: "https://github.com/leejet/stable-diffusion.cpp", env: {}, stored: {}, reachable: false, progress: true },
  { id: "fal", name: "fal", kind: "gateway", fields: [apiKey(["FAL_KEY"])], signup_url: "https://fal.ai/dashboard/keys", docs_url: "https://docs.fal.ai", env: { FAL_KEY: "set" }, stored: {}, progress: true },
  { id: "replicate", name: "Replicate", kind: "gateway", fields: [apiKey(["REPLICATE_API_TOKEN"])], signup_url: "https://replicate.com/account/api-tokens", docs_url: "https://replicate.com/docs", env: {}, stored: {}, progress: true },
  { id: "openai", name: "OpenAI", kind: "cloud", fields: [apiKey(["OPENAI_API_KEY"]), { key: "organization", label: "Organization ID", env: ["OPENAI_ORG_ID"], secret: false, optional: true, placeholder: "org-..." }], signup_url: "https://platform.openai.com/api-keys", docs_url: "https://platform.openai.com/docs", env: {}, stored: { api_key: "sk-stored" }, progress: false },
  { id: "google", name: "Google Gemini", kind: "cloud", fields: [apiKey(["GEMINI_API_KEY", "GOOGLE_API_KEY"])], signup_url: "https://aistudio.google.com/apikey", docs_url: "https://ai.google.dev/gemini-api/docs", env: {}, stored: {}, progress: false },
  { id: "runway", name: "Runway", kind: "cloud", fields: [apiKey(["RUNWAYML_API_SECRET"])], signup_url: "https://dev.runwayml.com", docs_url: "https://docs.dev.runwayml.com", env: {}, stored: {}, progress: true },
];

type Param = Record<string, unknown> & { key: string; label: string; type: string };

const ratios = (values: string[], fallback: string): Param => ({ key: "aspect_ratio", label: "Aspect ratio", type: "enum", options: values.map((v) => ({ value: v, label: v })), default: fallback, advanced: false });
const choices = (key: string, label: string, values: Value[], fallback: Value, advanced = false, labels?: string[]): Param => ({
  key,
  label,
  type: "enum",
  options: values.map((value, index) => ({ value, label: labels?.[index] ?? String(value) })),
  default: fallback,
  advanced,
});
const duration = (values: number[], fallback: number): Param => choices("duration", "Duration", values, fallback, false, values.map((v) => `${v}s`));
const seed: Param = { key: "seed", label: "Seed", type: "seed", advanced: true };
const sound = (fallback: boolean): Param => ({ key: "generate_audio", label: "Sound", type: "bool", default: fallback, advanced: false });
const int = (key: string, label: string, min: number, max: number, fallback: number, advanced = true, step = 1, help?: string): Param => ({ key, label, type: "int", min, max, step, default: fallback, advanced, help });
const float = (key: string, label: string, min: number, max: number, fallback: number, step = 0.1, help?: string): Param => ({ key, label, type: "float", min, max, step, default: fallback, advanced: true, help });
const format: Param = choices("output_format", "Format", ["png", "jpeg", "webp"], "png", true, ["PNG", "JPEG", "WebP"]);
const imageRatios = ["1:1", "4:5", "3:4", "2:3", "9:16", "16:9", "3:2", "4:3", "21:9"];

const slot = (role: string, kind: string, max: number, min = 0, label?: string) => ({ role, kind, min, max, ...(label ? { label } : {}) });

export interface ModelSeed {
  id: string;
  provider: string;
  name: string;
  vendor?: string;
  output: "image" | "video" | "audio";
  tasks: string[];
  inputs: ReturnType<typeof slot>[];
  params: Param[];
  prompt: boolean;
  negative_prompt: boolean;
  max_outputs: number;
  pricing?: Record<string, unknown>;
  open_weights: boolean;
  description?: string;
  tags: string[];
  /** Listed without params or inputs until GET /api/models/{id}, like discovered gateway models. */
  discovered?: boolean;
}

const model = (spec: Partial<ModelSeed> & Pick<ModelSeed, "id" | "provider" | "name" | "output" | "tasks">): ModelSeed => ({
  inputs: [],
  params: [],
  prompt: true,
  negative_prompt: false,
  max_outputs: 1,
  open_weights: false,
  tags: [],
  ...spec,
});

const cameraMoves = ["none", "static", "dolly_in", "dolly_out", "orbit_left", "orbit_right", "crane_up", "crane_down", "pan_left", "pan_right", "handheld"];

export const models: ModelSeed[] = [
  model({
    id: "fal:fal-ai/nano-banana-pro", provider: "fal", name: "Nano Banana Pro", vendor: "Google", output: "image", tasks: ["text_to_image", "image_edit"],
    inputs: [slot("reference", "image", 14, 0, "References")],
    params: [ratios(["auto", ...imageRatios], "auto"), choices("resolution", "Resolution", ["1K", "2K", "4K"], "1K"), format, seed],
    max_outputs: 4, pricing: { unit: "per_image", usd: 0.15 }, tags: ["featured"], description: "Strong prompt following and text rendering. Edits from up to 14 references.",
  }),
  model({
    id: "fal:fal-ai/flux-2-pro", provider: "fal", name: "FLUX.2 Pro", vendor: "Black Forest Labs", output: "image", tasks: ["text_to_image", "image_edit"],
    inputs: [slot("reference", "image", 8, 0, "References")],
    params: [ratios(imageRatios, "1:1"), { key: "enhance_prompt", label: "Enhance prompt", type: "bool", default: false, advanced: false }, float("guidance", "Guidance", 1.5, 10, 3.5), int("steps", "Steps", 10, 50, 28), seed],
    max_outputs: 4, pricing: { unit: "per_megapixel", usd: 0.03 }, tags: ["featured"],
  }),
  model({
    id: "fal:fal-ai/flux-2-dev", provider: "fal", name: "FLUX.2 dev", vendor: "Black Forest Labs", output: "image", tasks: ["text_to_image"],
    params: [int("width", "Width", 256, 2048, 1024, false, 64), int("height", "Height", 256, 2048, 1024, false, 64), int("steps", "Steps", 4, 50, 28), float("guidance", "Guidance", 1, 10, 3.5), seed],
    max_outputs: 4, pricing: { unit: "per_megapixel", usd: 0.012 }, open_weights: true, negative_prompt: true,
  }),
  model({
    id: "fal:fal-ai/bytedance/seedream/v4", provider: "fal", name: "Seedream 4", vendor: "ByteDance", output: "image", tasks: ["text_to_image", "image_edit"],
    inputs: [slot("reference", "image", 10, 0, "References")],
    params: [choices("image_size", "Image size", ["square_hd", "portrait_4_3", "portrait_16_9", "landscape_4_3", "landscape_16_9"], "square_hd", false, ["Square HD", "Portrait 3:4", "Portrait 9:16", "Landscape 4:3", "Landscape 16:9"]), seed],
    max_outputs: 4, pricing: { unit: "per_image", usd: 0.03 }, discovered: true,
  }),
  model({
    id: "openai:gpt-image-1.5", provider: "openai", name: "GPT Image 1.5", vendor: "OpenAI", output: "image", tasks: ["text_to_image", "image_edit"],
    inputs: [slot("reference", "image", 16, 0, "References"), slot("mask", "image", 1)],
    params: [choices("size", "Size", ["auto", "1024x1024", "1536x1024", "1024x1536"], "auto", false, ["Auto", "Square", "Landscape", "Portrait"]), choices("quality", "Quality", ["low", "medium", "high"], "medium", false, ["Low", "Medium", "High"]), choices("background", "Background", ["auto", "transparent", "opaque"], "auto", true, ["Auto", "Transparent", "Opaque"]), format],
    max_outputs: 4, pricing: { unit: "variable", note: "Billed by tokens" },
  }),
  model({
    id: "fal:fal-ai/topaz/upscale/image", provider: "fal", name: "Topaz Upscale", vendor: "Topaz Labs", output: "image", tasks: ["upscale"],
    inputs: [slot("reference", "image", 1, 1, "Image")], prompt: false,
    params: [choices("scale", "Upscale", [2, 4], 2, false, ["2×", "4×"]), { key: "face_enhancement", label: "Face enhancement", type: "bool", default: true, advanced: true }],
    pricing: { unit: "per_megapixel", usd: 0.08 },
  }),
  model({
    id: "comfyui:workflows/flux-dev", provider: "comfyui", name: "FLUX.1 dev workflow", vendor: "Black Forest Labs", output: "image", tasks: ["text_to_image"],
    params: [int("width", "Width", 512, 2048, 1024, false, 64), int("height", "Height", 512, 2048, 1024, false, 64), int("steps", "Steps", 1, 60, 20, false), float("guidance", "Guidance", 1, 10, 3.5), seed],
    negative_prompt: true, max_outputs: 4, pricing: { unit: "local" }, open_weights: true, description: "Your ComfyUI workflow flux-dev.json.",
  }),
  model({
    id: "sdcpp:sd3.5-medium", provider: "sdcpp", name: "SD 3.5 Medium", vendor: "Stability AI", output: "image", tasks: ["text_to_image"],
    params: [int("width", "Width", 512, 1536, 1024, false, 64), int("height", "Height", 512, 1536, 1024, false, 64), int("steps", "Steps", 1, 60, 30), float("guidance", "Guidance", 1, 12, 4.5), choices("sampler", "Sampler", ["euler", "euler_a", "dpm++2m"], "euler", true), seed],
    negative_prompt: true, pricing: { unit: "local" }, open_weights: true,
  }),
  model({
    id: "google:imagen-4", provider: "google", name: "Imagen 4", vendor: "Google", output: "image", tasks: ["text_to_image"],
    params: [ratios(["1:1", "3:4", "4:3", "9:16", "16:9"], "1:1"), seed], max_outputs: 4, pricing: { unit: "per_image", usd: 0.04 },
  }),
  model({
    id: "replicate:ideogram-ai/ideogram-v3-turbo", provider: "replicate", name: "Ideogram 3 Turbo", vendor: "Ideogram", output: "image", tasks: ["text_to_image"],
    params: [ratios(["1:1", "3:4", "4:3", "9:16", "16:9"], "1:1"), choices("style", "Style", ["auto", "general", "realistic", "design"], "auto", false, ["Auto", "General", "Realistic", "Design"]), { key: "enhance_prompt", label: "Magic prompt", type: "bool", default: true, advanced: true }, seed],
    pricing: { unit: "per_image", usd: 0.03 },
  }),

  model({
    id: "fal:fal-ai/kling-video/v3/pro/image-to-video", provider: "fal", name: "Kling 3 Pro", vendor: "Kuaishou", output: "video", tasks: ["image_to_video"],
    inputs: [slot("start_frame", "image", 1, 1, "Start frame"), slot("end_frame", "image", 1, 0, "End frame")],
    params: [duration([5, 10], 5), sound(true), float("cfg_scale", "Prompt strength", 0, 1, 0.5, 0.05, "How closely the motion follows the prompt."), seed],
    negative_prompt: true, pricing: { unit: "per_second", usd: 0.112 }, tags: ["featured"],
  }),
  model({
    id: "fal:fal-ai/kling-video/v3/pro/text-to-video", provider: "fal", name: "Kling 3 Pro text", vendor: "Kuaishou", output: "video", tasks: ["text_to_video"],
    params: [ratios(["16:9", "9:16", "1:1"], "16:9"), duration([5, 10], 5), sound(true), seed], negative_prompt: true, pricing: { unit: "per_second", usd: 0.112 },
  }),
  model({
    id: "fal:fal-ai/veo3.1/fast", provider: "fal", name: "Veo 3.1 Fast", vendor: "Google", output: "video", tasks: ["text_to_video", "image_to_video"],
    inputs: [slot("start_frame", "image", 1, 0, "Start frame")],
    params: [ratios(["16:9", "9:16"], "16:9"), duration([4, 6, 8], 8), choices("resolution", "Resolution", ["720p", "1080p"], "720p"), sound(true), seed],
    pricing: { unit: "per_second", usd: 0.15 }, tags: ["featured"],
  }),
  model({
    id: "google:veo-3.1", provider: "google", name: "Veo 3.1", vendor: "Google", output: "video", tasks: ["text_to_video", "image_to_video"],
    inputs: [slot("start_frame", "image", 1), slot("reference", "image", 3, 0, "References")],
    params: [ratios(["16:9", "9:16"], "16:9"), duration([4, 6, 8], 8), choices("resolution", "Resolution", ["720p", "1080p"], "1080p"), sound(true), seed],
    pricing: { unit: "per_second", usd: 0.4 },
  }),
  model({
    id: "fal:fal-ai/wan/v2.5", provider: "fal", name: "Wan 2.5", vendor: "Alibaba", output: "video", tasks: ["text_to_video", "image_to_video"],
    inputs: [slot("start_frame", "image", 1)],
    params: [choices("resolution", "Resolution", ["480p", "720p", "1080p"], "720p"), duration([5, 10], 5), { key: "enhance_prompt", label: "Expand prompt", type: "bool", default: true, advanced: true }, seed],
    negative_prompt: true, open_weights: true, pricing: { unit: "per_second", usd: 0.05 }, discovered: true,
  }),
  model({
    id: "fal:fal-ai/ltx-2", provider: "fal", name: "LTX-2", vendor: "Lightricks", output: "video", tasks: ["text_to_video", "image_to_video"],
    inputs: [slot("start_frame", "image", 1)],
    params: [choices("camera_motion", "Camera", cameraMoves, "none", false, cameraMoves.map((v) => v.replace("_", " ").replace(/^./, (c) => c.toUpperCase()))), duration([6, 8, 10], 6), choices("fps", "Frame rate", [25, 50], 25, true, ["25 fps", "50 fps"]), sound(false), seed],
    open_weights: true, pricing: { unit: "per_second", usd: 0.04 },
  }),
  model({
    id: "runway:gen4_turbo", provider: "runway", name: "Gen-4 Turbo", vendor: "Runway", output: "video", tasks: ["image_to_video"],
    inputs: [slot("start_frame", "image", 1, 1, "Start frame")],
    params: [ratios(["16:9", "9:16", "1:1"], "16:9"), duration([5, 10], 5), seed], pricing: { unit: "per_second", usd: 0.05 },
  }),
  model({
    id: "comfyui:workflows/wan-2.2-i2v", provider: "comfyui", name: "Wan 2.2 image to video", vendor: "Alibaba", output: "video", tasks: ["image_to_video"],
    inputs: [slot("start_frame", "image", 1, 1, "Start frame")],
    params: [int("length", "Frames", 33, 121, 81, false, 8, "Frames at 16 fps."), int("steps", "Steps", 4, 40, 20), seed],
    negative_prompt: true, open_weights: true, pricing: { unit: "local" }, description: "Your ComfyUI workflow wan22-i2v.json.",
  }),
  model({
    id: "fal:fal-ai/sync-lipsync/v2", provider: "fal", name: "Lipsync 2", vendor: "Sync Labs", output: "video", tasks: ["lip_sync"],
    inputs: [slot("video", "video", 1, 1, "Video"), slot("audio", "audio", 1, 1, "Voice")], prompt: false,
    params: [choices("sync_mode", "When lengths differ", ["cut_off", "loop", "bounce", "silence"], "cut_off", true, ["Cut off", "Loop", "Bounce", "Silence"])],
    pricing: { unit: "per_second", usd: 0.05 },
  }),
  model({
    id: "fal:fal-ai/topaz/upscale/video", provider: "fal", name: "Topaz Video Upscale", vendor: "Topaz Labs", output: "video", tasks: ["upscale", "video_to_video"],
    inputs: [slot("video", "video", 1, 1, "Video")], prompt: false,
    params: [choices("scale", "Upscale", [2, 4], 2, false, ["2×", "4×"]), choices("fps", "Frame rate", [24, 30, 60], 30, true, ["24 fps", "30 fps", "60 fps"])],
    pricing: { unit: "per_second", usd: 0.08 },
  }),
  model({
    id: "replicate:minimax/hailuo-02", provider: "replicate", name: "Hailuo 02", vendor: "MiniMax", output: "video", tasks: ["text_to_video", "image_to_video"],
    inputs: [slot("start_frame", "image", 1)],
    params: [duration([6, 10], 6), choices("resolution", "Resolution", ["768p", "1080p"], "768p"), { key: "enhance_prompt", label: "Prompt optimizer", type: "bool", default: true, advanced: true }],
    pricing: { unit: "per_video", usd: 0.27 },
  }),

  model({
    id: "fal:fal-ai/stable-audio-25/text-to-audio", provider: "fal", name: "Stable Audio 2.5", vendor: "Stability AI", output: "audio", tasks: ["text_to_audio"],
    params: [int("duration", "Length in seconds", 5, 190, 30, false, 1), int("steps", "Steps", 4, 16, 8)],
    pricing: { unit: "variable", note: "Billed per track" },
  }),
];

export const presets = [
  ...[
    ["dolly-in", "Dolly in", "slow dolly-in toward the subject, steady motion", "dolly_in", "Camera glides forward toward the subject."],
    ["dolly-out", "Dolly out", "slow dolly-out revealing the scene", "dolly_out", "Camera pulls back to reveal the setting."],
    ["orbit", "Orbit", "camera orbits around the subject", "orbit_left", "A smooth arc around the subject."],
    ["crane-up", "Crane up", "camera cranes up and over the scene", "crane_up", "Rises to a high view."],
    ["pan-left", "Pan left", "slow pan to the left", "pan_left", "Turns across the scene."],
    ["handheld", "Handheld", "handheld camera, natural sway", "handheld", "Documentary feel."],
    ["static", "Locked off", "static camera on a tripod", "static", "No camera motion."],
    ["fpv", "FPV drone", "fast FPV drone flight through the scene", null, "Fast, close flight."],
  ].map(([id, name, suffix, motion, description]) => ({
    id: `camera/${id}`, name, category: "camera", output: "video", prompt_suffix: suffix, params: motion ? { camera_motion: motion } : {}, description,
  })),
  ...[
    ["slow-motion", "Slow motion", "slow motion, high frame rate look", "Stretches the moment."],
    ["timelapse", "Timelapse", "timelapse, clouds racing", "Hours in seconds."],
  ].map(([id, name, suffix, description]) => ({ id: `style/${id}`, name, category: "style", output: "video", prompt_suffix: suffix, params: {}, description })),
  ...[
    ["cinematic", "Cinematic", "cinematic still, anamorphic, shallow depth of field", "Film frame look."],
    ["film-35mm", "35mm film", "shot on 35mm film, natural grain, soft contrast", "Analog warmth."],
    ["editorial", "Editorial", "editorial photograph, clean composition, magazine style", "Magazine clean."],
    ["product", "Product studio", "studio product photo, seamless backdrop, soft box light", "Seamless backdrop."],
    ["watercolor", "Watercolor", "loose watercolor painting on textured paper", "Painted look."],
    ["isometric", "Isometric 3D", "isometric 3D render, soft shadows, miniature", "Tiny world."],
  ].map(([id, name, suffix, description]) => ({ id: `style/${id}`, name, category: "style", output: "image", prompt_suffix: suffix, params: {}, description })),
  ...[
    ["golden-hour", "Golden hour", "golden hour light, long warm shadows", "Low warm sun."],
    ["blue-hour", "Blue hour", "blue hour, cool ambient light, lit windows", "Cool twilight."],
  ].map(([id, name, suffix, description]) => ({ id: `lighting/${id}`, name, category: "lighting", output: "image", prompt_suffix: suffix, params: {}, description })),
];

export const imagePrompts = [
  "a timber cabin in fog at dawn, pine forest, soft light",
  "alpine lake at sunrise, mist over the water, granite peaks",
  "ceramic vase on a linen table, morning window light, still life",
  "neon alley in Tokyo after rain, reflections, night",
  "desert dunes at golden hour, long shadows, minimal",
  "a coastal cliff with a lighthouse, storm clearing, cinematic",
  "geometric poster, orange circle, clay and ink shapes, Swiss style",
  "a modular wooden house on a hill at dusk, warm windows, forest",
  "glass perfume bottle on travertine, soft studio light, product photo",
  "misty pine forest path in winter, quiet morning",
  "night skyline over the river, city lights, long exposure",
  "sunset over the sea, a sailboat, warm haze",
  "abstract composition of arches and circles, risograph print",
  "a quiet harbor at blue hour, fishing boats",
  "terracotta teapot, dark background, chiaroscuro",
  "rolling hills in Tuscany at sunrise, cypress trees, fog",
];

export const videoPrompts = [
  "slow dolly-in on a cabin in falling snow, warm windows",
  "drone shot over a misty fjord at sunrise",
  "waves rolling onto a black sand beach, slow motion",
  "neon city street at night, people with umbrellas, rain",
  "camera orbits a ceramic vase as the light shifts",
  "timelapse of clouds over the mountains, golden hour",
];

const cameraField = { key: "camera", label: "Camera move", type: "preset", category: "camera", default: "camera/dolly-in", required: false, multiline: false, options: [] };
const durationField = { key: "duration", label: "Length", type: "enum", options: ["5", "10"], default: "5", required: false, multiline: false };
const sceneField = { key: "scene", label: "Scene", type: "text", required: true, multiline: true, options: [], help: "Who or what is in frame, where, and what happens." };
const lensField = { key: "lens", label: "Lens", type: "enum", options: ["24mm wide", "35mm", "50mm", "85mm portrait", "anamorphic"], default: "35mm", required: false, multiline: false };
const lightField = { key: "light", label: "Light", type: "enum", options: ["golden hour", "blue hour", "soft overcast light", "neon night", "studio softbox"], default: "golden hour", required: false, multiline: false };
const stillModels = ["fal:fal-ai/nano-banana-pro", "fal:fal-ai/flux-2-pro", "openai:gpt-image-1.5", "comfyui:workflows/flux-dev", "google:imagen-4"];
const motionModels = ["fal:fal-ai/kling-video/v3/pro/image-to-video", "fal:fal-ai/veo3.1/fast", "fal:fal-ai/wan/v2.5", "comfyui:workflows/wan-2.2-i2v", "runway:gen4_turbo"];
const shotStep = { id: "shot", label: "Shoot the move", output: "video", models: motionModels, prompt: "{{scene}}", params: { duration: "{{duration}}" }, preset: "{{camera}}", inputs: [{ from: "step:still", role: "start_frame" }] };

/** Recipes as GET /api/recipes returns them, without `step_models` and `runnable` (computed). */
export const recipes = [
  {
    id: "cinema/shot", name: "Cinema shot", output: "video",
    description: "Direct a shot like on a film set: Pictesa composes the still with your lens and light, then animates it with your camera move.",
    fields: [sceneField, cameraField, lensField, lightField, durationField], inputs: [],
    steps: [{ id: "still", label: "Compose the still", output: "image", models: stillModels, prompt: "{{scene}}, cinematic film still, {{lens}} lens, {{light}}, shallow depth of field", params: { aspect_ratio: "16:9" }, inputs: [] }, shotStep],
  },
  {
    id: "cinema/character-shot", name: "Character shot", output: "video",
    description: "Put your own character or product into a directed shot. Your reference keeps the look consistent; the camera move brings it to life.",
    fields: [sceneField, cameraField, { ...lensField, default: "50mm" }, lightField, durationField], inputs: [{ key: "character", label: "Character or product photos", kind: "image", max: 3, required: true }],
    steps: [{ id: "still", label: "Place the character", output: "image", models: stillModels.slice(0, 3), prompt: "{{scene}}, {{lens}} lens, {{light}}", params: { aspect_ratio: "16:9" }, inputs: [{ from: "input:character", role: "reference" }] }, shotStep],
  },
  {
    id: "product/hero", name: "Product hero", output: "video",
    description: "A clean studio still of your product, then a slow orbit around it.",
    fields: [{ key: "setting", label: "Setting", type: "enum", options: ["seamless white", "warm stone", "dark velvet", "outdoor morning"], default: "warm stone", required: false, multiline: false }, cameraField, durationField],
    inputs: [{ key: "product", label: "Product photo", kind: "image", max: 2, required: true }],
    steps: [{ id: "still", label: "Studio still", output: "image", models: stillModels.slice(0, 3), prompt: "studio product photo on {{setting}}, soft box light", params: { aspect_ratio: "1:1" }, inputs: [{ from: "input:product", role: "reference" }] }, { ...shotStep, label: "Orbit", prompt: "slow orbit around the product" }],
  },
  {
    id: "photo/animate", name: "Animate a photo", output: "video",
    description: "Bring one photo to life with a camera move.",
    fields: [{ key: "action", label: "What happens", type: "text", required: false, multiline: true, options: [] }, cameraField, durationField],
    inputs: [{ key: "photo", label: "Photo", kind: "image", max: 1, required: true }],
    steps: [{ ...shotStep, label: "Animate", prompt: "{{action}}", inputs: [{ from: "input:photo", role: "start_frame" }] }],
  },
  {
    id: "photo/new-angle", name: "New angle", output: "image",
    description: "Re-shoot a photo from another camera position.",
    fields: [{ key: "angle", label: "Angle", type: "enum", options: ["from above", "low angle", "side view", "close-up", "wide shot"], default: "low angle", required: false, multiline: false }],
    inputs: [{ key: "photo", label: "Photo", kind: "image", max: 1, required: true }],
    steps: [{ id: "angle", label: "Re-shoot", output: "image", models: stillModels.slice(0, 3), prompt: "the same scene, {{angle}}", params: {}, inputs: [{ from: "input:photo", role: "reference" }] }],
  },
  {
    id: "film/storyboard", name: "Storyboard to film", output: "video",
    description: "Turn a storyboard frame into a finished shot with Runway or Veo.",
    fields: [sceneField, durationField], inputs: [{ key: "frame", label: "Storyboard frame", kind: "image", max: 1, required: true }],
    steps: [{ id: "film", label: "Film the frame", output: "video", models: ["runway:gen4_turbo", "google:veo-3.1"], prompt: "{{scene}}", params: { duration: "{{duration}}" }, inputs: [{ from: "input:frame", role: "start_frame" }] }],
  },
];
