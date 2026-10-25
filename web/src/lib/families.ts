import type { AttachedInput } from "./composer";
import type { ArenaScore, MediaKind, ModelSpec, Pricing, Task } from "./types";

// One model is often listed several times: by several providers, and once per task (fal has a
// text-to-image and an edit endpoint for the same model). The picker shows one row per family and
// the composer picks the member that fits what the person is doing.

export interface Family {
  key: string;
  name: string;
  /** Who made the weights, when known. */
  maker: string | null;
  /** Set-up members first, then the cheapest. */
  variants: ModelSpec[];
  configured: boolean;
}

const TASK = "(?:(?:text|image|video|reference|keyframes|first[- ]last[- ]frame)[ -]to[ -](?:image|video)|edit(?: image)?)";
const TASK_TAIL = new RegExp(`(?:\\s+${TASK}|\\s*\\((?:${TASK}|preview|beta)\\)|\\s+(?:API|[TIV]2[IV]))$`, "i");

/** "Google: Nano Banana 2.1 Edit (preview)" becomes "Nano Banana 2.1". */
export function cleanName(name: string): string {
  let clean = name.replace(/^[\w .&-]{1,40}:\s+/, "").trim();
  for (let before = ""; before !== clean; ) {
    before = clean;
    clean = clean.replace(TASK_TAIL, "").trim();
  }
  return clean || name;
}

/** Words that tell one version of a model from another. */
const TIERS = new Set(["pro", "max", "lite", "flash", "turbo", "ultra", "fast", "mini", "plus", "standard", "prime", "dev", "schnell", "instruct", "klein"]);
const tokens = (text: string) => text.toLowerCase().split(/[^a-z0-9.]+/).map((word) => word.replace(/^v(?=\d)/, "").replace(/\.0$/, "")).filter(Boolean);
const title = (word: string) => (/\d/.test(word) ? word : word[0]!.toUpperCase() + word.slice(1));

/** A comparable cost per output: a 5 s clip for per-second video prices, one image per megapixel. */
export function unitCost(pricing: Pricing | null | undefined): number {
  if (!pricing || pricing.unit === "variable") return Infinity;
  if (pricing.unit === "local") return 0;
  return pricing.unit === "per_second" ? pricing.usd * 5 : pricing.usd;
}

const byPreference = (a: ModelSpec, b: ModelSpec) =>
  Number(b.configured) - Number(a.configured) || unitCost(a.pricing) - unitCost(b.pricing) || Number(Boolean(b.tags?.includes("featured"))) - Number(Boolean(a.tags?.includes("featured")));

/**
 * The most common clean name, preferring written names ("Nano Banana 2.1") to raw ids. A name that
 * says neither version nor tier ("Seedream" on fal) gets them from the family key: "Seedream 5 Flash".
 */
function familyName(key: string, variants: ModelSpec[]): string {
  const counts = new Map<string, number>();
  for (const model of variants) {
    const name = cleanName(model.name);
    counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  // Raw ids ("gpt-image-2") lose to written names; on a tie, a name with a version wins.
  const written = (name: string) => (/^[a-z0-9._/-]+$/.test(name) && /[-/_]/.test(name) ? 0 : 1);
  const versioned = (name: string) => (/\d/.test(name) ? 1 : 0);
  const name = [...counts].sort(([a, x], [b, y]) => written(b) - written(a) || y - x || versioned(b) - versioned(a) || a.length - b.length)[0]![0];
  const own = tokens(name);
  // Only when no provider names a version: "Nano Banana" stays, Google calls it Gemini 2.5 Flash.
  if (own.some((word) => /\d/.test(word) || TIERS.has(word)) || !written(name) || [...counts.keys()].some((other) => versioned(other))) return name;
  const keyWords = key.split(" ");
  const extra = [...keyWords.filter((word) => /\d/.test(word)), ...keyWords.filter((word) => TIERS.has(word))].filter((word) => !own.includes(word));
  return [name, ...extra.map(title)].join(" ");
}

function familyMaker(variants: ModelSpec[]): string | null {
  const makers = variants.map((model) => model.vendor).filter((vendor): vendor is string => Boolean(vendor));
  // A capitalised name ("Google") reads better than an id prefix ("google").
  return makers.find((maker) => maker[0] !== maker[0]?.toLowerCase()) ?? makers[0] ?? null;
}

export function groupFamilies(models: ModelSpec[]): Family[] {
  const groups = new Map<string, ModelSpec[]>();
  for (const model of models) {
    const key = model.family || model.id;
    groups.set(key, [...(groups.get(key) ?? []), model]);
  }
  // Keys that differ only in version digits and carry the same name are one model
  // ("fal-ai/flux/schnell" and "fal-ai/flux-1/schnell" are both FLUX.1 [schnell]).
  const byName = new Map<string, { key: string; name: string; members: ModelSpec[] }>();
  for (const [key, members] of groups) {
    const name = familyName(key, members);
    const same = `${name.toLowerCase()}|${key.split(" ").filter((word) => !/\d/.test(word)).join(" ")}`;
    const found = byName.get(same);
    if (found) found.members.push(...members);
    else byName.set(same, { key, name, members: [...members] });
  }
  return [...byName.values()].map(({ key, name, members }) => {
    const variants = members.sort(byPreference);
    return { key, name, maker: familyMaker(variants), variants, configured: variants.some((model) => model.configured) };
  });
}

/** What the composer is doing with its inputs: editing when a reference image is attached. */
export function intentOf(mode: MediaKind, inputs: AttachedInput[]): Task | null {
  const has = (...roles: string[]) => inputs.some((input) => (input.asset || input.upload) && roles.includes(input.role));
  if (mode === "image") return has("reference", "mask") ? "image_edit" : "text_to_image";
  if (mode === "video") return has("video") ? "video_to_video" : has("start_frame", "end_frame", "reference") ? "image_to_video" : "text_to_video";
  return null;
}

/**
 * The member to run when a family is picked: the current model when it belongs, else one fitting
 * the intent, preferring recently used ones, then set-up and cheaper ones.
 */
export function pickVariant(family: Family, intent: Task | null, current?: ModelSpec | null, recent: string[] = []): ModelSpec {
  const fitting = family.variants.filter((model) => !intent || model.tasks.includes(intent));
  const pool = fitting.length ? fitting : family.variants;
  if (current && pool.some((model) => model.id === current.id)) return current;
  const used = recent.map((id) => pool.find((model) => model.id === id && model.configured)).find(Boolean);
  return used ?? pool[0]!;
}

/** The member of `model`'s family that does `intent`: the model itself, or a set-up sibling from the same provider first. */
export function variantFor(models: ModelSpec[], model: ModelSpec, intent: Task): ModelSpec | null {
  if (model.tasks.includes(intent)) return model;
  const key = model.family || model.id;
  const siblings = models.filter((other) => other.id !== model.id && (other.family || other.id) === key && other.configured && other.tasks.includes(intent)).sort(byPreference);
  return siblings.find((other) => other.provider === model.provider) ?? siblings[0] ?? null;
}

/** The family's standing in the arena of `intent`, else its best one. */
export function familyScore(family: Family, intent: Task | null): ArenaScore | undefined {
  const scores = family.variants.flatMap((model) => model.arena ?? []);
  return scores.find((score) => score.arena === intent) ?? scores.sort((a, b) => a.rank - b.rank)[0];
}
