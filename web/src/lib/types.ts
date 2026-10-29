// Shapes of the Pictesa HTTP API (docs/API.md). The server generates models, params and input
// slots, so nothing here names a specific model or provider.

export type MediaKind = "image" | "video" | "audio" | "lora";

export type Task =
  | "text_to_image"
  | "image_edit"
  | "text_to_video"
  | "image_to_video"
  | "video_to_video"
  | "video_to_audio"
  | "upscale"
  | "lip_sync"
  | "text_to_audio"
  | "training";

export type InputRole = "reference" | "start_frame" | "end_frame" | "video" | "audio" | "mask";

/** Param values are plain JSON scalars. */
export type ParamValue = string | number | boolean | null | LoraSelection[];
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
  | { unit: "per_run" | "per_step" | "per_image" | "per_second" | "per_video" | "per_megapixel"; usd: number }
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
  /** LMArena standings in the arenas that rate this model's tasks. */
  arena?: ArenaScore[];
  /** When the provider published the model (YYYY-MM-DD). */
  released?: string | null;
  /** Shared by the same model on other providers and for other tasks. */
  family?: string;
}

/** One file of a ComfyUI model download (GET /api/downloads). */
export interface Download {
  model: string;
  /** `<directory>/<name>` inside the ComfyUI models folder. */
  file: string;
  done: number;
  total: number | null;
  status: "queued" | "running" | "done" | "failed";
  error: string | null;
}

export interface ArenaScore {
  /** The task the arena rates: text_to_image, image_edit, text_to_video or image_to_video. */
  arena: string;
  /** The model's name on the leaderboard. */
  name: string;
  rank: number;
  /** Models rated in that arena. */
  of: number;
  rating: number;
  votes: number;
  /** YYYY-MM-DD. */
  published: string;
}

export interface ModelPrefs {
  pinned: string[];
  /** Models of the latest generations, newest first. */
  recent: string[];
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
  made_from?: string | null;
  job_id: string | null;
  batch_id?: string | null;
  model: string | null;
  prompt: string | null;
  negative_prompt?: string | null;
  origin?: string | null;
  model_label?: string | null;
  params?: Record<string, unknown>;
  source_url?: string | null;
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
  /** Hosted mode: false when the operator closed sign-up. */
  signup?: boolean;
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
export type CharacterKind = "character" | "brand";

export interface Character {
  id: string;
  name: string;
  description: string | null;
  /** "brand" marks a brand kit: its colors and words join every prompt it is used in. */
  kind?: CharacterKind;
  /** Brand colors as #rrggbb. */
  colors?: string[];
  /** Brand words: tone, style and phrases. */
  words?: string;
  assets: Asset[];
  created_at: number;
  updated_at: number;
}

export interface RecipeField {
  key: string;
  label: string;
  /** `multi` takes several of `options`. */
  type: "text" | "enum" | "multi" | "preset";
  options?: string[];
  category?: string | null;
  default?: string | string[] | null;
  required?: boolean;
  /** Required unless this input or field has a value. */
  required_without?: string | null;
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
  /** The step runs once per item: fixed texts, or a lone `{{field}}` for that field's items. */
  each?: string[];
  /** What to set up when none of `models` is ready. */
  needs?: string | null;
}

export interface Recipe {
  id: string;
  name: string;
  description?: string | null;
  output: MediaKind;
  /** Card picture: a built-in illustration name or a library image id. */
  picture?: string | null;
  /** Offers a character or brand kit for every step. */
  character?: boolean;
  /** A studio page that runs this recipe with a richer tool. */
  open?: string | null;
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
  /** Jobs per step; a step that runs once per item has one job per item. */
  step_jobs?: string[][];
  /** The final step's jobs: their outputs are the results (a set when it ran per item). */
  results?: string[];
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

export interface Collection {
  id: string;
  name: string;
  created_at: number;
  asset_count: number;
}

export interface SpendGroup { name: string; usd: number }
export interface BudgetProgress { cap_usd: number | null; spent_usd: number; remaining_usd: number | null }
export interface Spend {
  today_usd: number;
  seven_days_usd: number;
  thirty_days_usd: number;
  days: SpendGroup[];
  models: SpendGroup[];
  providers: SpendGroup[];
  daily: BudgetProgress;
  monthly: BudgetProgress;
  unknown_runs: number;
  estimated_runs: number;
  timezone: string;
}

export interface AssetQuery {
  collection?: string;
  tag?: string;
  provider?: string;
  cost_min?: number;
  cost_max?: number;
  kind?: MediaKind;
  favorite?: boolean;
  q?: string;
  model?: string;
  source?: AssetSource;
  trash?: boolean;
}

export interface PromptHistory { prompt: string; model: string; model_name: string; last_used_at: number }
export interface PromptContent { name: string; prompt: string; negative_prompt?: string | null; model?: string | null; params?: ParamValues | null }
export interface SavedPrompt extends PromptContent { id: string; placeholders: string[]; created_at: number; updated_at: number }
export interface BatchRequest extends GenerationRequest {
  prompts?: string[];
  prompt_list?: string;
  prompt_format?: "lines" | "csv";
  seeds?: { start: number; count: number };
  grid?: { key: string; values: ParamValue[] }[];
}
export interface BatchEstimate { run_count: number; total_estimated_cost_usd: number | null; max_runs: number }

/** Boards: a canvas of library files, notes and steps (docs/API.md, Boards). */
export type BoardRole = "owner" | "editor" | "viewer";
export type NodeKind = "asset" | "note" | "step";

export interface BoardNode {
  id: string;
  kind: NodeKind;
  x: number;
  y: number;
  /** Width on the board; heights follow the content. */
  w: number;
  /** The board version of this node's last change. */
  rev: number;
  asset_id?: string;
  text?: string;
  model?: string;
  prompt?: string;
  negative_prompt?: string;
  params?: ParamValues;
  count?: number;
}

/** Into a step, an edge is an input: `role` picks the slot, none the first free one. */
export interface BoardEdge {
  id: string;
  from: string;
  to: string;
  role?: InputRole | null;
  rev: number;
}

export type StepRunStatus = "waiting" | "starting" | "running" | "succeeded" | "failed" | "canceled";

export interface StepRun {
  status: StepRunStatus;
  by: string;
  jobs?: string[];
  /** Results of the step's last finished run. */
  outputs?: string[];
  after?: string[];
  error?: string | null;
  updated_at: number;
  rev: number;
}

export interface Person {
  id: string;
  name: string | null;
  email: string | null;
}

export interface BoardMember extends Person {
  role: BoardRole;
  added_at: number;
}

export interface Board {
  id: string;
  name: string;
  role: BoardRole;
  owner: Person;
  version: number;
  created_at: number;
  updated_at: number;
  nodes: BoardNode[];
  edges: BoardEdge[];
  runs: Record<string, StepRun>;
  assets: Record<string, Asset>;
  members: BoardMember[];
}

export interface BoardSummary {
  id: string;
  name: string;
  role: BoardRole;
  owner: Person;
  version: number;
  created_at: number;
  updated_at: number;
  node_count: number;
  member_count: number;
  covers: Asset[];
}

/** One applied change (an answer or a `board` event): current items, removed ids, new files. */
export interface BoardChanges {
  board_id: string;
  version: number;
  name?: string;
  nodes?: BoardNode[];
  edges?: BoardEdge[];
  runs?: Record<string, StepRun>;
  removed_nodes?: string[];
  removed_edges?: string[];
  assets?: Record<string, Asset>;
  by?: Person;
  /** Another process changed the board: load it again. */
  reload?: boolean;
}

export type NodeFields = Pick<BoardNode, "w" | "text" | "model" | "prompt" | "negative_prompt" | "params" | "count">;

export type BoardOp =
  | { op: "add_node"; node: Partial<BoardNode> & Pick<BoardNode, "id" | "kind" | "x" | "y"> }
  | { op: "move_node"; id: string; base: number; x: number; y: number }
  | ({ op: "update_node"; id: string; base: number } & Partial<NodeFields>)
  | { op: "delete_node"; id: string; base: number }
  | { op: "add_edge"; edge: { id: string; from: string; to: string; role?: InputRole | null } }
  | { op: "update_edge"; id: string; base: number; role: InputRole | null }
  | { op: "delete_edge"; id: string; base: number }
  | { op: "rename"; name: string };

export interface Lora { asset: Asset; character_id: string | null; family: string; trigger_word: string }
export interface LoraSelection { asset_id: string; strength: number }
export interface TrainRequest { trainer: string; character?: string; asset_ids?: string[]; trigger_word: string; family: string; steps: number; rank: number; learning_rate: number }
