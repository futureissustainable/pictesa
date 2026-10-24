// Dev-only fake of the Pictesa server (docs/API.md), enabled with VITE_MOCK=1 (local mode) or
// VITE_MOCK=hosted (accounts). It runs inside the Vite dev server as middleware, so the browser
// code is exactly the production code: real HTTP, real SSE, real uploads and Range requests.
// Try: a prompt containing "fail" fails at half way; Stable Diffusion.cpp is unreachable.
// VITE_MOCK=token asks for an access token first (any value works except "wrong").

import { createReadStream, statSync } from "node:fs";
import type { IncomingMessage, ServerResponse } from "node:http";
import { join } from "node:path";
import type { Plugin } from "vite";
import { artSvg, hashString } from "./art.ts";
import { imagePrompts, models as modelSeeds, presets, providers as providerSeeds, recipes as recipeSeeds, videoPrompts, type ModelSeed, type ProviderSeed, type Value } from "./data.ts";

type Kind = "image" | "video" | "audio";
type Json = Record<string, unknown>;

interface Asset {
  id: string;
  kind: Kind;
  mime: string;
  width: number | null;
  height: number | null;
  duration_ms: number | null;
  size: number;
  url: string;
  thumb_url: string | null;
  source: "generated" | "upload" | "import";
  name?: string | null;
  job_id: string | null;
  model: string | null;
  prompt: string | null;
  favorite: boolean;
  created_at: number;
  deleted_at: number | null;
}

type FileSource = { type: "art"; seed: number; prompt: string } | { type: "media"; name: string; mime: string } | { type: "bytes"; data: Buffer; mime: string };
type ThumbSource = { type: "art" } | { type: "media"; name: string } | { type: "same" } | null;

interface AssetRecord {
  asset: Asset;
  file: FileSource;
  thumb: ThumbSource;
}

interface Job {
  id: string;
  batch_id: string;
  model: string;
  provider: string;
  model_name: string;
  output: Kind;
  status: "queued" | "running" | "succeeded" | "failed" | "canceled";
  prompt: string;
  negative_prompt: string | null;
  params: Record<string, Value>;
  inputs: { asset_id: string; role: string }[];
  preset: string | null;
  count: number;
  progress: number | null;
  message: string | null;
  error: { kind: string; message: string } | null;
  cost_usd: number | null;
  attempts: number;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  outputs: Asset[];
}

interface Plan {
  startAt: number;
  duration: number;
  progress: boolean;
  fail: { kind: string; message: string } | null;
}

const B32 = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
function ulid(time = Date.now(), random: () => number = Math.random): string {
  let t = time;
  let head = "";
  for (let i = 0; i < 10; i++) {
    head = B32[t % 32] + head;
    t = Math.floor(t / 32);
  }
  let tail = "";
  for (let i = 0; i < 16; i++) tail += B32[Math.floor(random() * 32)];
  return head + tail;
}

function seeded(seed: number) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const clip = (name: string, width: number, height: number) => ({ name: `clip-${name}.mp4`, poster: `poster-${name}.jpg`, width, height });
const clips = {
  wide: [clip("wide", 1280, 720), clip("wide-2", 1280, 720), clip("wide-3", 1280, 720)],
  tall: [clip("tall", 720, 1280), clip("tall-2", 720, 1280)],
  square: [clip("square", 960, 960)],
};

/** A synthetic sample clip in the asked ratio, varied by seed. */
function clipFor(ratio: Value | undefined, seed: string) {
  const text = String(ratio ?? "16:9");
  const pool = text === "9:16" || text.includes("portrait") ? clips.tall : text === "1:1" || text.includes("square") ? clips.square : clips.wide;
  return pool[hashString(seed) % pool.length]!;
}

function dimsFor(params: Record<string, Value>): [number, number] {
  const width = Number(params.width);
  const height = Number(params.height);
  if (width > 0 && height > 0) return [width, height];
  const named: Record<string, [number, number]> = {
    square_hd: [1024, 1024], portrait_4_3: [768, 1024], portrait_16_9: [576, 1024], landscape_4_3: [1024, 768], landscape_16_9: [1024, 576],
    "1024x1024": [1024, 1024], "1536x1024": [1536, 1024], "1024x1536": [1024, 1536],
  };
  const raw = String(params.aspect_ratio ?? params.image_size ?? params.size ?? "1:1");
  if (named[raw]) return named[raw]!;
  const long = String(params.resolution) === "4K" ? 4096 : String(params.resolution) === "2K" ? 2048 : 1024;
  const match = /^(\d+):(\d+)$/.exec(raw);
  const ratio = match ? Number(match[1]) / Number(match[2]) : 1;
  const round = (n: number) => Math.round(n / 16) * 16;
  return ratio >= 1 ? [long, round(long / ratio)] : [round(long * ratio), long];
}

function imageSize(data: Buffer): [number, number] | null {
  try {
    if (data.length > 24 && data.readUInt32BE(0) === 0x89504e47) return [data.readUInt32BE(16), data.readUInt32BE(20)];
    if (data.toString("ascii", 0, 3) === "GIF") return [data.readUInt16LE(6), data.readUInt16LE(8)];
    if (data[0] === 0xff && data[1] === 0xd8) {
      let i = 2;
      while (i + 9 < data.length) {
        if (data[i] !== 0xff) {
          i++;
          continue;
        }
        const marker = data[i + 1]!;
        const length = data.readUInt16BE(i + 2);
        if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) return [data.readUInt16BE(i + 7), data.readUInt16BE(i + 5)];
        i += 2 + length;
      }
    }
    if (data.toString("ascii", 0, 4) === "RIFF" && data.toString("ascii", 8, 12) === "WEBP") {
      const chunk = data.toString("ascii", 12, 16);
      if (chunk === "VP8X") return [1 + data.readUIntLE(24, 3), 1 + data.readUIntLE(27, 3)];
      if (chunk === "VP8 ") return [data.readUInt16LE(26) & 0x3fff, data.readUInt16LE(28) & 0x3fff];
      if (chunk === "VP8L") {
        const bits = data.readUInt32LE(21);
        return [1 + (bits & 0x3fff), 1 + ((bits >> 14) & 0x3fff)];
      }
    }
  } catch {
    return null;
  }
  return null;
}

class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
    readonly extra: Json = {},
  ) {
    super(message);
  }
}

export type MockMode = "local" | "hosted" | "token";

export function mockApi(options: { mode: MockMode }): Plugin {
  return {
    name: "pictesa-mock-api",
    apply: "serve",
    configureServer(server) {
      const media = join(server.config.root, "mock/media");
      const state = createState(options.mode, media);
      server.middlewares.use((req, res, next) => {
        const url = new URL(req.url ?? "/", "http://mock.local");
        if (!/^\/(api|files|thumbs|mcp|__mock)(\/|$)/.test(url.pathname)) return next();
        state.handle(req, res, url).catch((error: unknown) => {
          if (res.headersSent) return res.end();
          if (error instanceof HttpError) return sendJson(res, error.status, { error: error.message, ...error.extra });
          console.error(error);
          sendJson(res, 500, { error: "The mock server failed. See the dev server log." });
        });
      });
      server.config.logger.info(`\n  Pictesa mock API on (${options.mode} mode). Data resets on restart.\n`);
    },
  };
}

function sendJson(res: ServerResponse, status: number, data: unknown) {
  const body = JSON.stringify(data);
  res.writeHead(status, { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store" });
  res.end(body);
}

async function readBody(req: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks);
}

async function readJson(req: IncomingMessage): Promise<Json> {
  const raw = await readBody(req);
  if (!raw.length) return {};
  try {
    return JSON.parse(raw.toString("utf8")) as Json;
  } catch {
    throw new HttpError(400, "The body is not valid JSON.", { kind: "validation" });
  }
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function createState(mode: MockMode, mediaDir: string) {
  const hosted = mode === "hosted";
  const random = seeded(20261024);
  const providers = providerSeeds.map((seed) => ({ ...seed, stored: { ...seed.stored } }));
  const assets = new Map<string, AssetRecord>();
  const jobs = new Map<string, Job>();
  const plans = new Map<string, Plan>();
  const clients = new Set<ServerResponse>();
  const users = new Map<string, { id: string; email: string; name: string | null; role: "admin" | "user"; created_at: number; password: string }>();
  const sessions = new Map<string, string>();
  const tokens: { id: string; name: string; created_at: number; last_used_at: number | null; token: string }[] = [];
  const svgCache = new Map<string, string>();

  const providerFor = (id: string) => providers.find((provider) => provider.id === id);

  function fieldState(provider: ProviderSeed, key: string): { set: boolean; source?: string; value?: string } {
    const field = provider.fields.find((item) => item.key === key)!;
    if (provider.stored[key]) return { set: true, source: "stored", ...(field.secret ? {} : { value: provider.stored[key] }) };
    if (field.env.some((name) => provider.env[name])) return { set: true, source: "env" };
    if (field.default) return { set: true, source: "default", value: field.default };
    return { set: false };
  }

  function configured(provider: ProviderSeed | undefined): boolean {
    if (!provider) return false;
    if (provider.kind === "local") return Boolean(provider.stored.base_url) || Boolean(provider.reachable);
    return provider.fields.filter((field) => !field.optional).every((field) => fieldState(provider, field.key).set);
  }

  function providerState(provider: ProviderSeed) {
    return {
      id: provider.id,
      name: provider.name,
      kind: provider.kind,
      fields: provider.fields,
      signup_url: provider.signup_url,
      docs_url: provider.docs_url,
      configured: configured(provider),
      values: Object.fromEntries(provider.fields.map((field) => [field.key, fieldState(provider, field.key)])),
      model_count: modelSeeds.filter((model) => model.provider === provider.id).length,
    };
  }

  // Hosted runs are billed from the wallet, so prices include the markup.
  function modelJson(model: ModelSeed, full = false) {
    const { discovered, ...spec } = model;
    const pricing = spec.pricing as { unit: string; usd?: number } | undefined;
    const billed = hosted && Boolean(pricing && pricing.unit !== "local");
    const priced = billed && typeof pricing?.usd === "number" ? { ...pricing, usd: Math.round(pricing.usd * wallet.markup * 10000) / 10000 } : pricing;
    const listedOnly = discovered && !full;
    return { ...spec, pricing: priced, ...(listedOnly ? { params: [], inputs: [] } : {}), complete: !listedOnly, billed, configured: configured(providerFor(model.provider)) };
  }

  function broadcast(event: string, data: unknown) {
    const frame = `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
    for (const client of clients) client.write(frame);
  }

  function addAsset(record: AssetRecord) {
    assets.set(record.asset.id, record);
    return record.asset;
  }

  function generatedAsset(job: Job, at: number, index: number): Asset {
    const id = ulid(at + index);
    const base = {
      id,
      job_id: job.id,
      model: job.model,
      prompt: job.prompt,
      favorite: false,
      created_at: at + index,
      deleted_at: null,
      source: "generated" as const,
      url: `/files/${id}`,
      thumb_url: `/thumbs/${id}`,
    };
    if (job.output === "video") {
      const clip = clipFor(job.params.aspect_ratio ?? (job.inputs.length ? "16:9" : undefined), id);
      const seconds = Number(job.params.duration) || Math.round(Number(job.params.length ?? 80) / 16) || 5;
      return addAsset({
        asset: { ...base, kind: "video", mime: "video/mp4", width: clip.width, height: clip.height, duration_ms: seconds * 1000, size: Math.round(seconds * 900_000 * (0.8 + Math.random() * 0.4)) },
        file: { type: "media", name: clip.name, mime: "video/mp4" },
        thumb: { type: "media", name: clip.poster },
      });
    }
    if (job.output === "audio") {
      return addAsset({
        asset: { ...base, thumb_url: null, kind: "audio", mime: "audio/mp4", width: null, height: null, duration_ms: (Number(job.params.duration) || 8) * 1000, size: 480_000 },
        file: { type: "media", name: "pad.m4a", mime: "audio/mp4" },
        thumb: null,
      });
    }
    const [width, height] = dimsFor(job.params);
    return addAsset({
      asset: { ...base, kind: "image", mime: "image/svg+xml", width, height, duration_ms: null, size: Math.round(width * height * (0.6 + Math.random() * 0.8)) },
      file: { type: "art", seed: hashString(id), prompt: job.prompt },
      thumb: { type: "art" },
    });
  }

  function priceOf(model: ModelSeed, params: Record<string, Value>, count: number): number | null {
    const pricing = model.pricing as { unit: string; usd?: number } | undefined;
    if (!pricing) return null;
    if (pricing.unit === "local") return 0;
    if (pricing.unit === "variable") return Math.round((0.02 + Math.random() * 0.1) * count * 1000) / 1000;
    const usd = pricing.usd ?? 0;
    if (pricing.unit === "per_second") return usd * (Number(params.duration) || 5) * count;
    if (pricing.unit === "per_megapixel") {
      const [w, h] = dimsFor(params);
      return usd * ((w * h) / 1e6) * count;
    }
    return usd * count;
  }

  function defaults(model: ModelSeed): Record<string, Value> {
    const values: Record<string, Value> = {};
    for (const param of model.params) if (param.default !== undefined) values[param.key] = param.default as Value;
    return values;
  }

  // Seed a library that looks lived in: about a month of work, newest last.
  function seed() {
    const now = Date.now();
    const imageModels = modelSeeds.filter((model) => model.output === "image" && configured(providerFor(model.provider)) && model.prompt);
    const videoModels = modelSeeds.filter((model) => model.output === "video" && configured(providerFor(model.provider)) && model.prompt);
    const images: Asset[] = [];
    let t = now - 27 * 86_400_000;
    for (let i = 0; i < 70; i++) {
      t += (6 + random() * 9) * 3_600_000 * 0.9;
      if (t > now - 40 * 60_000) t = now - (70 - i) * 60_000;
      const kind = i % 6 === 5 ? "video" : "image";
      const pool = kind === "video" ? videoModels : imageModels;
      const model = pool[Math.floor(random() * pool.length)]!;
      const params = defaults(model);
      const ratio = model.params.find((param) => param.key === "aspect_ratio") as { options?: { value: Value }[] } | undefined;
      if (ratio?.options) {
        const options = ratio.options.map((option) => option.value).filter((value) => value !== "auto");
        params.aspect_ratio = options[Math.floor(random() * Math.min(options.length, 6))] ?? "1:1";
      }
      if (!("seed" in params) && model.params.some((param) => param.key === "seed")) params.seed = Math.floor(random() * 4_000_000_000);
      const prompt = kind === "video" ? videoPrompts[Math.floor(random() * videoPrompts.length)]! : imagePrompts[Math.floor(random() * imagePrompts.length)]!;
      const inputs: Job["inputs"] = [];
      const startSlot = model.inputs.find((slotItem) => slotItem.role === "start_frame");
      if (kind === "video" && startSlot && images.length) inputs.push({ asset_id: images[Math.floor(random() * images.length)]!.id, role: "start_frame" });
      if (kind === "video" && startSlot?.min && !inputs.length) continue;
      const count = kind === "image" && random() > 0.72 ? 2 + Math.floor(random() * 3) : 1;
      const jobId = ulid(t - 30_000, random);
      const job: Job = {
        id: jobId, batch_id: ulid(t - 30_000, random), model: model.id, provider: model.provider, model_name: model.name, output: kind,
        status: "succeeded", prompt, negative_prompt: null, params, inputs, preset: kind === "video" && random() > 0.6 ? "camera/dolly-in" : null, count,
        progress: 1, message: null, error: null, cost_usd: null, attempts: 1, created_at: t - 30_000, started_at: t - 25_000, finished_at: t, outputs: [],
      };
      job.cost_usd = priceOf(model, params, count);
      for (let n = 0; n < count; n++) {
        const asset = generatedAsset(job, t, n);
        asset.favorite = random() > 0.86;
        job.outputs.push(asset);
        if (asset.kind === "image") images.push(asset);
      }
      jobs.set(job.id, job);
    }
    // Uploads, an import, audio and a few trashed items.
    const uploadPrompts = ["timber cabin reference, forest", "ceramic vase reference", "coast reference with lighthouse", "dunes reference"];
    uploadPrompts.forEach((hidden, index) => {
      const id = ulid(now - (20 - index * 4) * 86_400_000, random);
      const [width, height] = index % 2 ? [1080, 1350] : [1600, 1067];
      addAsset({
        asset: { id, kind: "image", mime: "image/svg+xml", width, height, duration_ms: null, size: 1_400_000 + index * 90_000, url: `/files/${id}`, thumb_url: `/thumbs/${id}`, source: "upload", job_id: null, model: null, prompt: null, favorite: index === 0, created_at: now - (20 - index * 4) * 86_400_000, deleted_at: null },
        file: { type: "art", seed: hashString(id), prompt: hidden },
        thumb: { type: "art" },
      });
    });
    const videoUpload = ulid(now - 9 * 86_400_000, random);
    addAsset({
      asset: { id: videoUpload, kind: "video", mime: "video/mp4", width: 640, height: 360, duration_ms: 5000, size: 23_713, url: `/files/${videoUpload}`, thumb_url: null, source: "upload", job_id: null, model: null, prompt: null, favorite: false, created_at: now - 9 * 86_400_000, deleted_at: null },
      file: { type: "media", name: "clip-wide.mp4", mime: "video/mp4" },
      thumb: null,
    });
    const audioModel = modelSeeds.find((model) => model.output === "audio")!;
    for (let i = 0; i < 2; i++) {
      const at = now - (6 - i * 3) * 86_400_000;
      const job: Job = {
        id: ulid(at - 20_000, random), batch_id: ulid(at - 20_000, random), model: audioModel.id, provider: audioModel.provider, model_name: audioModel.name, output: "audio", status: "succeeded",
        prompt: i ? "rain on a cabin roof, distant thunder, calm" : "warm ambient pad, slow, analog synth", negative_prompt: null, params: { duration: 8, steps: 8 }, inputs: [], preset: null, count: 1,
        progress: 1, message: null, error: null, cost_usd: 0.2, attempts: 1, created_at: at - 20_000, started_at: at - 18_000, finished_at: at, outputs: [],
      };
      job.outputs.push(generatedAsset(job, at, 0));
      jobs.set(job.id, job);
    }
    const all = [...assets.values()].map((record) => record.asset).filter((asset) => asset.source === "generated");
    for (let i = 0; i < 4; i++) all[Math.floor(random() * all.length)]!.deleted_at = now - (i + 1) * 86_400_000;
  }
  seed();

  function listAssets(query: URLSearchParams) {
    const words = (query.get("q") ?? "").toLowerCase().split(/\s+/).filter(Boolean);
    const trash = query.get("trash") === "true";
    const limit = Math.min(200, Number(query.get("limit")) || 60);
    const before = query.get("before");
    const matches = [...assets.values()]
      .map((record) => record.asset)
      .filter(
        (asset) =>
          (!query.get("kind") || asset.kind === query.get("kind")) &&
          (query.get("favorite") !== "true" || asset.favorite) &&
          (!query.get("source") || asset.source === query.get("source")) &&
          (!query.get("model") || asset.model === query.get("model")) &&
          trash === Boolean(asset.deleted_at) &&
          words.every((word) => (asset.prompt ?? "").toLowerCase().includes(word)),
      )
      .sort((a, b) => (a.id < b.id ? 1 : -1))
      .filter((asset) => !before || asset.id < before);
    const page = matches.slice(0, limit);
    return { assets: page, next: matches.length > limit ? page[page.length - 1]!.id : null };
  }

  function validateInputs(model: ModelSeed, inputs: { asset_id: string; role: string }[], strict: boolean) {
    const kept: { asset_id: string; role: string }[] = [];
    for (const input of inputs) {
      const record = assets.get(input.asset_id);
      if (!record || record.asset.deleted_at) throw new HttpError(400, "An input is no longer in the library.", { kind: "validation", field: `inputs.${input.role}` });
      const slotSpec = model.inputs.find((item) => item.role === input.role && item.kind === record.asset.kind);
      if (slotSpec) kept.push(input);
      else if (strict) throw new HttpError(400, `${model.name} does not take that ${input.role.replace("_", " ")}.`, { kind: "validation", field: `inputs.${input.role}` });
    }
    for (const slotSpec of model.inputs) {
      const count = kept.filter((input) => input.role === slotSpec.role).length;
      if (count < slotSpec.min) throw new HttpError(400, `${strict ? "" : `${model.name}: `}add ${slotSpec.label ?? slotSpec.role.replace("_", " ")}.`.replace(/^a/, "A"), { kind: "validation", field: `inputs.${slotSpec.role}` });
      if (count > slotSpec.max && strict) throw new HttpError(400, `${slotSpec.label ?? slotSpec.role} takes at most ${slotSpec.max}.`, { kind: "validation", field: `inputs.${slotSpec.role}` });
    }
    return kept;
  }

  /** Params for one model. Compare models get them adapted: unknown keys dropped, bad values replaced. */
  function paramsFor(model: ModelSeed, userParams: Record<string, Value>, strict: boolean, presetParams: Record<string, Value>) {
    const fitted: Record<string, Value> = {};
    for (const [key, value] of Object.entries(userParams)) {
      const spec = model.params.find((param) => param.key === key) as (Json & { type: string; label: string; options?: { value: Value }[]; min?: number; max?: number }) | undefined;
      if (!spec) continue;
      const bad =
        (spec.type === "enum" && !spec.options?.some((option) => String(option.value) === String(value))) ||
        ((spec.type === "int" || spec.type === "float") && (typeof value !== "number" || value < (spec.min ?? -Infinity) || value > (spec.max ?? Infinity))) ||
        (spec.type === "bool" && typeof value !== "boolean");
      if (bad && strict) throw new HttpError(400, `${spec.label} has a value this model does not accept.`, { kind: "validation", field: `params.${key}` });
      if (!bad) fitted[key] = value;
    }
    const fromPreset = Object.fromEntries(Object.entries(presetParams).filter(([key]) => model.params.some((param) => param.key === key)));
    return { ...fromPreset, ...fitted };
  }

  function createJobs(body: Json): { batch_id: string; jobs: Job[] } {
    const model = modelSeeds.find((item) => item.id === body.model);
    if (!model) throw new HttpError(400, "Unknown model.", { kind: "validation", field: "model" });
    const compareIds = Array.isArray(body.compare) ? (body.compare as string[]) : [];
    if (compareIds.length > 7) throw new HttpError(400, "Compare up to 7 more models.", { kind: "validation", field: "compare" });
    const lineup = [model, ...compareIds.map((id) => modelSeeds.find((item) => item.id === id))];
    for (const item of lineup) {
      if (!item) throw new HttpError(400, "Unknown model to compare.", { kind: "validation", field: "compare" });
      const provider = providerFor(item.provider)!;
      if (!configured(provider)) throw new HttpError(400, `${provider.name} is not set up. Add it in Settings, Providers.`, { kind: "auth", field: item === model ? "model" : "compare" });
    }
    const prompt = typeof body.prompt === "string" ? body.prompt.trim() : "";
    const inputs = Array.isArray(body.inputs) ? [...(body.inputs as { asset_id: string; role: string }[])] : [];
    if (typeof body.character === "string" && body.character) {
      const character = characters.get(body.character);
      if (!character) throw new HttpError(400, "That character no longer exists.", { kind: "validation", field: "character" });
      const slotSpec = model.inputs.find((item) => item.role === "reference" && item.kind === "image");
      if (!slotSpec) throw new HttpError(400, `${model.name} does not take reference images, so it cannot use a character.`, { kind: "validation", field: "character" });
      const room = slotSpec.max - inputs.filter((input) => input.role === "reference").length;
      for (const id of character.asset_ids.slice(0, Math.max(0, room))) inputs.push({ asset_id: id, role: "reference" });
    }
    if (model.prompt && !prompt && !inputs.length) throw new HttpError(400, "Add a prompt.", { kind: "validation", field: "prompt" });
    const count = Number(body.count ?? 1);
    if (!Number.isInteger(count) || count < 1 || count > 8) throw new HttpError(400, "Count must be from 1 to 8.", { kind: "validation", field: "count" });
    const preset = typeof body.preset === "string" ? presets.find((item) => item.id === body.preset) : undefined;
    if (body.preset && (!preset || preset.output !== model.output)) throw new HttpError(400, "Unknown preset for this output.", { kind: "validation", field: "preset" });
    const userParams = (body.params && typeof body.params === "object" ? body.params : {}) as Record<string, Value>;
    const plansFor = lineup.map((item, index) => {
      const strict = index === 0;
      const runInputs = validateInputs(item!, inputs, strict);
      const params = { ...defaults(item!), ...paramsFor(item!, userParams, strict, (preset?.params ?? {}) as Record<string, Value>) };
      return { model: item!, inputs: runInputs, params };
    });
    // Hosted wallet: refuse runs the balance cannot cover.
    if (wallet.enabled) {
      const estimate = plansFor.reduce((sum, plan) => sum + (priceOf(plan.model, plan.params, count) ?? 0) * wallet.markup, 0);
      if (wallet.balance < estimate) throw new HttpError(400, `Your balance (${wallet.balance.toFixed(2)} USD) does not cover this run (about ${estimate.toFixed(2)} USD).`, { kind: "validation", field: "billing" });
    }
    const batch = ulid();
    const created: Job[] = [];
    let position = [...jobs.values()].filter((job) => job.status === "queued").length;
    for (const plan of plansFor) {
      const provider = providerFor(plan.model.provider)!;
      let remaining = count;
      while (remaining > 0) {
        const size = Math.min(plan.model.max_outputs, remaining);
        remaining -= size;
        const now = Date.now();
        const job: Job = {
          id: ulid(now + created.length), batch_id: batch, model: plan.model.id, provider: plan.model.provider, model_name: plan.model.name, output: plan.model.output,
          status: "queued", prompt, negative_prompt: typeof body.negative_prompt === "string" ? body.negative_prompt : null, params: plan.params, inputs: plan.inputs, preset: preset?.id ?? null, count: size,
          progress: null, message: `In queue (position ${++position})`, error: null, cost_usd: priceOf(plan.model, plan.params, size), attempts: 0, created_at: now, started_at: null, finished_at: null, outputs: [],
        };
        const unreachable = provider.kind === "local" && !provider.reachable;
        plans.set(job.id, {
          startAt: now + 700 + created.length * 700 + Math.random() * 600,
          duration: plan.model.output === "video" ? 8000 + Math.random() * 5000 : plan.model.output === "audio" ? 4500 : 2600 + Math.random() * 2600,
          progress: provider.progress,
          fail: unreachable
            ? { kind: "transient", message: `Nothing answered at ${fieldState(provider, "base_url").value ?? provider.stored.base_url}.` }
            : /fail/i.test(prompt)
              ? { kind: "content_policy", message: "The provider's safety filter blocked this prompt." }
              : null,
        });
        jobs.set(job.id, job);
        created.push(job);
      }
    }
    created.forEach((job) => broadcast("job", job));
    return { batch_id: batch, jobs: created };
  }

  const characters = new Map<string, { id: string; name: string; description: string; asset_ids: string[]; created_at: number; updated_at: number }>();
  const characterJson = (character: { id: string; name: string; description: string; asset_ids: string[]; created_at: number; updated_at: number }) => ({
    id: character.id,
    name: character.name,
    description: character.description,
    assets: character.asset_ids.map((id) => assets.get(id)?.asset).filter(Boolean),
    created_at: character.created_at,
    updated_at: character.updated_at,
  });
  function applyCharacter(character: { name: string; description: string; asset_ids: string[] }, body: Json, creating: boolean) {
    if (creating || body.name !== undefined) {
      const name = String(body.name ?? "").trim();
      if (!name || name.length > 80) throw new HttpError(400, "Give the character a short name.", { kind: "validation", field: "name" });
      character.name = name;
    }
    if (body.description !== undefined) character.description = String(body.description ?? "").slice(0, 500);
    if (creating || body.asset_ids !== undefined) {
      const ids = Array.isArray(body.asset_ids) ? (body.asset_ids as string[]) : [];
      if (!ids.length || ids.length > 14) throw new HttpError(400, "A character needs 1 to 14 images.", { kind: "validation", field: "asset_ids" });
      if (ids.some((id) => assets.get(id)?.asset.kind !== "image" || assets.get(id)?.asset.deleted_at)) throw new HttpError(400, "Characters use images from the library.", { kind: "validation", field: "asset_ids" });
      character.asset_ids = ids;
    }
  }

  // Recipes: each step is an ordinary job; a run starts the next step when one finishes.
  interface Run {
    id: string;
    recipe_id: string;
    recipe_name: string;
    status: "running" | "succeeded" | "failed" | "canceled";
    step: number;
    steps: number;
    jobs: string[];
    error: string | null;
    created_at: number;
    updated_at: number;
  }
  const recipeRuns = new Map<string, Run>();
  const runOfJob = new Map<string, { run: Run; values: Record<string, string>; inputs: Record<string, string[]>; models: (string | null)[] }>();

  const fill = (text: string, values: Record<string, string>) => text.replace(/\{\{\s*([\w-]+)\s*\}\}/g, (_, key: string) => values[key] ?? "");

  function stepModels(recipe: (typeof recipeSeeds)[number], chosen: Record<string, string> = {}) {
    return recipe.steps.map((step) => {
      const pick = chosen[step.id];
      if (pick && step.models.includes(pick) && configured(providerFor(modelSeeds.find((item) => item.id === pick)?.provider ?? ""))) return pick;
      return step.models.find((id) => configured(providerFor(modelSeeds.find((item) => item.id === id)?.provider ?? ""))) ?? null;
    });
  }

  function recipeView(recipe: (typeof recipeSeeds)[number]) {
    const chosen = stepModels(recipe);
    return { ...recipe, step_models: chosen, runnable: chosen.every(Boolean) };
  }

  function startStep(run: Run, index: number, context: { values: Record<string, string>; inputs: Record<string, string[]>; models: (string | null)[]; previous: Asset[] }) {
    const recipe = recipeSeeds.find((item) => item.id === run.recipe_id)!;
    const step = recipe.steps[index]!;
    const stepInputs = (step.inputs ?? []).flatMap((link) =>
      link.from.startsWith("input:") ? (context.inputs[link.from.slice(6)] ?? []).map((id) => ({ asset_id: id, role: link.role })) : context.previous.map((asset) => ({ asset_id: asset.id, role: link.role })),
    );
    const params = Object.fromEntries(Object.entries(step.params ?? {}).map(([key, value]) => [key, typeof value === "string" ? (/^\d+$/.test(fill(value, context.values)) ? Number(fill(value, context.values)) : fill(value, context.values)) : value]));
    const presetId = "preset" in step && step.preset ? fill(step.preset, context.values) : "";
    const { jobs: created } = createJobs({ model: context.models[index], prompt: fill(step.prompt ?? "", context.values).replace(/^[,\s]+/, ""), params, inputs: stepInputs, count: 1, ...(presetId ? { preset: presetId } : {}) });
    const job = created[0]!;
    run.step = index;
    run.jobs.push(job.id);
    run.updated_at = Date.now();
    runOfJob.set(job.id, { run, values: context.values, inputs: context.inputs, models: context.models });
    broadcast("recipe", run);
  }

  function advanceRun(job: Job) {
    const link = runOfJob.get(job.id);
    if (!link || link.run.status !== "running") return;
    const { run } = link;
    const recipe = recipeSeeds.find((item) => item.id === run.recipe_id)!;
    if (job.status === "succeeded") {
      if (run.step + 1 < run.steps) {
        try {
          startStep(run, run.step + 1, { ...link, previous: job.outputs });
        } catch (error) {
          run.status = "failed";
          run.error = `${recipe.steps[run.step + 1]!.label}: ${(error as Error).message}`;
          run.updated_at = Date.now();
          broadcast("recipe", run);
        }
        return;
      }
      run.status = "succeeded";
    } else {
      run.status = job.status === "canceled" ? "canceled" : "failed";
      run.error = job.status === "failed" ? `${recipe.steps[run.step]!.label}: ${job.error?.message ?? "failed"}` : null;
    }
    run.updated_at = Date.now();
    broadcast("recipe", run);
  }

  // Hosted wallet (mock): a balance, a ledger and a fake checkout.
  const wallet = { enabled: hosted, balance: 12.4, debt: 0, markup: 1.25, ledger: [] as { id: string; amount_usd: number; reason: string; job_id: string | null; created_at: number }[] };
  if (hosted) {
    const now = Date.now();
    wallet.ledger.push(
      { id: ulid(now - 5 * 86_400_000), amount_usd: 25, reason: "Top-up", job_id: null, created_at: now - 5 * 86_400_000 },
      { id: ulid(now - 4 * 86_400_000), amount_usd: -0.75, reason: "Kling 3 Pro", job_id: null, created_at: now - 4 * 86_400_000 },
      { id: ulid(now - 2 * 86_400_000), amount_usd: -11.85, reason: "Veo 3.1 Fast", job_id: null, created_at: now - 2 * 86_400_000 },
    );
    wallet.ledger.reverse();
  }

  function charge(job: Job) {
    if (!wallet.enabled || !job.cost_usd) return;
    const amount = Math.round(job.cost_usd * wallet.markup * 100) / 100;
    wallet.balance = Math.round((wallet.balance - amount) * 100) / 100;
    wallet.ledger.unshift({ id: ulid(), amount_usd: -amount, reason: job.model_name, job_id: job.id, created_at: Date.now() });
  }

  // Job lifecycle: queued, running (with or without reported progress), then done or failed.
  setInterval(() => {
    const now = Date.now();
    for (const job of jobs.values()) {
      if (job.status !== "queued" && job.status !== "running") continue;
      const plan = plans.get(job.id);
      if (!plan) continue;
      if (job.status === "queued") {
        if (now < plan.startAt) continue;
        job.status = "running";
        job.attempts = 1;
        job.started_at = now;
        job.message = plan.progress ? null : "Generating";
        job.progress = plan.progress ? 0 : null;
        broadcast("job", job);
        continue;
      }
      const elapsed = now - (job.started_at ?? now);
      const fraction = elapsed / plan.duration;
      if (plan.fail && fraction > 0.45) {
        job.status = "failed";
        job.error = plan.fail;
        job.finished_at = now;
        job.message = null;
        broadcast("job", job);
        advanceRun(job);
        continue;
      }
      if (fraction >= 1) {
        const model = modelSeeds.find((item) => item.id === job.model)!;
        job.outputs = Array.from({ length: job.count }, (_, index) => generatedAsset(job, now, index));
        job.status = "succeeded";
        job.progress = 1;
        job.message = null;
        job.finished_at = now;
        job.cost_usd = priceOf(model, job.params, job.count);
        job.outputs.forEach((asset) => broadcast("asset", asset));
        broadcast("job", job);
        charge(job);
        advanceRun(job);
        continue;
      }
      if (plan.progress) job.progress = Math.min(0.99, Math.round(fraction * 100) / 100);
      else job.message = fraction < 0.3 ? "Generating" : fraction < 0.75 ? "Rendering frames" : "Finishing";
      broadcast("job", job);
    }
  }, 400);
  setInterval(() => broadcast("ping", {}), 20_000);

  function sessionUser(req: IncomingMessage) {
    const cookie = req.headers.cookie ?? "";
    const sid = /pictesa_mock=([^;]+)/.exec(cookie)?.[1];
    const id = sid ? sessions.get(sid) : undefined;
    return id ? users.get(id) : undefined;
  }

  function publicUser(user: { id: string; email: string; name: string | null; role: "admin" | "user"; created_at: number }) {
    return { id: user.id, email: user.email, name: user.name, role: user.role, created_at: user.created_at };
  }

  function startSession(res: ServerResponse, userId: string) {
    const sid = ulid();
    sessions.set(sid, userId);
    res.setHeader("Set-Cookie", `pictesa_mock=${sid}; Path=/; HttpOnly; SameSite=Lax`);
  }

  function serveFile(req: IncomingMessage, res: ServerResponse, url: URL, data: { path?: string; buffer?: Buffer; mime: string; name: string }) {
    const size = data.path ? statSync(data.path).size : data.buffer!.length;
    const headers: Record<string, string | number> = { "Content-Type": data.mime, "Accept-Ranges": "bytes", "Cache-Control": "private, max-age=3600" };
    if (url.searchParams.get("download") === "1") headers["Content-Disposition"] = `attachment; filename="${data.name}"`;
    const range = /bytes=(\d*)-(\d*)/.exec(req.headers.range ?? "");
    if (range) {
      const start = range[1] ? Number(range[1]) : Math.max(0, size - Number(range[2]));
      const end = range[1] && range[2] ? Math.min(Number(range[2]), size - 1) : size - 1;
      if (start >= size) {
        res.writeHead(416, { "Content-Range": `bytes */${size}` });
        return res.end();
      }
      res.writeHead(206, { ...headers, "Content-Range": `bytes ${start}-${end}/${size}`, "Content-Length": end - start + 1 });
      if (data.path) createReadStream(data.path, { start, end }).pipe(res);
      else res.end(data.buffer!.subarray(start, end + 1));
      return;
    }
    res.writeHead(200, { ...headers, "Content-Length": size });
    if (data.path) createReadStream(data.path).pipe(res);
    else res.end(data.buffer);
  }

  function art(record: AssetRecord, thumb: boolean): string {
    const key = `${record.asset.id}:${thumb}`;
    let svg = svgCache.get(key);
    if (!svg && record.file.type === "art") {
      const { width, height } = record.asset;
      const scale = thumb ? Math.min(1, 512 / Math.max(width ?? 1024, height ?? 1024)) : 1;
      svg = artSvg(record.file.seed, Math.round((width ?? 1024) * scale), Math.round((height ?? 1024) * scale), record.file.prompt);
      svgCache.set(key, svg);
    }
    return svg ?? "";
  }

  const extension = (mime: string) => ({ "image/svg+xml": "svg", "video/mp4": "mp4", "audio/mp4": "m4a", "image/png": "png", "image/jpeg": "jpg", "image/webp": "webp" })[mime] ?? mime.split("/")[1] ?? "bin";

  async function handle(req: IncomingMessage, res: ServerResponse, url: URL) {
    const method = req.method ?? "GET";
    const path = url.pathname;
    const user = sessionUser(req);
    const isAuth = path.startsWith("/api/auth/") || path === "/api/health";
    if (hosted && !user && !isAuth) throw new HttpError(401, "Log in first.", { kind: "auth" });
    const unlocked = mode !== "token" || /pictesa_token=ok/.test(req.headers.cookie ?? "");
    if (!unlocked && !isAuth && path !== "/__mock/checkout") throw new HttpError(401, "This server needs its access token.", { kind: "auth" });
    if (method === "GET" && path.startsWith("/api/") && path !== "/api/events") await sleep(60 + Math.random() * 140);

    let match: RegExpExecArray | null;

    if (path === "/mcp") throw new HttpError(501, "The mock server has no MCP endpoint. Run the real server for agents.");

    if ((match = /^\/(files|thumbs)\/([^/]+)$/.exec(path)) && method === "GET") {
      const record = assets.get(decodeURIComponent(match[2]!));
      if (!record) throw new HttpError(404, "No such file.", { kind: "not_found" });
      const thumb = match[1] === "thumbs";
      const name = `pictesa-${record.asset.id}.${extension(record.asset.mime)}`;
      if (thumb) {
        if (!record.thumb) throw new HttpError(404, "No thumbnail yet.", { kind: "not_found" });
        if (record.thumb.type === "media") return serveFile(req, res, url, { path: join(mediaDir, record.thumb.name), mime: "image/jpeg", name });
        if (record.thumb.type === "same" && record.file.type === "bytes") return serveFile(req, res, url, { buffer: record.file.data, mime: record.file.mime, name });
      }
      if (record.file.type === "art") return serveFile(req, res, url, { buffer: Buffer.from(art(record, thumb)), mime: "image/svg+xml", name });
      if (record.file.type === "media") return serveFile(req, res, url, { path: join(mediaDir, record.file.name), mime: record.file.mime, name });
      return serveFile(req, res, url, { buffer: record.file.data, mime: record.file.mime, name });
    }

    if (path === "/api/events" && method === "GET") {
      res.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache", Connection: "keep-alive", "X-Accel-Buffering": "no" });
      res.write(": connected\n\n");
      clients.add(res);
      req.on("close", () => clients.delete(res));
      return;
    }

    // Auth
    if (path === "/api/auth/me" && method === "GET") return sendJson(res, 200, { mode: hosted ? "hosted" : "local", needs_token: !unlocked, user: hosted && user ? publicUser(user) : null });
    if (path === "/api/auth/token" && method === "POST") {
      const body = await readJson(req);
      const token = String(body.token ?? "").trim();
      if (!token || token === "wrong") throw new HttpError(401, "That token does not match this server.", { kind: "auth", field: "token" });
      res.setHeader("Set-Cookie", "pictesa_token=ok; Path=/; HttpOnly; SameSite=Strict");
      res.writeHead(204);
      return res.end();
    }
    if ((path === "/api/auth/login" || path === "/api/auth/signup") && method === "POST") {
      if (!hosted) throw new HttpError(400, "Accounts exist only in hosted mode.", { kind: "validation" });
      const body = await readJson(req);
      const email = String(body.email ?? "").trim().toLowerCase();
      const password = String(body.password ?? "");
      if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) throw new HttpError(400, "Enter a valid email address.", { kind: "validation", field: "email" });
      if (path.endsWith("signup")) {
        if (password.length < 8) throw new HttpError(400, "Use at least 8 characters for the password.", { kind: "validation", field: "password" });
        if ([...users.values()].some((item) => item.email === email)) throw new HttpError(409, "That email already has an account. Log in instead.", { kind: "conflict", field: "email" });
        const created = { id: ulid(), email, name: typeof body.name === "string" && body.name.trim() ? body.name.trim() : null, role: users.size ? ("user" as const) : ("admin" as const), created_at: Date.now(), password };
        users.set(created.id, created);
        startSession(res, created.id);
        return sendJson(res, 201, publicUser(created));
      }
      let existing = [...users.values()].find((item) => item.email === email);
      // Any address works in the mock; "wrongpassword" shows the failure path.
      if (password === "wrongpassword" || password.length < 8) throw new HttpError(401, "That email and password do not match.", { kind: "auth" });
      if (!existing) {
        existing = { id: ulid(), email, name: null, role: users.size ? "user" : "admin", created_at: Date.now() - 40 * 86_400_000, password };
        users.set(existing.id, existing);
      }
      startSession(res, existing.id);
      return sendJson(res, 200, publicUser(existing));
    }
    if (path === "/api/auth/logout" && method === "POST") {
      res.setHeader("Set-Cookie", "pictesa_mock=; Path=/; Max-Age=0");
      res.writeHead(204);
      return res.end();
    }
    if (path === "/api/tokens" && method === "GET") return sendJson(res, 200, { tokens: tokens.map(({ token: _token, ...rest }) => rest) });
    if (path === "/api/tokens" && method === "POST") {
      const body = await readJson(req);
      const name = String(body.name ?? "").trim();
      if (!name) throw new HttpError(400, "Give the token a name.", { kind: "validation", field: "name" });
      const token = { id: ulid(), name, created_at: Date.now(), last_used_at: null, token: `pct_${Array.from({ length: 32 }, () => B32[Math.floor(Math.random() * 32)]).join("").toLowerCase()}` };
      tokens.unshift(token);
      return sendJson(res, 201, { id: token.id, name: token.name, token: token.token });
    }
    if ((match = /^\/api\/tokens\/([^/]+)$/.exec(path)) && method === "DELETE") {
      const index = tokens.findIndex((token) => token.id === match![1]);
      if (index >= 0) tokens.splice(index, 1);
      res.writeHead(204);
      return res.end();
    }

    if (path === "/api/health") return sendJson(res, 200, { ok: true, version: "0.1.0", mode: hosted ? "hosted" : "local", ffmpeg: true, data_dir: "~/.local/share/pictesa" });

    // Providers
    if (path === "/api/providers" && method === "GET") return sendJson(res, 200, { providers: providers.map(providerState) });
    if ((match = /^\/api\/providers\/([^/]+)(\/check)?$/.exec(path))) {
      const provider = providerFor(decodeURIComponent(match[1]!));
      if (!provider) throw new HttpError(404, "Unknown provider.", { kind: "not_found" });
      if (match[2] && method === "POST") {
        await sleep(500 + Math.random() * 500);
        if (provider.kind === "local") {
          const target = fieldState(provider, "base_url").value ?? provider.stored.base_url;
          return sendJson(res, 200, provider.reachable ? { ok: true } : { ok: false, error: `Nothing answered at ${target}. Start the server or change the URL.` });
        }
        if (!configured(provider)) return sendJson(res, 200, { ok: false, error: "Add an API key first." });
        if (/bad|wrong/.test(provider.stored.api_key ?? "")) return sendJson(res, 200, { ok: false, error: `${provider.name} rejected the key (HTTP 401).` });
        return sendJson(res, 200, { ok: true });
      }
      if (method === "PUT") {
        const body = await readJson(req);
        const values = (body.values ?? {}) as Record<string, string>;
        for (const [key, value] of Object.entries(values)) {
          if (!provider.fields.some((field) => field.key === key)) throw new HttpError(400, `Unknown field ${key}.`, { kind: "validation", field: `values.${key}` });
          if (value === "") delete provider.stored[key];
          else provider.stored[key] = value;
        }
        if (provider.kind === "local" && values.base_url) provider.reachable = !/7860/.test(values.base_url);
        return sendJson(res, 200, providerState(provider));
      }
      if (method === "DELETE") {
        provider.stored = {};
        res.writeHead(204);
        return res.end();
      }
    }

    // Models and presets
    if (path === "/api/models" && method === "GET") {
      const q = (url.searchParams.get("q") ?? "").toLowerCase();
      const list = modelSeeds
        .filter(
          (model) =>
            (!url.searchParams.get("output") || model.output === url.searchParams.get("output")) &&
            (!url.searchParams.get("task") || model.tasks.includes(url.searchParams.get("task")!)) &&
            (!url.searchParams.get("provider") || model.provider === url.searchParams.get("provider")) &&
            (!q || `${model.name} ${model.vendor ?? ""} ${model.id}`.toLowerCase().includes(q)),
        )
        .map((model) => modelJson(model))
        .filter((model) => url.searchParams.get("configured") !== "true" || model.configured);
      return sendJson(res, 200, { models: list });
    }
    if ((match = /^\/api\/models\/(.+)$/.exec(path)) && method === "GET" && match[1] !== "refresh") {
      const id = decodeURIComponent(match[1]!);
      const model = modelSeeds.find((item) => item.id === id);
      if (!model) throw new HttpError(404, "Unknown model.", { kind: "not_found" });
      await sleep(250);
      return sendJson(res, 200, modelJson(model, true));
    }
    // Workflow import: reads the graph roughly like the server and adds a ComfyUI model.
    if (path === "/api/workflows/import" && method === "POST") {
      const body = await readJson(req);
      const graph = body.workflow as Record<string, { class_type?: string; inputs?: Record<string, unknown> }> | null;
      if (!graph || typeof graph !== "object" || Array.isArray(graph)) throw new HttpError(400, "The workflow must be a JSON object.", { kind: "validation", field: "workflow" });
      if ("nodes" in graph && "links" in graph) throw new HttpError(400, "This is the ComfyUI editor format. In ComfyUI use Workflow > Export (API) and import that file.", { kind: "validation", field: "workflow" });
      const nodes = Object.values(graph);
      if (!nodes.length || nodes.some((node) => typeof node?.class_type !== "string")) throw new HttpError(400, "This is not a ComfyUI API workflow: every node needs a class_type.", { kind: "validation", field: "workflow" });
      const name = String(body.name ?? "").trim() || "Imported workflow";
      const types = nodes.map((node) => node.class_type!);
      const video = types.some((type) => /Video|VHS_/.test(type));
      const loads = types.filter((type) => /^LoadImage/.test(type)).length;
      const seed: ModelSeed = {
        id: `comfyui:${name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "")}`,
        provider: "comfyui", name, output: video ? "video" : "image", tasks: [video ? (loads ? "image_to_video" : "text_to_video") : loads ? "image_edit" : "text_to_image"],
        inputs: loads ? [{ role: video ? "start_frame" : "reference", kind: "image", min: 1, max: 1 }] : [],
        params: [{ key: "seed", label: "Seed", type: "seed", advanced: true }, { key: "steps", label: "Steps", type: "int", min: 1, max: 60, step: 1, default: 20, advanced: true }],
        prompt: types.some((type) => /TextEncode/.test(type)), negative_prompt: types.filter((type) => /TextEncode/.test(type)).length > 1, max_outputs: 1, pricing: { unit: "local" }, open_weights: true, tags: [],
        description: `Imported ComfyUI workflow (${nodes.length} nodes).`,
      };
      const existing = modelSeeds.findIndex((item) => item.id === seed.id);
      if (existing >= 0) modelSeeds.splice(existing, 1, seed);
      else modelSeeds.push(seed);
      await sleep(600);
      return sendJson(res, 201, {
        model: seed.id,
        manifest: { name, description: seed.description, output: seed.output, tasks: seed.tasks, prompt: seed.prompt ? [["1", "text"]] : [], negative_prompt: seed.negative_prompt ? [["2", "text"]] : [], inputs: seed.inputs, params: seed.params, requires: { models: [] } },
        notes: ["The mock server runs imported workflows with its fake engine."],
      });
    }
    if (path === "/api/models/refresh" && method === "POST") {
      await sleep(700);
      return sendJson(res, 200, { models: modelSeeds.length });
    }
    if (path === "/api/presets" && method === "GET") {
      const output = url.searchParams.get("output");
      return sendJson(res, 200, { presets: presets.filter((preset) => !output || preset.output === output) });
    }

    // Generations
    if (path === "/api/generations" && method === "POST") return sendJson(res, 201, createJobs(await readJson(req)));
    if (path === "/api/generations" && method === "GET") {
      const status = url.searchParams.get("status");
      const output = url.searchParams.get("output");
      const before = url.searchParams.get("before");
      const limit = Math.min(200, Number(url.searchParams.get("limit")) || 50);
      const list = [...jobs.values()]
        .filter((job) => (!status || status.split(",").includes(job.status)) && (!output || job.output === output) && (!before || job.id < before))
        .sort((a, b) => (a.id < b.id ? 1 : -1));
      const page = list.slice(0, limit);
      return sendJson(res, 200, { jobs: page, next: list.length > limit ? page[page.length - 1]!.id : null });
    }
    if ((match = /^\/api\/generations\/([^/]+)(\/cancel|\/retry)?$/.exec(path))) {
      const job = jobs.get(decodeURIComponent(match[1]!));
      if (!job) throw new HttpError(404, "No such generation.", { kind: "not_found" });
      if (match[2] === "/cancel" && method === "POST") {
        if (job.status === "queued" || job.status === "running") {
          job.status = "canceled";
          job.finished_at = Date.now();
          job.message = null;
          broadcast("job", job);
          advanceRun(job);
        }
        return sendJson(res, 200, job);
      }
      if (match[2] === "/retry" && method === "POST") {
        const again = createJobs({ model: job.model, prompt: job.prompt, negative_prompt: job.negative_prompt, params: job.params, inputs: job.inputs, count: job.count, preset: job.preset ?? undefined });
        return sendJson(res, 201, again);
      }
      if (!match[2] && method === "GET") return sendJson(res, 200, job);
      if (!match[2] && method === "DELETE") {
        jobs.delete(job.id);
        res.writeHead(204);
        return res.end();
      }
    }

    // Prompt help: a canned rewrite after a realistic wait. "error" in the prompt shows the failure.
    if (path === "/api/prompt/enhance" && method === "POST") {
      const body = await readJson(req);
      const prompt = String(body.prompt ?? "").trim();
      if (!prompt) throw new HttpError(400, "Write a short idea first.", { kind: "validation", field: "prompt" });
      await sleep(1200 + Math.random() * 1500);
      if (/error/i.test(prompt)) throw new HttpError(400, "Prompt help needs an OpenRouter key. Add one in Settings, Providers.", { kind: "validation", field: "prompt" });
      const extra =
        body.output === "video"
          ? "slow cinematic camera move, soft morning light, natural motion, gentle depth of field, 24 fps film look"
          : "natural window light, rich textures, shallow depth of field, balanced composition, shot on 35mm film";
      return sendJson(res, 200, { prompt: `${prompt.replace(/[.\s]+$/, "")}, ${extra}` });
    }

    // Recipes
    if (path === "/api/recipes" && method === "GET") return sendJson(res, 200, { recipes: recipeSeeds.map(recipeView) });
    if ((match = /^\/api\/recipes\/run\/(.+)$/.exec(path)) && method === "POST") {
      const recipe = recipeSeeds.find((item) => item.id === decodeURIComponent(match![1]!));
      if (!recipe) throw new HttpError(404, "Unknown recipe.", { kind: "not_found" });
      const body = await readJson(req);
      const values = { ...Object.fromEntries(recipe.fields.filter((field) => "default" in field && field.default).map((field) => [field.key, String((field as { default?: string }).default)])), ...((body.values ?? {}) as Record<string, string>) };
      const inputs = (body.inputs ?? {}) as Record<string, string[]>;
      for (const field of recipe.fields) if (field.required && !values[field.key]?.trim()) throw new HttpError(400, `Fill in ${field.label.toLowerCase()}.`, { kind: "validation", field: `values.${field.key}` });
      for (const input of recipe.inputs) if (input.required && !(inputs[input.key] ?? []).length) throw new HttpError(400, `Add ${input.label.toLowerCase()}.`, { kind: "validation", field: `inputs.${input.key}` });
      const models = stepModels(recipe, (body.models ?? {}) as Record<string, string>);
      const missing = recipe.steps.find((_, index) => !models[index]);
      if (missing) throw new HttpError(400, `No configured model for ${missing.label.toLowerCase()}.`, { kind: "validation", field: `models.${missing.id}` });
      const now = Date.now();
      const run: Run = { id: ulid(now), recipe_id: recipe.id, recipe_name: recipe.name, status: "running", step: 0, steps: recipe.steps.length, jobs: [], error: null, created_at: now, updated_at: now };
      recipeRuns.set(run.id, run);
      startStep(run, 0, { values, inputs, models, previous: [] });
      return sendJson(res, 201, run);
    }
    if (path === "/api/recipes/runs" && method === "GET") {
      const limit = Number(url.searchParams.get("limit")) || 30;
      return sendJson(res, 200, { runs: [...recipeRuns.values()].sort((a, b) => b.created_at - a.created_at).slice(0, limit) });
    }
    if ((match = /^\/api\/recipes\/runs\/([^/]+)(\/cancel)?$/.exec(path))) {
      const run = recipeRuns.get(decodeURIComponent(match[1]!));
      if (!run) throw new HttpError(404, "Unknown run.", { kind: "not_found" });
      if (match[2] && method === "POST" && run.status === "running") {
        const job = jobs.get(run.jobs[run.jobs.length - 1]!);
        run.status = "canceled";
        run.updated_at = Date.now();
        if (job && (job.status === "queued" || job.status === "running")) {
          job.status = "canceled";
          job.finished_at = Date.now();
          broadcast("job", job);
        }
        broadcast("recipe", run);
      }
      return sendJson(res, 200, run);
    }

    // Characters: named sets of reference images.
    if (path === "/api/characters" && method === "GET") return sendJson(res, 200, { characters: [...characters.values()].sort((a, b) => b.updated_at - a.updated_at).map(characterJson) });
    if (path === "/api/characters" && method === "POST") {
      const body = await readJson(req);
      const character = { id: ulid(), name: "", description: "", asset_ids: [] as string[], created_at: Date.now(), updated_at: Date.now() };
      applyCharacter(character, body, true);
      characters.set(character.id, character);
      broadcast("character", characterJson(character));
      return sendJson(res, 201, characterJson(character));
    }
    if ((match = /^\/api\/characters\/([^/]+)$/.exec(path))) {
      const character = characters.get(decodeURIComponent(match[1]!));
      if (!character) throw new HttpError(404, "No such character.", { kind: "not_found" });
      if (method === "GET") return sendJson(res, 200, characterJson(character));
      if (method === "PATCH") {
        applyCharacter(character, await readJson(req), false);
        character.updated_at = Date.now();
        broadcast("character", characterJson(character));
        return sendJson(res, 200, characterJson(character));
      }
      if (method === "DELETE") {
        characters.delete(character.id);
        broadcast("character", { id: character.id, deleted: true });
        res.writeHead(204);
        return res.end();
      }
    }

    // Billing (hosted)
    if (path === "/api/billing" && method === "GET") {
      return sendJson(res, 200, { enabled: wallet.enabled, topups_available: wallet.enabled, balance_usd: wallet.balance, debt_usd: wallet.debt, locked: wallet.balance < 0, markup: wallet.markup, topups_usd: [10, 25, 50], ledger: wallet.ledger.slice(0, 50) });
    }
    if (path === "/api/billing/checkout" && method === "POST") {
      if (!wallet.enabled) throw new HttpError(400, "Billing is off.", { kind: "validation" });
      const body = await readJson(req);
      const cents = Number(body.amount_cents);
      if (![1000, 2500, 5000].includes(cents)) throw new HttpError(400, "Pick one of the offered amounts.", { kind: "validation", field: "amount_cents" });
      return sendJson(res, 200, { url: `/__mock/checkout?cents=${cents}` });
    }
    if (path === "/__mock/checkout" && method === "GET") {
      const cents = Number(url.searchParams.get("cents")) || 0;
      wallet.balance = Math.round((wallet.balance + cents / 100) * 100) / 100;
      wallet.ledger.unshift({ id: ulid(), amount_usd: cents / 100, reason: "Top-up", job_id: null, created_at: Date.now() });
      res.writeHead(302, { Location: "/settings/billing?checkout=success" });
      return res.end();
    }

    // Assets
    if (path === "/api/assets" && method === "GET") return sendJson(res, 200, listAssets(url.searchParams));
    if (path === "/api/assets" && method === "POST") {
      const raw = await readBody(req);
      const form = await new Request("http://mock.local/upload", { method: "POST", headers: { "content-type": req.headers["content-type"] ?? "" }, body: raw }).formData();
      const files = form.getAll("file").filter((entry) => typeof entry !== "string") as unknown as { name: string; type: string; arrayBuffer(): Promise<ArrayBuffer> }[];
      if (!files.length) throw new HttpError(400, "Send at least one file.", { kind: "validation", field: "file" });
      await sleep(300);
      // Like the server: keep every real media file, list the others with a reason.
      const created: Asset[] = [];
      const errors: { name: string; error: string }[] = [];
      for (const file of files) {
        const data = Buffer.from(await file.arrayBuffer());
        const kind = file.type.split("/")[0] as Kind;
        const text = data.subarray(0, 64).toString("utf8").trimStart();
        if (!["image", "video", "audio"].includes(kind) || file.type === "image/svg+xml" || text.startsWith("<")) {
          errors.push({ name: file.name, error: "Pictesa can only use image, video and audio files, and this file is something else." });
          continue;
        }
        const id = ulid();
        const size = kind === "image" ? imageSize(data) : null;
        const asset = addAsset({
          asset: { id, kind, mime: file.type, width: size?.[0] ?? null, height: size?.[1] ?? null, duration_ms: null, size: data.length, url: `/files/${id}`, thumb_url: kind === "image" ? `/thumbs/${id}` : null, source: "upload", name: file.name, job_id: null, model: null, prompt: null, favorite: false, created_at: Date.now(), deleted_at: null },
          file: { type: "bytes", data, mime: file.type },
          thumb: kind === "image" ? { type: "same" } : null,
        });
        broadcast("asset", asset);
        created.push(asset);
      }
      if (!created.length) throw new HttpError(400, errors[0]?.error ?? "No file could be stored.", { kind: "validation", field: "file", errors });
      return sendJson(res, 201, { assets: created, errors });
    }
    if (path === "/api/assets/import" && method === "POST") {
      const body = await readJson(req);
      const link = String(body.url ?? "");
      if (!/^https?:\/\//.test(link)) throw new HttpError(400, "Use an http or https link.", { kind: "validation", field: "url" });
      let response: Response;
      try {
        response = await fetch(link, { signal: AbortSignal.timeout(10_000) });
      } catch {
        throw new HttpError(400, "That link could not be downloaded.", { kind: "validation", field: "url" });
      }
      const mime = (response.headers.get("content-type") ?? "").split(";")[0]!.trim();
      const kind = mime.split("/")[0] as Kind;
      if (!response.ok || !["image", "video", "audio"].includes(kind)) throw new HttpError(400, "That link is not an image, video or audio file.", { kind: "validation", field: "url" });
      const data = Buffer.from(await response.arrayBuffer());
      const id = ulid();
      const size = kind === "image" ? imageSize(data) : null;
      const asset = addAsset({
        asset: { id, kind, mime, width: size?.[0] ?? null, height: size?.[1] ?? null, duration_ms: null, size: data.length, url: `/files/${id}`, thumb_url: kind === "image" ? `/thumbs/${id}` : null, source: "import", name: decodeURIComponent(new URL(link).pathname.split("/").pop() || "import"), job_id: null, model: null, prompt: null, favorite: false, created_at: Date.now(), deleted_at: null },
        file: { type: "bytes", data, mime },
        thumb: kind === "image" ? { type: "same" } : null,
      });
      broadcast("asset", asset);
      return sendJson(res, 201, { assets: [asset] });
    }
    if ((match = /^\/api\/assets\/([^/]+)(\/restore)?$/.exec(path))) {
      const record = assets.get(decodeURIComponent(match[1]!));
      if (!record) throw new HttpError(404, "That item is not in the library.", { kind: "not_found" });
      const asset = record.asset;
      if (match[2] && method === "POST") {
        asset.deleted_at = null;
        broadcast("asset", asset);
        return sendJson(res, 200, asset);
      }
      if (method === "GET") return sendJson(res, 200, { ...asset, job: asset.job_id ? (jobs.get(asset.job_id) ?? null) : null });
      if (method === "PATCH") {
        const body = await readJson(req);
        if (typeof body.favorite === "boolean") asset.favorite = body.favorite;
        broadcast("asset", asset);
        return sendJson(res, 200, asset);
      }
      if (method === "DELETE") {
        if (url.searchParams.get("permanent") === "true") assets.delete(asset.id);
        else {
          asset.deleted_at = Date.now();
          broadcast("asset", asset);
        }
        res.writeHead(204);
        return res.end();
      }
    }

    throw new HttpError(404, `No mock route for ${method} ${path}.`, { kind: "not_found" });
  }

  return { handle };
}
