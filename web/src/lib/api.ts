import type {
  ApiToken,
  Asset,
  Billing,
  Collection,
  Spend,
  Character,
  AssetDetail,
  AssetQuery,
  GenerationRequest,
  Health,
  Job,
  MediaKind,
  Me,
  ModelPrefs,
  ModelSpec,
  Padding,
  Preset,
  ProviderState,
  Recipe,
  RecipeRun,
  User,
  WorkflowImport,
} from "./types";

export interface FileError {
  name: string;
  error: string;
}

/** A failed API call. `field` points at the offending input ("params.duration", "prompt"). */
export class ApiError extends Error {
  readonly status: number;
  readonly kind?: string;
  readonly field?: string;
  /** Uploads: the files the server refused, with its reason for each. */
  readonly errors?: FileError[];
  constructor(message: string, status: number, kind?: string, field?: string, errors?: FileError[]) {
    super(message);
    this.status = status;
    this.kind = kind;
    this.field = field;
    this.errors = errors;
  }
}

export const OFFLINE_MESSAGE = "Pictesa is not answering. Check that the server is running.";

type QueryValue = string | number | boolean | null | undefined;

export function withQuery(path: string, query: Record<string, QueryValue> = {}): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== null && value !== "") params.set(key, String(value));
  }
  const text = params.toString();
  if (!text) return path;
  return `${path}${path.includes("?") ? "&" : "?"}${text}`;
}

const unauthorized = new Set<() => void>();
/** Called when any request answers 401, so the app can show the login screen. */
export function onUnauthorized(listener: () => void): () => void {
  unauthorized.add(listener);
  return () => unauthorized.delete(listener);
}

function errorFrom(status: number, data: unknown): ApiError {
  const body = (data && typeof data === "object" ? data : {}) as { error?: unknown; kind?: unknown; field?: unknown; errors?: unknown };
  const message = typeof body.error === "string" && body.error ? body.error : `The request failed (${status}).`;
  const errors = Array.isArray(body.errors) ? (body.errors as FileError[]) : undefined;
  return new ApiError(message, status, typeof body.kind === "string" ? body.kind : undefined, typeof body.field === "string" ? body.field : undefined, errors);
}

async function request<T>(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  let response: Response;
  try {
    response = await fetch(path, {
      method,
      signal,
      credentials: "same-origin",
      headers:
        body === undefined
          ? { Accept: "application/json" }
          : { Accept: "application/json", "Content-Type": body instanceof Blob ? body.type || "application/octet-stream" : "application/json" },
      body: body === undefined ? undefined : body instanceof Blob ? body : JSON.stringify(body),
    });
  } catch (error) {
    if (signal?.aborted) throw error;
    throw new ApiError(OFFLINE_MESSAGE, 0, "network");
  }
  if (response.status === 401) unauthorized.forEach((listener) => listener());
  if (response.status === 204) return undefined as T;
  const text = await response.text();
  let data: unknown = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }
  }
  if (!response.ok) throw errorFrom(response.status, data);
  return data as T;
}

const id = (value: string) => encodeURIComponent(value);

export const api = {
  me: () => request<Me>("GET", "/api/auth/me"),
  login: (body: { email: string; password: string }) => request<User>("POST", "/api/auth/login", body),
  submitToken: (token: string) => request<void>("POST", "/api/auth/token", { token }),
  signup: (body: { email: string; password: string; name?: string }) => request<User>("POST", "/api/auth/signup", body),
  logout: () => request<void>("POST", "/api/auth/logout"),
  tokens: () => request<{ tokens: ApiToken[] }>("GET", "/api/tokens"),
  createToken: (name: string) => request<{ id: string; name: string; token: string }>("POST", "/api/tokens", { name }),
  deleteToken: (tokenId: string) => request<void>("DELETE", `/api/tokens/${id(tokenId)}`),

  health: () => request<Health>("GET", "/api/health"),

  providers: () => request<{ providers: ProviderState[] }>("GET", "/api/providers"),
  saveProvider: (providerId: string, values: Record<string, string>) =>
    request<ProviderState>("PUT", `/api/providers/${id(providerId)}`, { values }),
  forgetProvider: (providerId: string) => request<void>("DELETE", `/api/providers/${id(providerId)}`),
  checkProvider: (providerId: string) => request<{ ok: boolean; error?: string }>("POST", `/api/providers/${id(providerId)}/check`),

  models: (query: { output?: MediaKind; task?: string; provider?: string; q?: string; configured?: boolean } = {}) =>
    request<{ models: ModelSpec[] }>("GET", withQuery("/api/models", query)),
  /** Full spec of one model. Ids contain ":" and "/", which the server expects as they are. */
  model: (modelId: string) => request<ModelSpec>("GET", `/api/models/${encodeURI(modelId).replaceAll("?", "%3F").replaceAll("#", "%23")}`),
  refreshModels: () => request<{ models: number }>("POST", "/api/models/refresh"),
  modelPrefs: () => request<ModelPrefs>("GET", "/api/models/prefs"),
  setPinnedModels: (pinned: string[]) => request<ModelPrefs>("PUT", "/api/models/prefs", { pinned }),
  importWorkflow: (name: string, workflow: unknown) => request<WorkflowImport>("POST", "/api/workflows/import", { name, workflow }),
  presets: (output: MediaKind) => request<{ presets: Preset[] }>("GET", withQuery("/api/presets", { output })),
  enhancePrompt: (prompt: string, output: MediaKind) => request<{ prompt: string }>("POST", "/api/prompt/enhance", { prompt, output }),

  generate: (body: GenerationRequest) => request<{ batch_id: string; jobs: Job[] }>("POST", "/api/generations", body),
  jobs: (query: { status?: string; output?: MediaKind; limit?: number; before?: string } = {}) =>
    request<{ jobs: Job[]; next: string | null }>("GET", withQuery("/api/generations", query)),
  job: (jobId: string) => request<Job>("GET", `/api/generations/${id(jobId)}`),
  cancelJob: (jobId: string) => request<Job>("POST", `/api/generations/${id(jobId)}/cancel`),
  retryJob: (jobId: string) => request<{ batch_id: string; jobs: Job[] }>("POST", `/api/generations/${id(jobId)}/retry`),

  assets: (query: AssetQuery & { limit?: number; before?: string | null }, signal?: AbortSignal) =>
    request<{ assets: Asset[]; next: string | null }>("GET", withQuery("/api/assets", { ...query }), undefined, signal),
  asset: (assetId: string, signal?: AbortSignal) => request<AssetDetail>("GET", `/api/assets/${id(assetId)}`, undefined, signal),
  patchAsset: (assetId: string, patch: { favorite?: boolean }) => request<Asset>("PATCH", `/api/assets/${id(assetId)}`, patch),
  deleteAsset: (assetId: string, permanent = false) =>
    request<void>("DELETE", withQuery(`/api/assets/${id(assetId)}`, { permanent })),
  restoreAsset: (assetId: string) => request<Asset>("POST", `/api/assets/${id(assetId)}/restore`),
  importAsset: (url: string) => request<{ assets: Asset[] }>("POST", "/api/assets/import", { url }),
  /** Stores a painted mask (any image; paint is its opaque or light pixels) for an image. */
  saveMask: (assetId: string, mask: Blob) => request<Asset>("POST", `/api/assets/${id(assetId)}/mask`, mask),
  expandCanvas: (assetId: string, padding: Padding) => request<{ image: Asset; mask: Asset }>("POST", `/api/assets/${id(assetId)}/expand`, padding),

  recipes: () => request<{ recipes: Recipe[] }>("GET", "/api/recipes"),
  runRecipe: (recipeId: string, body: { values: Record<string, string>; inputs: Record<string, string[]>; models?: Record<string, string> }) =>
    request<RecipeRun>("POST", `/api/recipes/run/${encodeURI(recipeId)}`, body),
  recipeRuns: (limit = 30) => request<{ runs: RecipeRun[] }>("GET", withQuery("/api/recipes/runs", { limit })),
  recipeRun: (runId: string) => request<RecipeRun>("GET", `/api/recipes/runs/${id(runId)}`),
  cancelRun: (runId: string) => request<RecipeRun>("POST", `/api/recipes/runs/${id(runId)}/cancel`),

  characters: () => request<{ characters: Character[] }>("GET", "/api/characters"),
  createCharacter: (body: { name: string; description?: string; asset_ids: string[] }) => request<Character>("POST", "/api/characters", body),
  updateCharacter: (characterId: string, patch: { name?: string; description?: string; asset_ids?: string[] }) =>
    request<Character>("PATCH", `/api/characters/${id(characterId)}`, patch),
  deleteCharacter: (characterId: string) => request<void>("DELETE", `/api/characters/${id(characterId)}`),

  collections: () => request<{ collections: Collection[] }>("GET", "/api/collections"),
  createCollection: (name: string) => request<Collection>("POST", "/api/collections", { name }),
  renameCollection: (collectionId: string, name: string) => request<void>("PATCH", `/api/collections/${id(collectionId)}`, { name }),
  deleteCollection: (collectionId: string) => request<void>("DELETE", `/api/collections/${id(collectionId)}`),
  collectionAssets: (collectionId: string, assetIds: string[], remove = false) => request<void>(remove ? "DELETE" : "POST", `/api/collections/${id(collectionId)}/assets`, { asset_ids: assetIds }),
  tags: () => request<{ tags: string[] }>("GET", "/api/tags"),
  assetTags: (assetId: string) => request<{ tags: string[] }>("GET", `/api/assets/${id(assetId)}/tags`),
  tagAssets: (assetIds: string[], tag: string, remove = false) => request<void>("POST", "/api/tags", { asset_ids: assetIds, tag, remove }),
  bulkAssets: (assetIds: string[], action: "favorite" | "unfavorite" | "trash") => request<void>("POST", "/api/assets/bulk", { asset_ids: assetIds, action }),
  spend: () => request<Spend>("GET", "/api/spend"),
  setBudgets: (daily_usd: number | null, monthly_usd: number | null) => request<Spend>("PUT", "/api/spend/budgets", { daily_usd, monthly_usd }),

  billing: () => request<Billing>("GET", "/api/billing"),
  checkout: (amountCents: number, requestId: string) => request<{ url: string }>("POST", "/api/billing/checkout", { amount_cents: amountCents, request_id: requestId }),
};

/**
 * Uploads files as multipart `file` fields. Uses XHR because fetch has no upload progress,
 * and the UI shows real progress rather than a guess. Refused files come back in `errors`.
 */
export function uploadFiles(files: File[], onProgress?: (fraction: number) => void, signal?: AbortSignal): Promise<{ assets: Asset[]; errors: FileError[] }> {
  return new Promise((resolve, reject) => {
    const form = new FormData();
    for (const file of files) form.append("file", file, file.name);
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "/api/assets");
    xhr.responseType = "text";
    xhr.withCredentials = true;
    xhr.upload.onprogress = (event) => {
      if (event.lengthComputable && onProgress) onProgress(event.loaded / event.total);
    };
    xhr.onload = () => {
      let data: unknown = null;
      try {
        data = xhr.responseText ? JSON.parse(xhr.responseText) : null;
      } catch {
        data = null;
      }
      if (xhr.status === 401) unauthorized.forEach((listener) => listener());
      if (xhr.status >= 200 && xhr.status < 300) {
        const body = (data ?? {}) as { assets?: Asset[]; errors?: FileError[] };
        resolve({ assets: body.assets ?? [], errors: body.errors ?? [] });
      } else reject(errorFrom(xhr.status, data));
    };
    xhr.onerror = () => reject(new ApiError(OFFLINE_MESSAGE, 0, "network"));
    xhr.onabort = () => reject(new DOMException("Upload canceled", "AbortError"));
    signal?.addEventListener("abort", () => xhr.abort(), { once: true });
    xhr.send(form);
  });
}

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  if (error instanceof DOMException && error.name === "AbortError") return "Canceled.";
  if (error instanceof Error && error.message) return error.message;
  return "Something went wrong. Please try again.";
}

export function isAbort(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}

/** Downloads a bounded server-streamed ZIP. */
export async function downloadZip(assetIds: string[]): Promise<void> {
  const response = await fetch("/api/assets/download", {
    method: "POST", credentials: "same-origin",
    headers: { "Content-Type": "application/json" }, body: JSON.stringify({ asset_ids: assetIds }),
  });
  if (response.status === 401) unauthorized.forEach((listener) => listener());
  if (!response.ok) throw errorFrom(response.status, await response.json().catch(() => null));
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = "pictesa-library.zip";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}
