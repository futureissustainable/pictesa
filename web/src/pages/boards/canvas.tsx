import { memo, useCallback, useEffect, useId, useMemo, useReducer, useRef, useState, type Dispatch, type DragEvent, type KeyboardEvent, type PointerEvent as ReactPointerEvent, type RefObject, type SetStateAction } from "react";
import { ASSET_DRAG_TYPE, readDraggedAsset } from "../../lib/attach";
import { connect, moveNodes, newId, type BoardView } from "../../lib/boards";
import { roleLabel } from "../../lib/format";
import { useMediaQuery } from "../../lib/prefs";
import type { Asset, Board, BoardEdge, BoardNode } from "../../lib/types";
import { Icon } from "../../ui/icon";
import { AssetBody, assetHeight, NoteBody, StepBody, type NodeActions } from "./nodes";

// The board canvas: plain DOM nodes and SVG edges in one transformed layer. One finger or the
// mouse pans the background and drags nodes, two fingers pinch, the wheel pans (with Ctrl or a
// trackpad pinch it zooms), and dragging a node's handle onto another node connects them.

export interface Viewport {
  x: number;
  y: number;
  z: number;
}

export interface Point {
  x: number;
  y: number;
}

export const MIN_ZOOM = 0.2;
export const MAX_ZOOM = 2.5;
/** Below this zoom, node buttons, handles and note fields are too small to hit (a phone snaps a
 * tap to the nearest control, which could start a run), so they hide or stop taking taps: a tap
 * selects, and the panel has full-size controls. */
const FAR = 0.5;
/** Fingers need bigger targets: on touch screens node controls show from 90% zoom. */
const FAR_TOUCH = 0.9;
const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/** The board point under a point of the canvas element. */
export const toBoard = (view: Viewport, sx: number, sy: number): Point => ({ x: (sx - view.x) / view.z, y: (sy - view.y) / view.z });

export function zoomAround(view: Viewport, factor: number, sx: number, sy: number): Viewport {
  const z = clamp(view.z * factor, MIN_ZOOM, MAX_ZOOM);
  const k = z / view.z;
  return { z, x: sx - (sx - view.x) * k, y: sy - (sy - view.y) * k };
}

/** Heights the canvas measured, with estimates for nodes it has not drawn yet. */
export function nodeHeight(node: BoardNode, measured: Map<string, number>, asset?: Asset): number {
  return measured.get(node.id) ?? (node.kind === "asset" ? assetHeight(node.w, asset) : node.kind === "note" ? 140 : 190);
}

/** A view that shows every node with some room, never above 100%. With `floor`, it never goes
 * below that zoom: the content's top left corner shows instead (a phone stays readable). */
export function fitView(board: Board, nodes: BoardNode[], measured: Map<string, number>, width: number, height: number, floor = MIN_ZOOM): Viewport {
  if (!nodes.length || !width || !height) return { x: 48, y: 48, z: 1 };
  const left = Math.min(...nodes.map((n) => n.x));
  const top = Math.min(...nodes.map((n) => n.y));
  const right = Math.max(...nodes.map((n) => n.x + n.w));
  const bottom = Math.max(...nodes.map((n) => n.y + nodeHeight(n, measured, n.asset_id ? board.assets[n.asset_id] : undefined)));
  const fits = Math.min((width - 64) / Math.max(1, right - left), (height - 64) / Math.max(1, bottom - top), 1);
  if (fits < floor) return { z: floor, x: 24 - left * floor, y: 24 - top * floor };
  const z = clamp(fits, MIN_ZOOM, MAX_ZOOM);
  return { z, x: (width - (right - left) * z) / 2 - left * z, y: (height - (bottom - top) * z) / 2 - top * z };
}

type Gesture =
  | { kind: "pan"; pointer: number; startX: number; startY: number; origin: Viewport; moved: boolean }
  | { kind: "pinch"; distance: number; mid: Point; origin: Viewport }
  | { kind: "drag"; pointer: number; startX: number; startY: number; origins: { id: string; x: number; y: number }[]; moved: boolean; key: string }
  | { kind: "connect"; pointer: number; from: string; startX: number; startY: number; moved: boolean };

/** True while the person types in a field, where keys and paste are theirs, not the board's. */
export const typing = (target: EventTarget | null) => target instanceof HTMLElement && (["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) || target.isContentEditable);

function curve(a: Point, b: Point): string {
  const dx = Math.max(40, Math.abs(b.x - a.x) / 2);
  return `M ${a.x} ${a.y} C ${a.x + dx} ${a.y}, ${b.x - dx} ${b.y}, ${b.x} ${b.y}`;
}

const middle = (a: Point, b: Point): Point => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });

export interface CanvasProps {
  board: Board;
  view: BoardView;
  editable: boolean;
  viewport: Viewport;
  setViewport: Dispatch<SetStateAction<Viewport>>;
  canvasRef: RefObject<HTMLDivElement | null>;
  /** Heights of the drawn nodes, for edges and fitting. */
  measured: Map<string, number>;
  selected: ReadonlySet<string>;
  selectedEdge: string | null;
  onSelect(ids: string[], mode: "replace" | "toggle"): void;
  onSelectEdge(id: string | null): void;
  connecting: string | null;
  onConnecting(id: string | null): void;
  /** A handle dropped on empty board: start a step there that takes this node. */
  onConnectToEmpty(from: string, at: Point): void;
  onDropAssets(assets: Asset[], at: Point): void;
  onDropFiles(files: File[], at: Point): void;
  onDelete(): void;
  onFit(): void;
  actions: NodeActions;
  titleOf(node: BoardNode): string;
  /** The name of a step's model, and whether it takes a prompt. */
  modelOf(id?: string): { name: string; prompt: boolean };
}

export function Canvas(props: CanvasProps) {
  const { board, view, editable, viewport, setViewport, canvasRef, measured, selected, selectedEdge, connecting } = props;
  const hintId = useId();
  const [, remeasured] = useReducer((count: number) => count + 1, 0);
  const [link, setLink] = useState<{ from: string; to: Point } | null>(null);
  const [dropping, setDropping] = useState(false);
  const gesture = useRef<Gesture | null>(null);
  const pointers = useRef(new Map<number, Point>());
  // Pointer handlers read the latest values without being rebuilt on every change.
  const latest = useRef(props);
  latest.current = props;

  const nodesById = useMemo(() => new Map(view.nodes.map((node) => [node.id, node])), [view.nodes]);
  // Inputs per step, rebuilt only when edges or kinds change (not on every drag move), so the
  // memoized nodes keep their props.
  const kinds = view.nodes.map((node) => node.kind[0] + node.id).join();
  const stepInputs = useMemo(() => {
    const kindOf = new Map(view.nodes.map((node) => [node.id, node.kind]));
    const map = new Map<string, BoardEdge[]>();
    for (const edge of view.edges) {
      const source = kindOf.get(edge.from);
      if (kindOf.get(edge.to) === "step" && source && source !== "note") map.set(edge.to, [...(map.get(edge.to) ?? []), edge]);
    }
    return map;
  }, [view.edges, kinds]);
  const focusNode = useCallback((id: string) => {
    if (!latest.current.selected.has(id) && !latest.current.connecting) latest.current.onSelect([id], "replace");
  }, []);

  // One observer measures every node: the edges and the fit need their heights.
  const observer = useMemo(
    () =>
      new ResizeObserver((entries) => {
        let changed = false;
        for (const entry of entries) {
          const element = entry.target as HTMLElement;
          const id = element.dataset.node;
          if (!id) continue;
          const height = element.offsetHeight;
          if (measured.get(id) !== height) {
            measured.set(id, height);
            changed = true;
          }
        }
        if (changed) remeasured();
      }),
    [measured],
  );
  useEffect(() => () => observer.disconnect(), [observer]);
  const observe = useCallback(
    (element: HTMLDivElement | null) => {
      if (element) observer.observe(element);
    },
    [observer],
  );

  // The wheel pans; with Ctrl, or a trackpad pinch, it zooms around the pointer.
  useEffect(() => {
    const element = canvasRef.current;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey && (event.target as Element).closest("textarea")) return;
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      if (event.ctrlKey || event.metaKey) setViewport((current) => zoomAround(current, Math.exp(-event.deltaY * 0.01), event.clientX - rect.left, event.clientY - rect.top));
      else setViewport((current) => ({ ...current, x: current.x - event.deltaX, y: current.y - event.deltaY }));
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, [canvasRef, setViewport]);

  const local = (clientX: number, clientY: number): Point => {
    const rect = canvasRef.current!.getBoundingClientRect();
    return { x: clientX - rect.left, y: clientY - rect.top };
  };

  const anchor = (node: BoardNode, side: "in" | "out"): Point => {
    const height = nodeHeight(node, measured, node.asset_id ? board.assets[node.asset_id] : undefined);
    return { x: side === "out" ? node.x + node.w : node.x, y: node.y + Math.min(height / 2, 120) };
  };

  /** Connects and selects the target, whose settings then list the new input. */
  const join = (from: string, to: string) => {
    connect(from, to);
    latest.current.onSelect([to], "replace");
  };
  const finishConnect = (to: string) => {
    const from = latest.current.connecting;
    latest.current.onConnecting(null);
    if (from && from !== to) join(from, to);
  };

  const pointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    const { editable: canEdit, selected: chosen, viewport: current } = latest.current;
    const target = event.target as Element;
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (pointers.current.size === 2) {
      const [a, b] = [...pointers.current.values()] as [Point, Point];
      gesture.current = { kind: "pinch", distance: Math.hypot(a.x - b.x, a.y - b.y), mid: local((a.x + b.x) / 2, (a.y + b.y) / 2), origin: current };
      canvasRef.current?.setPointerCapture(event.pointerId);
      return;
    }
    const edge = target.closest<SVGElement>("[data-edge]");
    if (edge) {
      latest.current.onSelectEdge(edge.dataset.edge ?? null);
      return;
    }
    const handle = target.closest<HTMLElement>("[data-handle]");
    if (handle && canEdit) {
      event.preventDefault();
      gesture.current = { kind: "connect", pointer: event.pointerId, from: handle.dataset.handle!, startX: event.clientX, startY: event.clientY, moved: false };
      canvasRef.current?.setPointerCapture(event.pointerId);
      return;
    }
    const element = target.closest<HTMLElement>("[data-node]");
    if (element) {
      const id = element.dataset.node!;
      if (target.closest("button, a, input, textarea, select") && !target.closest("[data-drag]")) return;
      if (latest.current.connecting) return finishConnect(id);
      if (event.shiftKey || event.metaKey || event.ctrlKey) return latest.current.onSelect([id], "toggle");
      if (!chosen.has(id)) latest.current.onSelect([id], "replace");
      if (!canEdit) return;
      const ids = chosen.has(id) ? [...chosen] : [id];
      const origins = ids.flatMap((nodeId) => {
        const node = nodesById.get(nodeId);
        return node ? [{ id: nodeId, x: node.x, y: node.y }] : [];
      });
      gesture.current = { kind: "drag", pointer: event.pointerId, startX: event.clientX, startY: event.clientY, origins, moved: false, key: newId() };
      canvasRef.current?.setPointerCapture(event.pointerId);
      return;
    }
    gesture.current = { kind: "pan", pointer: event.pointerId, startX: event.clientX, startY: event.clientY, origin: current, moved: false };
    canvasRef.current?.setPointerCapture(event.pointerId);
  };

  const pointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!pointers.current.has(event.pointerId)) return;
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    const g = gesture.current;
    if (!g) return;
    if (g.kind === "pinch") {
      if (pointers.current.size < 2) return;
      const [a, b] = [...pointers.current.values()] as [Point, Point];
      const mid = local((a.x + b.x) / 2, (a.y + b.y) / 2);
      const zoomed = zoomAround(g.origin, Math.hypot(a.x - b.x, a.y - b.y) / Math.max(1, g.distance), g.mid.x, g.mid.y);
      setViewport({ ...zoomed, x: zoomed.x + mid.x - g.mid.x, y: zoomed.y + mid.y - g.mid.y });
      return;
    }
    if (event.pointerId !== g.pointer) return;
    const dx = event.clientX - g.startX;
    const dy = event.clientY - g.startY;
    if (!g.moved && Math.hypot(dx, dy) < 4) return;
    g.moved = true;
    if (g.kind === "pan") setViewport({ ...g.origin, x: g.origin.x + dx, y: g.origin.y + dy });
    else if (g.kind === "drag") {
      const z = latest.current.viewport.z;
      moveNodes(g.origins.map((origin) => ({ id: origin.id, x: origin.x + dx / z, y: origin.y + dy / z })), g.key);
    } else {
      const point = local(event.clientX, event.clientY);
      setLink({ from: g.from, to: toBoard(latest.current.viewport, point.x, point.y) });
    }
  };

  const pointerUp = (event: ReactPointerEvent<HTMLDivElement>, canceled = false) => {
    pointers.current.delete(event.pointerId);
    const g = gesture.current;
    if (!g) return;
    if (g.kind === "pinch") {
      if (pointers.current.size < 2) gesture.current = null;
      return;
    }
    if (event.pointerId !== g.pointer) return;
    gesture.current = null;
    if (canceled) return setLink(null);
    if (g.kind === "pan" && !g.moved) {
      latest.current.onSelect([], "replace");
      latest.current.onSelectEdge(null);
      latest.current.onConnecting(null);
    } else if (g.kind === "connect") {
      setLink(null);
      // A click on a handle: the next item chosen is the target.
      if (!g.moved) return latest.current.onConnecting(g.from);
      const element = document.elementFromPoint(event.clientX, event.clientY);
      const to = element?.closest<HTMLElement>("[data-node]")?.dataset.node;
      if (to && to !== g.from) join(g.from, to);
      else if (!to && element && canvasRef.current?.contains(element)) {
        const point = local(event.clientX, event.clientY);
        latest.current.onConnectToEmpty(g.from, toBoard(latest.current.viewport, point.x, point.y));
      }
    }
  };

  const keyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (typing(event.target) || event.altKey || event.metaKey || event.ctrlKey) return;
    const { selected: chosen, editable: canEdit } = latest.current;
    const focused = (event.target as Element).closest<HTMLElement>("[data-node]")?.dataset.node;
    const step = event.shiftKey ? 64 : 16;
    const arrows: Record<string, [number, number]> = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    const arrow = arrows[event.key];
    if (arrow) {
      event.preventDefault();
      if (chosen.size && canEdit && focused) {
        const moves = [...chosen].flatMap((id) => {
          const node = nodesById.get(id);
          return node ? [{ id, x: node.x + arrow[0] * step, y: node.y + arrow[1] * step }] : [];
        });
        moveNodes(moves);
      } else setViewport((current) => ({ ...current, x: current.x - arrow[0] * step * 3, y: current.y - arrow[1] * step * 3 }));
    } else if ((event.key === "Delete" || event.key === "Backspace") && canEdit) {
      event.preventDefault();
      latest.current.onDelete();
    } else if (event.key === "Escape") {
      latest.current.onConnecting(null);
      latest.current.onSelect([], "replace");
      latest.current.onSelectEdge(null);
    } else if (event.key === "Enter" && focused && latest.current.connecting) {
      event.preventDefault();
      finishConnect(focused);
    } else if (event.key === "+" || event.key === "=" || event.key === "-") {
      const rect = canvasRef.current!.getBoundingClientRect();
      setViewport((current) => zoomAround(current, event.key === "-" ? 0.8 : 1.25, rect.width / 2, rect.height / 2));
    } else if (event.key === "0") latest.current.onFit();
  };

  const dropPoint = (event: DragEvent) => {
    const point = local(event.clientX, event.clientY);
    return toBoard(latest.current.viewport, point.x, point.y);
  };
  const accepts = (event: DragEvent) => editable && (event.dataTransfer.types.includes("Files") || event.dataTransfer.types.includes(ASSET_DRAG_TYPE));

  const touch = useMediaQuery("(pointer: coarse)");
  const feedsSteps = (node: BoardNode) => view.edges.some((edge) => edge.from === node.id && nodesById.get(edge.to)?.kind === "step");
  const linkFrom = link && nodesById.get(link.from);

  return (
    <div
      ref={canvasRef}
      className={`board-canvas ${connecting ? "connecting" : ""} ${dropping ? "drop-over" : ""} ${viewport.z < (touch ? FAR_TOUCH : FAR) ? "far" : ""}`}
      style={{ backgroundSize: `${24 * viewport.z}px ${24 * viewport.z}px`, backgroundPosition: `${viewport.x}px ${viewport.y}px` }}
      tabIndex={0}
      role="region"
      aria-label="Board canvas"
      aria-describedby={hintId}
      onPointerDown={pointerDown}
      onPointerMove={pointerMove}
      onPointerUp={(event) => pointerUp(event)}
      onPointerCancel={(event) => pointerUp(event, true)}
      onClick={(event) => {
        // A handle pressed with the keyboard: the next item chosen is the target.
        const handle = (event.target as Element).closest<HTMLElement>("[data-handle]");
        if (handle && event.detail === 0) latest.current.onConnecting(handle.dataset.handle!);
      }}
      onKeyDown={keyDown}
      onDragOver={(event) => {
        if (!accepts(event)) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
        setDropping(true);
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropping(false);
      }}
      onDrop={(event) => {
        setDropping(false);
        if (!accepts(event)) return;
        event.preventDefault();
        const asset = readDraggedAsset(event.dataTransfer);
        if (asset) props.onDropAssets([asset], dropPoint(event));
        else if (event.dataTransfer.files.length) props.onDropFiles([...event.dataTransfer.files], dropPoint(event));
      }}
    >
      <p id={hintId} className="sr-only">
        Arrow keys move the view, or the selected items when one has focus. Plus and minus zoom, 0 fits the board. Tab moves through the items.
      </p>
      <div className="board-world" style={{ transform: `translate(${viewport.x}px, ${viewport.y}px) scale(${viewport.z})` }}>
        <svg className="board-edges" aria-hidden="true">
          {view.edges.map((edge) => {
            const from = nodesById.get(edge.from);
            const to = nodesById.get(edge.to);
            if (!from || !to) return null;
            const d = curve(anchor(from, "out"), anchor(to, "in"));
            const input = to.kind === "step" && from.kind !== "note";
            return (
              <g key={edge.id}>
                <path className="board-edge-hit" d={d} data-edge={edge.id} />
                <path className={`board-edge ${input ? "input" : ""} ${selectedEdge === edge.id ? "selected" : ""}`} d={d} />
              </g>
            );
          })}
          {link && linkFrom && <path className="board-edge drafting" d={curve(anchor(linkFrom, "out"), link.to)} />}
        </svg>
        {view.edges.map((edge) => (edge.role ? <EdgeLabel key={edge.id} edge={edge} from={nodesById.get(edge.from)} to={nodesById.get(edge.to)} anchor={anchor} /> : null))}
        {view.nodes.map((node) => (
          <NodeView
            key={node.id}
            node={node}
            asset={node.asset_id ? board.assets[node.asset_id] : undefined}
            run={board.runs[node.id]}
            inputs={stepInputs.get(node.id) ?? NO_EDGES}
            feedsSteps={node.kind === "step" && feedsSteps(node)}
            selected={selected.has(node.id)}
            target={Boolean(connecting) && connecting !== node.id}
            editable={editable}
            title={props.titleOf(node)}
            modelName={node.kind === "step" ? props.modelOf(node.model).name : ""}
            takesPrompt={node.kind !== "step" || props.modelOf(node.model).prompt}
            actions={props.actions}
            observe={observe}
            onFocusNode={focusNode}
          />
        ))}
      </div>
      {!view.nodes.length && (
        <div className="board-empty">
          <Icon name="schema" />
          <p>{editable ? "This board is empty. Add files from the library or drop them here, or add a note or a step." : "This board is empty."}</p>
        </div>
      )}
      {connecting && (
        <p className="board-connect-hint" role="status">
          Choose an item to connect to. Press Escape to stop.
        </p>
      )}
    </div>
  );
}

const NO_EDGES: BoardEdge[] = [];

function EdgeLabel({ edge, from, to, anchor }: { edge: BoardEdge; from?: BoardNode; to?: BoardNode; anchor(node: BoardNode, side: "in" | "out"): Point }) {
  if (!from || !to || !edge.role) return null;
  const at = middle(anchor(from, "out"), anchor(to, "in"));
  return (
    <span className="board-edge-label" style={{ left: at.x, top: at.y }} aria-hidden="true">
      {roleLabel[edge.role]}
    </span>
  );
}

interface NodeViewProps {
  node: BoardNode;
  asset?: Asset;
  run?: Board["runs"][string];
  inputs: BoardEdge[];
  feedsSteps: boolean;
  selected: boolean;
  /** Connect mode: this node can be the target. */
  target: boolean;
  editable: boolean;
  title: string;
  modelName: string;
  takesPrompt: boolean;
  actions: NodeActions;
  observe(element: HTMLDivElement | null): void;
  onFocusNode(id: string): void;
}

const NodeView = memo(function NodeView({ node, asset, run, inputs, feedsSteps, selected, target, editable, title, modelName, takesPrompt, actions, observe, onFocusNode }: NodeViewProps) {
  return (
    <div
      ref={observe}
      data-node={node.id}
      className={`board-node board-node-${node.kind} ${selected ? "selected" : ""} ${target ? "target" : ""}`}
      style={{ left: node.x, top: node.y, width: node.w }}
      tabIndex={0}
      role="group"
      aria-label={title}
      onFocus={(event) => {
        if (event.target === event.currentTarget) onFocusNode(node.id);
      }}
      onDoubleClick={() => asset && actions.open(asset)}
    >
      {node.kind === "asset" && <AssetBody node={node} asset={asset} title={title} />}
      {node.kind === "note" && <NoteBody node={node} editable={editable} actions={actions} />}
      {node.kind === "step" && <StepBody node={node} run={run} inputs={inputs} feedsSteps={feedsSteps} modelName={modelName} takesPrompt={takesPrompt} editable={editable} actions={actions} />}
      {editable && <button type="button" className="board-handle" data-handle={node.id} aria-label={`Connect ${title} to another item`} title="Drag to connect" />}
    </div>
  );
});
