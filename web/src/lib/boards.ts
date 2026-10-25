import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, errorMessage } from "./api";
import { onReconnect, subscribeEvents } from "./events";
import { notify, notifyError } from "./notices";
import { createStore, useStore } from "./store";
import type { Board, BoardChanges, BoardEdge, BoardNode, BoardOp, InputRole, NodeFields, NodeKind, Person } from "./types";

// The open board: the server's state, kept current by `board` events, plus this tab's changes
// that the server has not answered yet, shown at once. Changes go out one request at a time.
// Each change names the revision of the item it was based on; when someone else changed that
// item first, the server refuses it and this tab shows their version (docs/API.md, Boards).

/** A change waiting for its answer. Ops on items this tab is still changing take their base
 * revision from the answer to the earlier change, so a quick series of edits never conflicts
 * with itself. */
interface Draft {
  op: BoardOp;
  /** "n:<id>" or "e:<id>": the item the op adds or changes. */
  target?: string;
  /** The base comes from this tab's previous change to the same item. */
  chained: boolean;
}

interface Batch {
  ops: Draft[];
  /** A newer batch with the same key replaces this one while it waits (a drag). */
  key?: string;
  sent: boolean;
}

export interface OpenBoard {
  id: string;
  /** The server's state. */
  board: Board | null;
  status: "loading" | "ready" | "gone" | "error";
  error: string | null;
  viewers: Person[];
  pending: Batch[];
}

export interface BoardView {
  nodes: BoardNode[];
  edges: BoardEdge[];
  name: string;
}

const closed: OpenBoard = { id: "", board: null, status: "loading", error: null, viewers: [], pending: [] };
export const openBoard = createStore<OpenBoard>(closed);

/** Removed items and the version that removed them, so a late, older event cannot bring one back. */
let tombs = new Map<string, number>();
/** The revision the server gave this tab's latest change to each item. */
let ownRev = new Map<string, number>();
let nameVersion = 0;
let sending = false;

/** Ids for new nodes and edges: 1 to 64 letters, digits, - or _. Works without a secure context. */
export function newId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(12));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export const nodeKey = (id: string) => `n:${id}`;
export const edgeKey = (id: string) => `e:${id}`;

export const DEFAULT_WIDTH: Record<NodeKind, number> = { asset: 240, note: 240, step: 300 };

function upsert<T extends { id: string; rev: number }>(list: T[], incoming: T[] | undefined, key: (id: string) => string): T[] {
  if (!incoming?.length) return list;
  const next = [...list];
  for (const item of incoming) {
    if ((tombs.get(key(item.id)) ?? -1) >= item.rev) continue;
    const index = next.findIndex((existing) => existing.id === item.id);
    if (index === -1) next.push(item);
    else if (item.rev >= next[index]!.rev) next[index] = item;
  }
  return next;
}

function drop<T extends { id: string }>(list: T[], ids: string[] | undefined, key: (id: string) => string, version: number): T[] {
  if (!ids?.length) return list;
  for (const id of ids) tombs.set(key(id), Math.max(version, tombs.get(key(id)) ?? 0));
  return list.filter((item) => !ids.includes(item.id));
}

/** Folds a change into the server state. Items carry revisions, so order of arrival does not matter. */
export function mergeChanges(board: Board, changes: BoardChanges): Board {
  const nodes = drop(upsert(board.nodes, changes.nodes, nodeKey), changes.removed_nodes, nodeKey, changes.version);
  const edges = drop(upsert(board.edges, changes.edges, edgeKey), changes.removed_edges, edgeKey, changes.version);
  const runs = { ...board.runs };
  for (const [id, run] of Object.entries(changes.runs ?? {})) {
    if (!runs[id] || run.rev >= runs[id]!.rev) runs[id] = run;
  }
  for (const id of changes.removed_nodes ?? []) delete runs[id];
  let name = board.name;
  if (changes.name && changes.version >= nameVersion) {
    name = changes.name;
    nameVersion = changes.version;
  }
  return { ...board, name, nodes, edges, runs, assets: { ...board.assets, ...changes.assets }, version: Math.max(board.version, changes.version) };
}

function applyLocal(view: BoardView, op: BoardOp): BoardView {
  const { nodes, edges } = view;
  switch (op.op) {
    case "add_node":
      return { ...view, nodes: [...nodes, { w: DEFAULT_WIDTH[op.node.kind], rev: 0, ...op.node }] };
    case "move_node":
      return { ...view, nodes: nodes.map((node) => (node.id === op.id ? { ...node, x: op.x, y: op.y } : node)) };
    case "update_node": {
      const { op: _op, id, base: _base, ...fields } = op;
      return { ...view, nodes: nodes.map((node) => (node.id === id ? { ...node, ...fields } : node)) };
    }
    case "delete_node":
      return { ...view, nodes: nodes.filter((node) => node.id !== op.id), edges: edges.filter((edge) => edge.from !== op.id && edge.to !== op.id) };
    case "add_edge":
      return { ...view, edges: [...edges, { role: null, rev: 0, ...op.edge }] };
    case "update_edge":
      return { ...view, edges: edges.map((edge) => (edge.id === op.id ? { ...edge, role: op.role } : edge)) };
    case "delete_edge":
      return { ...view, edges: edges.filter((edge) => edge.id !== op.id) };
    case "rename":
      return { ...view, name: op.name };
  }
}

/** What this tab shows: the server's state with its unanswered changes on top. */
export function viewOf(board: Board, pending: Batch[]): BoardView {
  let view: BoardView = { nodes: board.nodes, edges: board.edges, name: board.name };
  for (const batch of pending) for (const draft of batch.ops) view = applyLocal(view, draft.op);
  return view;
}

function confirmedRev(board: Board | null, target: string): number {
  const id = target.slice(2);
  const item = target.startsWith("n:") ? board?.nodes.find((node) => node.id === id) : board?.edges.find((edge) => edge.id === id);
  return item?.rev ?? -1;
}

/** An op on an existing item, based on the revision this tab knows (or `seen`, the revision the
 * person was looking at when they started the change). */
function based(state: OpenBoard, target: string, op: BoardOp, seen?: number): Draft {
  const chained = state.pending.some((batch) => batch.ops.some((draft) => draft.target === target));
  if (chained) return { op, target, chained };
  return { op: { ...op, base: seen ?? confirmedRev(state.board, target) } as BoardOp, target, chained: false };
}

function queue(make: (state: OpenBoard) => Draft[], key?: string) {
  const state = openBoard.get();
  if (state.status !== "ready" || !state.board) return;
  const ops = make(state);
  if (!ops.length) return;
  const last = state.pending[state.pending.length - 1];
  let pending: Batch[];
  if (key && last && !last.sent && last.key === key) {
    // A newer position of the same drag: keep the waiting batch's bases, take the new values.
    const merged = ops.map((draft) => {
      const old = last.ops.find((item) => item.target === draft.target);
      return old ? { ...draft, chained: old.chained, op: "base" in old.op ? ({ ...draft.op, base: old.op.base } as BoardOp) : draft.op } : draft;
    });
    pending = [...state.pending.slice(0, -1), { ...last, ops: merged }];
  } else {
    pending = [...state.pending, { ops, key, sent: false }];
  }
  openBoard.set({ ...state, pending });
  void flush();
}

const withoutBatch = (pending: Batch[], batch: Batch, refused?: Set<string>) =>
  pending
    .filter((item) => item !== batch)
    .map((item) => (refused ? { ...item, ops: item.ops.filter((draft) => !(draft.chained && draft.target && refused.has(draft.target))) } : item))
    .filter((item) => item.ops.length);

async function flush() {
  if (sending) return;
  const state = openBoard.get();
  const batch = state.pending.find((item) => !item.sent);
  if (!batch || state.status !== "ready") return;
  const ops: BoardOp[] = [];
  for (const draft of batch.ops) {
    if (!draft.chained) ops.push(draft.op);
    else if (draft.target && ownRev.has(draft.target)) ops.push({ ...draft.op, base: ownRev.get(draft.target)! } as BoardOp);
  }
  const targets = new Set(batch.ops.flatMap((draft) => (draft.target ? [draft.target] : [])));
  if (!ops.length) {
    // Every op followed a change of this tab that was refused: they go with it.
    openBoard.set({ ...state, pending: withoutBatch(state.pending, batch, targets) });
    return void flush();
  }
  const sent: Batch = { ...batch, sent: true };
  openBoard.set({ ...state, pending: state.pending.map((item) => (item === batch ? sent : item)) });
  sending = true;
  const id = state.id;
  try {
    const changes = await api.boardOps(id, ops);
    if (openBoard.get().id !== id) return;
    for (const node of changes.nodes ?? []) ownRev.set(nodeKey(node.id), node.rev);
    for (const edge of changes.edges ?? []) ownRev.set(edgeKey(edge.id), edge.rev);
    openBoard.set((current) => ({ ...current, board: current.board && mergeChanges(current.board, changes), pending: withoutBatch(current.pending, sent) }));
  } catch (error) {
    if (openBoard.get().id !== id) return;
    targets.forEach((target) => ownRev.delete(target));
    const current = error instanceof ApiError && error.status === 409 ? (error.body as { current?: BoardChanges } | null)?.current : undefined;
    openBoard.set((now) => ({ ...now, board: now.board && current ? mergeChanges(now.board, current) : now.board, pending: withoutBatch(now.pending, sent, targets) }));
    if (error instanceof ApiError && error.status === 409) notify(error.message);
    else {
      notifyError(errorMessage(error));
      // The answer was lost: the change may or may not have landed.
      if (error instanceof ApiError && error.status === 0) void refresh();
    }
  } finally {
    sending = false;
    void flush();
  }
}

/** Resolves once every change of this tab has an answer. */
export function whenSaved(): Promise<void> {
  return new Promise((resolve) => {
    let stop = () => {};
    const check = () => {
      if (openBoard.get().pending.length) return;
      stop();
      resolve();
    };
    stop = openBoard.subscribe(check);
    check();
  });
}

// ---------- changes the UI makes ----------

export function addNodes(nodes: (Partial<BoardNode> & Pick<BoardNode, "id" | "kind" | "x" | "y">)[], edges: { from: string; to: string; role?: InputRole | null }[] = []) {
  queue(() => [
    ...nodes.map((node) => ({ op: { op: "add_node", node } as BoardOp, target: nodeKey(node.id), chained: false })),
    ...edges.map((edge) => {
      const id = newId();
      return { op: { op: "add_edge", edge: { id, ...edge } } as BoardOp, target: edgeKey(id), chained: false };
    }),
  ]);
}

/** Moves nodes; a drag sends its moves under one key so only the newest position waits. */
export function moveNodes(moves: { id: string; x: number; y: number }[], key?: string) {
  queue((state) => moves.map(({ id, x, y }) => based(state, nodeKey(id), { op: "move_node", id, base: -1, x: Math.round(x), y: Math.round(y) })), key);
}

/** `seen`: the node's revision when the person started this change (typing), so a change someone
 * else made meanwhile is not overwritten. */
export function updateNode(id: string, fields: Partial<NodeFields>, seen?: number) {
  queue((state) => [based(state, nodeKey(id), { op: "update_node", id, base: -1, ...fields }, seen)]);
}

export function deleteItems(nodeIds: string[], edgeIds: string[] = []) {
  queue((state) => {
    // Edges of deleted nodes go with them.
    const edges = edgeIds.filter((id) => {
      const edge = state.board?.edges.find((item) => item.id === id);
      return !edge || !(nodeIds.includes(edge.from) || nodeIds.includes(edge.to));
    });
    return [...edges.map((id) => based(state, edgeKey(id), { op: "delete_edge", id, base: -1 })), ...nodeIds.map((id) => based(state, nodeKey(id), { op: "delete_node", id, base: -1 }))];
  });
}

export function connect(from: string, to: string, role: InputRole | null = null) {
  addNodes([], [{ from, to, role }]);
}

export function setEdgeRole(id: string, role: InputRole | null) {
  queue((state) => [based(state, edgeKey(id), { op: "update_edge", id, base: -1, role })]);
}

export function renameBoard(name: string) {
  queue(() => [{ op: { op: "rename", name }, chained: false }]);
}

// ---------- loading and live updates ----------

async function refresh() {
  const { id } = openBoard.get();
  if (!id) return;
  try {
    const board = await api.board(id);
    if (openBoard.get().id !== id) return;
    nameVersion = board.version;
    openBoard.set((state) => ({ ...state, board, status: "ready", error: null }));
    void flush();
  } catch (error) {
    if (openBoard.get().id !== id) return;
    const gone = error instanceof ApiError && error.status === 404;
    openBoard.set((state) => ({ ...state, status: gone ? "gone" : state.board ? state.status : "error", error: gone ? "This board was deleted, or it is no longer shared with you." : errorMessage(error) }));
  }
}

function open(id: string) {
  tombs = new Map();
  ownRev = new Map();
  nameVersion = 0;
  openBoard.set({ ...closed, id });
  void refresh();
}

function close(id: string) {
  if (openBoard.get().id === id) openBoard.set(closed);
}

subscribeEvents({
  board(changes) {
    const state = openBoard.get();
    if (changes.board_id !== state.id || !state.board) return;
    if (changes.reload) return void refresh();
    openBoard.set((current) => ({ ...current, board: current.board && mergeChanges(current.board, changes) }));
  },
  boards(info) {
    const state = openBoard.get();
    if (info.board_id !== state.id) return;
    if (info.deleted) openBoard.set((current) => ({ ...current, status: "gone", error: "The owner deleted this board." }));
    else void refresh();
  },
  presence(info) {
    if (info.board_id === openBoard.get().id) openBoard.set((current) => ({ ...current, viewers: info.viewers }));
  },
});
onReconnect(() => void refresh());

/** Opens a board for this page and keeps it live; closes it when the page goes. */
export function useOpenBoard(id: string): { state: OpenBoard; view: BoardView | null } {
  useEffect(() => {
    open(id);
    return () => close(id);
  }, [id]);
  const state = useStore(openBoard);
  const view = useMemo(() => (state.board && state.id === id ? viewOf(state.board, state.pending) : null), [state.board, state.pending, state.id, id]);
  return { state, view };
}

const client = newId();

/** Hosted mode: tells the server this tab shows the board, every 20 seconds while visible. */
export function usePresence(id: string, active: boolean) {
  useEffect(() => {
    if (!active) return;
    const beat = () => {
      if (document.visibilityState !== "visible") return;
      api.boardPresence(id, client).then(
        ({ viewers }) => openBoard.set((state) => (state.id === id ? { ...state, viewers } : state)),
        () => undefined,
      );
    };
    const leave = () => {
      void fetch(`/api/boards/${encodeURIComponent(id)}/presence`, {
        method: "POST",
        keepalive: true,
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ client, leave: true }),
      }).catch(() => undefined);
    };
    beat();
    const timer = setInterval(beat, 20_000);
    document.addEventListener("visibilitychange", beat);
    window.addEventListener("pagehide", leave);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", beat);
      window.removeEventListener("pagehide", leave);
      leave();
    };
  }, [id, active]);
}

// ---------- text fields ----------

const drafts = new Set<() => void>();

/** Sends typing that has not gone out yet (before a run, so the run uses it). */
export function flushDrafts() {
  drafts.forEach((flush) => flush());
}

/** A text field on a board item: shows the typing at once, sends it a moment after typing stops,
 * on leaving the field and before a run. The change is based on the item's revision `rev` when the
 * typing started: if someone else changed the item meanwhile, the server refuses it. */
export function useTextDraft(value: string, rev: number, save: (text: string, seen: number) => void, delay = 500) {
  const [draft, setDraft] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const seen = useRef(rev);
  const latest = useRef({ draft, value, save });
  latest.current = { draft, value, save };
  const flush = useCallback(() => {
    clearTimeout(timer.current);
    const current = latest.current;
    if (current.draft === null) return;
    if (current.draft !== current.value) current.save(current.draft, seen.current);
    latest.current = { ...current, draft: null };
    setDraft(null);
  }, []);
  useEffect(() => {
    drafts.add(flush);
    return () => {
      drafts.delete(flush);
      flush();
    };
  }, [flush]);
  const change = (text: string) => {
    if (latest.current.draft === null) seen.current = rev;
    latest.current = { ...latest.current, draft: text };
    setDraft(text);
    clearTimeout(timer.current);
    timer.current = setTimeout(flush, delay);
  };
  return { text: draft ?? value, change, flush };
}
