import { createStore } from "./store";
import type { Asset, Job, RecipeRun } from "./types";

// One EventSource for the whole app (GET /api/events). Lists and the job tray subscribe here.

export type ConnectionState = "idle" | "connecting" | "open" | "reconnecting";

export const connection = createStore<ConnectionState>("idle");

interface Handlers {
  job?: (job: Job) => void;
  asset?: (asset: Asset) => void;
  recipe?: (run: RecipeRun) => void;
  /** Local only: an asset deleted permanently. */
  removed?: (id: string) => void;
}

const handlers = new Set<Handlers>();
const reconnectListeners = new Set<() => void>();
let source: EventSource | null = null;
let retryTimer: ReturnType<typeof setTimeout> | undefined;
let retryDelay = 2000;

function parse<T>(event: MessageEvent): T | null {
  try {
    return JSON.parse(event.data as string) as T;
  } catch {
    return null;
  }
}

export function startEvents() {
  if (source) return;
  clearTimeout(retryTimer);
  const wasDropped = connection.get() === "reconnecting";
  connection.set(wasDropped ? "reconnecting" : "connecting");
  const stream = new EventSource("/api/events", { withCredentials: true });
  source = stream;
  let dropped = wasDropped;
  stream.addEventListener("open", () => {
    connection.set("open");
    retryDelay = 2000;
    // The stream has no replay, so anything that changed while it was down is fetched again.
    if (dropped) reconnectListeners.forEach((listener) => listener());
    dropped = false;
  });
  stream.addEventListener("error", () => {
    dropped = true;
    connection.set("reconnecting");
    if (stream.readyState === EventSource.CLOSED) {
      // The browser gave up (server down or a non-stream answer). Try again with backoff.
      source = null;
      retryTimer = setTimeout(startEvents, retryDelay);
      retryDelay = Math.min(retryDelay * 2, 30000);
    }
  });
  stream.addEventListener("job", (event) => {
    const job = parse<Job>(event as MessageEvent);
    if (job) handlers.forEach((handler) => handler.job?.(job));
  });
  stream.addEventListener("asset", (event) => {
    const asset = parse<Asset>(event as MessageEvent);
    if (asset) handlers.forEach((handler) => handler.asset?.(asset));
  });
  stream.addEventListener("recipe", (event) => {
    const run = parse<RecipeRun>(event as MessageEvent);
    if (run) handlers.forEach((handler) => handler.recipe?.(run));
  });
}

export function stopEvents() {
  clearTimeout(retryTimer);
  source?.close();
  source = null;
  connection.set("idle");
}

export function subscribeEvents(handler: Handlers): () => void {
  handlers.add(handler);
  return () => handlers.delete(handler);
}

export function onReconnect(listener: () => void): () => void {
  reconnectListeners.add(listener);
  return () => reconnectListeners.delete(listener);
}

/** Lets local actions (an upload, a favorite toggle) reach every list the same way server events do. */
export function emitLocal({ job, asset, removed, recipe }: { job?: Job; asset?: Asset; removed?: string; recipe?: RecipeRun }) {
  if (job) handlers.forEach((handler) => handler.job?.(job));
  if (recipe) handlers.forEach((handler) => handler.recipe?.(recipe));
  if (asset) handlers.forEach((handler) => handler.asset?.(asset));
  if (removed) handlers.forEach((handler) => handler.removed?.(removed));
}
