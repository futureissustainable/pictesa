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

export type AssetSource = "generated" | "upload" | "import";

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
}

export interface AssetQuery {
  kind?: MediaKind;
  favorite?: boolean;
  q?: string;
  model?: string;
  source?: AssetSource;
  trash?: boolean;
}
