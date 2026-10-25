// Shapes of the Pictesa HTTP API (docs/API.md). The server generates models, params and input
// slots, so nothing here names a specific model or provider.

export type MediaKind = "image" | "video" | "audio";

export type Task =
  | "text_to_image"
  | "image_edit"
  | "text_to_video"
  | "image_to_video"
  | "video_to_video"
  | "upscale"
  | "lip_sync"
  | "text_to_audio";

export type InputRole = "reference" | "start_frame" | "end_frame" | "video" | "audio" | "mask";

/** Param values are plain JSON scalars. */
export type ParamValue = string | number | boolean | null;
export type ParamValues = Record<string, ParamValue>;

export interface InputSlot {
  role: InputRole;
  kind: MediaKind;
  min: number;
  max: number;
  label?: string | null;
}

export interface EnumOption {
  value: ParamValue;
  label: string;
}

interface ParamBase {
  key: string;
  label: string;
  default?: ParamValue;
  advanced?: boolean;
  help?: string | null;
}

export type ParamSpec = ParamBase &
  (
    | { type: "enum"; options: EnumOption[] }
    | { type: "int"; min: number; max: number; step?: number }
    | { type: "float"; min: number; max: number; step?: number }
    | { type: "bool" }
    | { type: "text"; multiline?: boolean; max_len?: number | null }
    | { type: "seed" }
  );

export type Pricing =
  | { unit: "per_image" | "per_second" | "per_video" | "per_megapixel"; usd: number }
  | { unit: "local" }
  | { unit: "variable"; note: string };

export interface ModelSpec {
  id: string;
  provider: string;
  name: string;
  vendor?: string | null;
  output: MediaKind;
  tasks: Task[];
  inputs: InputSlot[];
  params: ParamSpec[];
  prompt: boolean;
  negative_prompt: boolean;
  max_outputs: number;
  pricing?: Pricing | null;
  open_weights: boolean;
  description?: string | null;
  tags?: string[];
  configured: boolean;
  /** False for discovered gateway models listed without params or inputs; load GET /api/models/{id}. */
  complete?: boolean;
  /** Hosted mode: runs are paid from the wallet and `pricing` already includes the markup. */
  billed?: boolean;
  meta?: Record<string, unknown> | null;
}

export type ProviderKind = "cloud" | "gateway" | "local";

export interface CredentialField {
  key: string;
  label: string;
  env?: string[];
  secret: boolean;
  optional?: boolean;
  default?: string | null;
  placeholder?: string | null;
}

export interface FieldValue {
  set: boolean;
  source?: "stored" | "env" | "default" | null;
  /** Only present for non-secret fields. */
  value?: string | null;
}

export interface ProviderState {
  id: string;
  name: string;
  kind: ProviderKind;
  fields: CredentialField[];
  signup_url?: string | null;
  docs_url?: string | null;
  configured: boolean;
  values: Record<string, FieldValue>;
  model_count: number;
}

export interface Preset {
  id: string;
  name: string;
  category: string;
  output: MediaKind;
  prompt_suffix?: string | null;
  params?: ParamValues | null;
  description?: string | null;
}

export type JobStatus = "queued" | "running" | "succeeded" | "failed" | "canceled";

export interface JobInput {
  asset_id: string;
  role: InputRole;
}

export interface Job {
  id: string;
  batch_id: string;
  model: string;
  provider: string;
  model_name: string;
  output: MediaKind;
  status: JobStatus;
  prompt: string;
  negative_prompt: string | null;
  params: ParamValues;
  inputs: JobInput[];
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

/** `edit`: masks and expanded canvases made by the edit view, hidden from library lists. */
export type AssetSource = "generated" | "upload" | "import" | "edit";

export interface Asset {
  id: string;
  kind: MediaKind;
  mime: string;
  width: number | null;
  height: number | null;
  duration_ms: number | null;
  size: number;
  url: string;
  thumb_url: string | null;
  source: AssetSource;
  /** Original file name of uploads and imports. */
  name?: string | null;
  /** Edit helpers: the image this mask or expanded canvas was made from. */
  origin_id?: string | null;
  job_id: string | null;
  model: string | null;
  prompt: string | null;
  favorite: boolean;
  created_at: number;
  deleted_at: number | null;
}

export type AssetDetail = Asset & { job: Job | null };

export interface User {
  id: string;
  email: string;
  name: string | null;
  role: "admin" | "user";
  created_at: number;
}

export type Mode = "local" | "hosted";

export interface Me {
  mode: Mode;
  user: User | null;
  /** Local mode with PICTESA_TOKEN set and no cookie yet. */
  needs_token?: boolean;
}

export interface Health {
  ok: boolean;
  version: string;
  mode: Mode;
  ffmpeg: boolean;
  data_dir: string;
}

export interface ApiToken {
  id: string;
  name: string;
  created_at: number;
  last_used_at: number | null;
}

export interface GenerationRequest {
  model: string;
  prompt: string;
  negative_prompt: string | null;
  params: ParamValues;
  inputs: JobInput[];
  count: number;
  preset?: string;
  /** Up to 7 more models that run the same prompt, inputs and params in the same batch. */
  compare?: string[];
  /** A character id: its images join as reference inputs, up to what the model takes. */
  character?: string;
}

/** What Pictesa read from an imported ComfyUI workflow (the manifest it wrote). */
export interface WorkflowManifest {
  name?: string;
  description?: string;
  output?: MediaKind;
  tasks?: Task[];
  prompt?: [string, string][];
  negative_prompt?: [string, string][];
  inputs?: { role: string; kind: MediaKind; min?: number; label?: string }[];
  params?: { key: string; label?: string; type?: string }[];
  requires?: { models?: { directory?: string; name: string }[] };
}

export interface WorkflowImport {
  model: string;
  manifest: WorkflowManifest;
  notes: string[];
}

/** A named set of 1 to 14 library images (a person, a product, a style). */
export interface Character {
  id: string;
  name: string;
  description: string | null;
  assets: Asset[];
  created_at: number;
  updated_at: number;
}

export interface RecipeField {
  key: string;
  label: string;
  type: "text" | "enum" | "preset";
  options?: string[];
  category?: string | null;
  default?: string | null;
  required?: boolean;
  multiline?: boolean;
  help?: string | null;
}

export interface RecipeInput {
  key: string;
  label: string;
  kind: MediaKind;
  max: number;
  required?: boolean;
}

export interface RecipeStep {
  id: string;
  label: string;
  output: MediaKind;
  models: string[];
  prompt?: string | null;
  params?: Record<string, ParamValue> | null;
  preset?: string | null;
  inputs?: { from: string; role: InputRole }[];
}

export interface Recipe {
  id: string;
  name: string;
  description?: string | null;
  output: MediaKind;
  fields: RecipeField[];
  inputs: RecipeInput[];
  steps: RecipeStep[];
  /** The model each step uses by default, or null when none is configured. */
  step_models: (string | null)[];
  runnable: boolean;
}

export type RunStatus = "running" | "succeeded" | "failed" | "canceled";

export interface RecipeRun {
  id: string;
  recipe_id: string;
  recipe_name: string;
  status: RunStatus;
  /** Index of the current (or last) step. */
  step: number;
  steps: number;
  jobs: string[];
  error: string | null;
  created_at: number;
  updated_at: number;
}

export interface LedgerEntry {
  id: string;
  amount_usd: number;
  reason: string;
  job_id: string | null;
  created_at: number;
}

export interface Billing {
  enabled: boolean;
  topups_available: boolean;
  balance_usd: number;
  debt_usd: number;
  locked: boolean;
  markup: number;
  topups_usd: number[];
  ledger: LedgerEntry[];
}

/** Pixels added on each side of an image (POST /api/assets/{id}/expand). */
export interface Padding {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface AssetQuery {
  kind?: MediaKind;
  favorite?: boolean;
  q?: string;
  model?: string;
  source?: AssetSource;
  trash?: boolean;
}
