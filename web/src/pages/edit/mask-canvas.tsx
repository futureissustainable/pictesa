import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import type { Asset, Padding } from "../../lib/types";
import { Icon } from "../../ui/icon";

// A plain <canvas> over the image: paint a mask with brush and eraser, undo and redo, and zoom and
// pan with a mouse wheel, a pen, one-finger paint and two-finger pinch.

export interface MaskHandle {
  /** The painted mask as a PNG (paint opaque, the rest transparent), or null when nothing is painted. */
  exportMask(): Promise<Blob | null>;
  hasPaint(): boolean;
}

type Stroke = { erase: boolean; size: number; points: number[] } | { clear: true };
type Tool = "brush" | "eraser" | "hand";

/** Longest side of the mask canvas. The server scales the mask to the image's full size. */
const MASK_EDGE = 2048;
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 12;
const PAINT = "#f2693d";

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

function drawStroke(ctx: CanvasRenderingContext2D, stroke: Stroke) {
  if ("clear" in stroke) {
    ctx.clearRect(0, 0, ctx.canvas.width, ctx.canvas.height);
    return;
  }
  const { points, size, erase } = stroke;
  ctx.globalCompositeOperation = erase ? "destination-out" : "source-over";
  ctx.strokeStyle = PAINT;
  ctx.fillStyle = PAINT;
  ctx.lineWidth = size;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  if (points.length === 2) {
    ctx.beginPath();
    ctx.arc(points[0]!, points[1]!, size / 2, 0, Math.PI * 2);
    ctx.fill();
  } else {
    ctx.beginPath();
    ctx.moveTo(points[0]!, points[1]!);
    for (let i = 2; i < points.length; i += 2) ctx.lineTo(points[i]!, points[i + 1]!);
    ctx.stroke();
  }
  ctx.globalCompositeOperation = "source-over";
}

/** Strokes after the last clear that still paint something. */
const painted = (strokes: Stroke[]) => {
  const last = strokes.map((stroke) => "clear" in stroke).lastIndexOf(true);
  return strokes.slice(last + 1).some((stroke) => !("clear" in stroke) && !stroke.erase);
};

interface Props {
  asset: Asset;
  /** Brush, eraser and history only when the tool needs a mask. */
  paint: boolean;
  /** Expand preview: the space added around the image. */
  frame?: Padding | null;
  onPaintChange?(painted: boolean): void;
}

export const MaskCanvas = forwardRef<MaskHandle, Props>(function MaskCanvas({ asset, paint, frame, onPaintChange }, ref) {
  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [natural, setNatural] = useState<{ w: number; h: number } | null>(asset.width && asset.height ? { w: asset.width, h: asset.height } : null);
  const [stage, setStage] = useState({ w: 0, h: 0 });
  const [view, setView] = useState({ zoom: 1, x: 0, y: 0 });
  const viewRef = useRef(view);
  viewRef.current = view;
  const [tool, setTool] = useState<Tool>("brush");
  const [brush, setBrush] = useState(40);
  const [strokes, setStrokes] = useState<Stroke[]>([]);
  const [redo, setRedo] = useState<Stroke[]>([]);
  const strokesRef = useRef(strokes);
  strokesRef.current = strokes;
  const [cursor, setCursor] = useState<{ x: number; y: number } | null>(null);
  const [spaceHeld, setSpaceHeld] = useState(false);

  // Pointer state lives in refs: it changes on every move and needs no render.
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const gesture = useRef<
    | { kind: "paint"; stroke: { erase: boolean; size: number; points: number[] } }
    | { kind: "pan"; startX: number; startY: number; viewX: number; viewY: number }
    | { kind: "pinch"; distance: number; midX: number; midY: number; zoom: number; x: number; y: number }
    | null
  >(null);

  const scale = natural ? Math.min(1, MASK_EDGE / Math.max(natural.w, natural.h)) : 1;
  const maskSize = natural ? { w: Math.max(1, Math.round(natural.w * scale)), h: Math.max(1, Math.round(natural.h * scale)) } : null;

  // Fit the image (and the expanded frame) into the stage with a small margin.
  const pad = frame ?? { left: 0, top: 0, right: 0, bottom: 0 };
  const outer = natural ? { w: natural.w + pad.left + pad.right, h: natural.h + pad.top + pad.bottom } : null;
  const fit = outer && stage.w && stage.h ? Math.min((stage.w * 0.92) / outer.w, (stage.h * 0.92) / outer.h) : 0;
  // Centre the whole frame, not just the image.
  const frameShift = { x: ((pad.left - pad.right) / 2) * fit, y: ((pad.top - pad.bottom) / 2) * fit };
  // Zoom maths works around the image centre, which sits `frameShift` away from the stage centre.
  const shiftRef = useRef(frameShift);
  shiftRef.current = frameShift;

  useEffect(() => {
    const element = stageRef.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => setStage({ w: entry!.contentRect.width, h: entry!.contentRect.height }));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const redraw = useCallback((list: Stroke[]) => {
    const ctx = canvasRef.current?.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, ctx.canvas.width, ctx.canvas.height);
    list.forEach((stroke) => drawStroke(ctx, stroke));
  }, []);

  useEffect(() => redraw(strokes), [strokes, redraw, maskSize?.w, maskSize?.h]);
  useEffect(() => onPaintChange?.(painted(strokes)), [strokes, onPaintChange]);

  useImperativeHandle(
    ref,
    () => ({
      hasPaint: () => painted(strokesRef.current),
      exportMask: () =>
        new Promise((resolve) => {
          const canvas = canvasRef.current;
          if (!canvas || !painted(strokesRef.current)) return resolve(null);
          canvas.toBlob((blob) => resolve(blob), "image/png");
        }),
    }),
    [],
  );

  const commit = (stroke: Stroke) => {
    setStrokes((list) => [...list, stroke]);
    setRedo([]);
  };
  const undo = () => {
    const list = strokesRef.current;
    if (!list.length) return;
    setRedo((items) => [list[list.length - 1]!, ...items]);
    setStrokes(list.slice(0, -1));
  };
  const redoOne = () => {
    if (!redo.length) return;
    setStrokes((list) => [...list, redo[0]!]);
    setRedo(redo.slice(1));
  };
  const clear = () => {
    if (painted(strokesRef.current)) commit({ clear: true });
  };

  /** Zooms by `factor` around a stage point (relative to the stage centre). */
  const zoomAt = (factor: number, px = 0, py = 0) => {
    setView((current) => {
      const zoom = clamp(current.zoom * factor, MIN_ZOOM, MAX_ZOOM);
      const ratio = zoom / current.zoom;
      return { zoom, x: px - (px - current.x) * ratio, y: py - (py - current.y) * ratio };
    });
  };
  const resetView = () => setView({ zoom: 1, x: 0, y: 0 });

  // A new image or frame starts fitted.
  useEffect(resetView, [asset.id, frame?.left, frame?.top, frame?.right, frame?.bottom]);

  // Keyboard: B brush, E eraser, H or Space hand, [ and ] size, Ctrl+Z undo, Ctrl+Shift+Z redo, 0 fit, + and - zoom.
  useEffect(() => {
    const typing = (target: EventTarget | null) => target instanceof HTMLElement && (["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) || target.isContentEditable);
    const down = (event: KeyboardEvent) => {
      // Not while typing, or while a dialog or popover covers the canvas.
      if (typing(event.target) || document.querySelector("dialog[open], :popover-open")) return;
      const key = event.key.toLowerCase();
      const mod = event.ctrlKey || event.metaKey;
      if (mod && key === "z" && paint) {
        event.preventDefault();
        if (event.shiftKey) redoOne();
        else undo();
      } else if (mod && key === "y" && paint) {
        event.preventDefault();
        redoOne();
      } else if (mod || event.altKey) {
        return;
      } else if (key === " ") {
        // Space still presses a focused button.
        if (event.target instanceof HTMLElement && event.target.closest("button, a, [role='button']")) return;
        event.preventDefault();
        setSpaceHeld(true);
      } else if (paint && key === "b") setTool("brush");
      else if (paint && key === "e") setTool("eraser");
      else if (key === "h") setTool("hand");
      else if (paint && key === "[") setBrush((size) => clamp(Math.round(size / 1.2), 4, 200));
      else if (paint && key === "]") setBrush((size) => clamp(Math.round(size * 1.2), 4, 200));
      else if (key === "0") resetView();
      else if (key === "+" || key === "=") zoomAt(1.25);
      else if (key === "-") zoomAt(0.8);
    };
    const up = (event: KeyboardEvent) => {
      if (event.key === " ") setSpaceHeld(false);
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  });

  // Wheel zoom needs a non-passive listener to stop the page from scrolling.
  useEffect(() => {
    const element = stageRef.current;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      const factor = Math.exp(-clamp(event.deltaY, -100, 100) * (event.ctrlKey ? 0.01 : 0.002));
      zoomAt(factor, event.clientX - rect.left - rect.width / 2 - shiftRef.current.x, event.clientY - rect.top - rect.height / 2 - shiftRef.current.y);
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, []);

  const local = (event: { clientX: number; clientY: number }) => {
    const rect = stageRef.current!.getBoundingClientRect();
    return { x: event.clientX - rect.left - rect.width / 2, y: event.clientY - rect.top - rect.height / 2 };
  };

  /** Pointer position in mask pixels. */
  const toMask = (event: { clientX: number; clientY: number }) => {
    const canvas = canvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    return [((event.clientX - rect.left) / rect.width) * canvas.width, ((event.clientY - rect.top) / rect.height) * canvas.height] as const;
  };

  const startPinch = () => {
    const [a, b] = [...pointers.current.values()];
    if (!a || !b) return;
    // A second finger turns a stroke into a pinch: drop the stroke.
    if (gesture.current?.kind === "paint") redraw(strokesRef.current);
    const { zoom, x, y } = viewRef.current;
    const shift = shiftRef.current;
    gesture.current = { kind: "pinch", distance: Math.hypot(a.x - b.x, a.y - b.y) || 1, midX: (a.x + b.x) / 2 - shift.x, midY: (a.y + b.y) / 2 - shift.y, zoom, x, y };
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerType === "mouse" && event.button !== 0 && event.button !== 1) return;
    try {
      stageRef.current?.setPointerCapture(event.pointerId);
    } catch {
      // Pointers that are no longer active (rare on touch) simply go uncaptured.
    }
    pointers.current.set(event.pointerId, local(event));
    if (pointers.current.size === 2 && event.pointerType === "touch") return startPinch();
    if (pointers.current.size > 1) return;
    const panning = !paint || tool === "hand" || spaceHeld || event.button === 1;
    if (panning || !canvasRef.current) {
      const point = local(event);
      gesture.current = { kind: "pan", startX: point.x, startY: point.y, viewX: viewRef.current.x, viewY: viewRef.current.y };
      return;
    }
    const canvas = canvasRef.current;
    const rect = canvas.getBoundingClientRect();
    const size = Math.max(1, brush * (canvas.width / rect.width));
    const [x, y] = toMask(event);
    const stroke = { erase: tool === "eraser", size, points: [x, y] };
    gesture.current = { kind: "paint", stroke };
    const ctx = canvas.getContext("2d");
    if (ctx) drawStroke(ctx, stroke);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerType !== "touch") setCursor(local(event));
    if (!pointers.current.has(event.pointerId)) return;
    pointers.current.set(event.pointerId, local(event));
    const current = gesture.current;
    if (!current) return;
    if (current.kind === "pinch") {
      const [a, b] = [...pointers.current.values()];
      if (!a || !b) return;
      const zoom = clamp(current.zoom * (Math.hypot(a.x - b.x, a.y - b.y) / current.distance), MIN_ZOOM, MAX_ZOOM);
      const mid = { x: (a.x + b.x) / 2 - shiftRef.current.x, y: (a.y + b.y) / 2 - shiftRef.current.y };
      const ratio = zoom / current.zoom;
      setView({ zoom, x: mid.x - (current.midX - current.x) * ratio, y: mid.y - (current.midY - current.y) * ratio });
    } else if (current.kind === "pan") {
      const point = local(event);
      setView((view) => ({ ...view, x: current.viewX + point.x - current.startX, y: current.viewY + point.y - current.startY }));
    } else {
      const ctx = canvasRef.current?.getContext("2d");
      if (!ctx) return;
      const events = event.nativeEvent.getCoalescedEvents?.() ?? [];
      for (const item of events.length ? events : [event.nativeEvent]) {
        const [x, y] = toMask(item);
        current.stroke.points.push(x, y);
      }
      // Redraw the live stroke as one path so its overlaps stay even.
      redraw(strokesRef.current);
      drawStroke(ctx, current.stroke);
    }
  };

  const onPointerEnd = (event: ReactPointerEvent<HTMLDivElement>) => {
    pointers.current.delete(event.pointerId);
    const current = gesture.current;
    if (!current) return;
    if (current.kind === "paint") {
      if (event.type === "pointercancel") redraw(strokesRef.current);
      else commit(current.stroke);
      gesture.current = null;
    } else if (current.kind === "pinch") {
      // Lifting one finger ends the pinch; painting needs a fresh touch.
      if (pointers.current.size < 2) gesture.current = null;
    } else if (pointers.current.size === 0) {
      gesture.current = null;
    }
  };

  const panning = !paint || tool === "hand" || spaceHeld;
  const viewStyle: CSSProperties | undefined =
    natural && fit
      ? {
          width: natural.w * fit,
          height: natural.h * fit,
          transform: `translate(-50%, -50%) translate(${view.x + frameShift.x}px, ${view.y + frameShift.y}px) scale(${view.zoom})`,
        }
      : undefined;
  const frameStyle: CSSProperties | undefined =
    frame && natural
      ? {
          left: `${(-frame.left / natural.w) * 100}%`,
          top: `${(-frame.top / natural.h) * 100}%`,
          width: `${((natural.w + frame.left + frame.right) / natural.w) * 100}%`,
          height: `${((natural.h + frame.top + frame.bottom) / natural.h) * 100}%`,
        }
      : undefined;

  return (
    <div className="edit-canvas">
      <div
        ref={stageRef}
        className={`edit-stage ${panning ? "panning" : ""} ${paint && !panning ? "painting" : ""}`}
        role="group"
        aria-label={paint ? "Canvas. Paint over the area to change. Pinch or scroll to zoom." : "Canvas. Pinch or scroll to zoom, drag to move."}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerEnd}
        onPointerCancel={onPointerEnd}
        onPointerLeave={(event) => {
          if (!pointers.current.has(event.pointerId)) setCursor(null);
        }}
        onContextMenu={(event) => event.preventDefault()}
      >
        <div className="edit-view" style={viewStyle ?? { visibility: "hidden" }}>
          {frameStyle && <div className="edit-frame" style={frameStyle} aria-hidden="true" />}
          <img
            src={asset.url}
            alt=""
            draggable={false}
            decoding="async"
            onLoad={(event) => {
              const img = event.currentTarget;
              if (!natural || natural.w !== img.naturalWidth || natural.h !== img.naturalHeight) setNatural({ w: img.naturalWidth, h: img.naturalHeight });
            }}
          />
          {/* Kept while other tools are open, so the painting survives a tool switch. */}
          {maskSize && <canvas ref={canvasRef} className="edit-mask" width={maskSize.w} height={maskSize.h} hidden={!paint} />}
        </div>
        {paint && !panning && cursor && (
          <span className="brush-cursor" aria-hidden="true" style={{ width: brush, height: brush, transform: `translate(${cursor.x - brush / 2}px, ${cursor.y - brush / 2}px)` }} />
        )}
      </div>
      <div className="edit-toolbar" role="toolbar" aria-label="Canvas tools">
        {paint && (
          <>
            <div className="segmented segmented-small" role="group" aria-label="Paint tool">
              <button type="button" aria-pressed={tool === "brush"} aria-label="Brush" title="Brush (B)" onClick={() => setTool("brush")}>
                <Icon name="brush" />
              </button>
              <button type="button" aria-pressed={tool === "eraser"} aria-label="Eraser" title="Eraser (E)" onClick={() => setTool("eraser")}>
                <Icon name="ink_eraser" />
              </button>
              <button type="button" className="tool-hand" aria-pressed={tool === "hand"} aria-label="Move" title="Move (H, or hold Space)" onClick={() => setTool("hand")}>
                <Icon name="pan_tool" />
              </button>
            </div>
            <label className="brush-size" title="Brush size ([ and ])">
              <span className="sr-only">Brush size</span>
              <span className="brush-dot" aria-hidden="true" style={{ width: 6 + brush / 10, height: 6 + brush / 10 }} />
              <input type="range" min={4} max={200} step={1} value={brush} onChange={(event) => setBrush(Number(event.target.value))} aria-valuetext={`${brush} pixels`} />
            </label>
            <button type="button" className="icon-button" aria-label="Undo" title="Undo (Ctrl+Z)" disabled={!strokes.length} onClick={undo}>
              <Icon name="undo" />
            </button>
            <button type="button" className="icon-button" aria-label="Redo" title="Redo (Ctrl+Shift+Z)" disabled={!redo.length} onClick={redoOne}>
              <Icon name="redo" />
            </button>
            <button type="button" className="icon-button" aria-label="Clear the mask" title="Clear" disabled={!painted(strokes)} onClick={clear}>
              <Icon name="layers_clear" />
            </button>
          </>
        )}
        <div className="edit-zoom">
          <button type="button" className="icon-button" aria-label="Zoom out" title="Zoom out (-)" disabled={view.zoom <= MIN_ZOOM} onClick={() => zoomAt(0.8)}>
            <Icon name="zoom_out" />
          </button>
          <button type="button" className="zoom-value" aria-label="Fit to view" title="Fit (0)" onClick={resetView}>
            {Math.round(view.zoom * 100)}%
          </button>
          <button type="button" className="icon-button" aria-label="Zoom in" title="Zoom in (+)" disabled={view.zoom >= MAX_ZOOM} onClick={() => zoomAt(1.25)}>
            <Icon name="zoom_in" />
          </button>
        </div>
      </div>
    </div>
  );
});
