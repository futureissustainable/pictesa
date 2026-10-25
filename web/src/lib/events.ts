import { createStore } from "./store";
import type { Asset, BoardChanges, Character, Job, Person, RecipeRun } from "./types";

// One EventSource for the whole app (GET /api/events). Lists and the job tray subscribe here.

export type ConnectionState = "idle" | "connecting" | "open" | "reconnecting";

export const connection = createStore<ConnectionState>("idle");

/** A newer in-progress image of a running job, at `url` (ComfyUI and other streaming backends). */
export interface PreviewEvent {
  job_id: string;
  version: number;
  url: string;
}

interface Handlers {
  lora?: () => void;
  library?: () => void;
  job?: (job: Job) => void;
  asset?: (asset: Asset) => void;
  recipe?: (run: RecipeRun) => void;
  /** A saved or changed character, or `{ id, deleted: true }`. */
  character?: (character: Character | { id: string; deleted: true }) => void;
  preview?: (preview: PreviewEvent) => void;
  /** Local only: an asset deleted permanently. */
  removed?: (id: string) => void;
  /** A change to a board this person can open. */
  board?: (changes: BoardChanges) => void;
  /** The board list changed: a board was created, renamed, shared, left or deleted. */
  boards?: (info: { board_id: string; deleted?: boolean; removed?: string }) => void;
  /** Who has a board open right now. */
  presence?: (info: { board_id: string; viewers: Person[] }) => void;
}

const handlers = new Set<Handlers>();
const reconnectListeners = new Set<() => void>();
let source: EventSource | null = null;
let retryTimer: ReturnType<typeof setTimeout> | undefined;
let retryDelay = 2000;
let lastRefetch = 0;

/** Events are not replayed, so lists refetch after a drop. The reopened stream and the server's
 * `resync` both ask for this; one refetch covers both. */
function refetchAll() {
  if (Date.now() - lastRefetch < 2000) return;
  lastRefetch = Date.now();
  reconnectListeners.forEach((listener) => listener());
}

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
    if (dropped) refetchAll();
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
  stream.addEventListener("lora", () => handlers.forEach((handler) => handler.lora?.()));
  stream.addEventListener("library", () => handlers.forEach((handler) => handler.library?.()));
  stream.addEventListener("asset", (event) => {
    const asset = parse<Asset>(event as MessageEvent);
    if (asset) handlers.forEach((handler) => handler.asset?.(asset));
  });
  stream.addEventListener("character", (event) => {
    const character = parse<Character | { id: string; deleted: true }>(event as MessageEvent);
    if (character) handlers.forEach((handler) => handler.character?.(character));
  });
  stream.addEventListener("recipe", (event) => {
    const run = parse<RecipeRun>(event as MessageEvent);
    if (run) handlers.forEach((handler) => handler.recipe?.(run));
  });
  stream.addEventListener("preview", (event) => {
    const preview = parse<PreviewEvent>(event as MessageEvent);
    if (preview) handlers.forEach((handler) => handler.preview?.(preview));
  });
  stream.addEventListener("board", (event) => {
    const changes = parse<BoardChanges>(event as MessageEvent);
    if (changes) handlers.forEach((handler) => handler.board?.(changes));
  });
  stream.addEventListener("boards", (event) => {
    const info = parse<{ board_id: string; deleted?: boolean; removed?: string }>(event as MessageEvent);
    if (info) handlers.forEach((handler) => handler.boards?.(info));
  });
  stream.addEventListener("board_presence", (event) => {
    const info = parse<{ board_id: string; viewers: Person[] }>(event as MessageEvent);
    if (info) handlers.forEach((handler) => handler.presence?.(info));
  });
  // Sent after a reconnect or when this tab fell behind and missed events.
  stream.addEventListener("resync", refetchAll);
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
