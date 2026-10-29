import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { PHONE, useMediaQuery } from "../lib/prefs";
import type { Asset } from "../lib/types";
import { AssetCard } from "./asset-card";
import { Spinner } from "./icon";
import { LivePreview } from "./live-preview";

/** A pending output shown at its requested shape until the real asset arrives. */
export interface GridPlaceholder {
  key: string;
  ratio: number;
  progress: number | null;
  /** The run's latest in-progress frame, when its backend streams them. */
  preview?: string;
}

export interface GridProps {
  items: Asset[];
  /** Target column width in px; equal columns stretch to fill the width. */
  size: number;
  /** The element that scrolls, or the window when omitted. */
  scroller?: RefObject<HTMLElement | null>;
  label: string;
  titleOf(asset: Asset): string;
  onOpen(asset: Asset, index: number): void;
  onFavorite?(asset: Asset): void;
  hasMore?: boolean;
  loading?: boolean;
  onEndReached?(): void;
  fresh?: ReadonlySet<string>;
  selected?: ReadonlySet<string>;
  selectable?: boolean;
  placeholders?: GridPlaceholder[];
  /** What a click does, for the cards' accessible names: "Open" by default. */
  verb?: string;
  /** Hover actions on the cards (variations, favorite). Pickers turn them off. */
  actions?: boolean;
}

/** Width over height, from the asset's own size; 1:1 when unknown, clamped so a cell stays usable. */
export function assetRatio(asset: Asset): number {
  return clampRatio(asset.width && asset.height ? asset.width / asset.height : 1);
}

/** Shared empty default, so grids without placeholders keep their memoized layout between renders. */
const NO_PLACEHOLDERS: GridPlaceholder[] = [];

export const clampRatio = (ratio: number) => (Number.isFinite(ratio) && ratio > 0 ? Math.min(3, Math.max(1 / 3, ratio)) : 1);

interface Layout {
  columns: number;
  cellWidth: number;
  /** Per cell (placeholders first, then items): left, top, height. */
  x: Float64Array;
  y: Float64Array;
  h: Float64Array;
  column: Int32Array;
  height: number;
}

/** Masonry: each cell goes to the shortest column, at its own aspect ratio. */
function layoutCells(ratios: number[], width: number, size: number, gap: number): Layout {
  // The column count follows the size slider only: a few results keep their size instead of
  // stretching to the full width (one result would otherwise be taller than the window).
  const columns = width ? Math.max(1, Math.round((width + gap) / (size + gap))) : 1;
  const cellWidth = width ? (width - gap * (columns - 1)) / columns : size;
  const heights = new Float64Array(columns);
  const n = ratios.length;
  const x = new Float64Array(n);
  const y = new Float64Array(n);
  const h = new Float64Array(n);
  const column = new Int32Array(n);
  for (let i = 0; i < n; i++) {
    let shortest = 0;
    for (let c = 1; c < columns; c++) if (heights[c]! < heights[shortest]! - 0.5) shortest = c;
    column[i] = shortest;
    x[i] = shortest * (cellWidth + gap);
    y[i] = heights[shortest]!;
    h[i] = cellWidth / ratios[i]!;
    heights[shortest] = y[i]! + h[i]! + gap;
  }
  let height = 0;
  for (let c = 0; c < columns; c++) height = Math.max(height, heights[c]!);
  return { columns, cellWidth, x, y, h, column, height: Math.max(0, height - gap) };
}

/**
 * A virtualized masonry gallery: equal-width columns, every item at its own shape. Only cells
 * near the viewport are in the DOM, so a library of thousands stays fast. Tab order follows
 * the list (newest first); arrow keys move between cards and scroll them into view.
 */
export function AssetGrid({ items, size, scroller, label, titleOf, onOpen, onFavorite, hasMore, loading, onEndReached, fresh, selected, selectable, placeholders = NO_PLACEHOLDERS, verb, actions = true }: GridProps) {
  const gridRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [view, setView] = useState({ top: 0, bottom: 1200 });
  const phone = useMediaQuery(PHONE);
  const gap = phone ? 4 : 6;
  const lead = placeholders.length;

  useLayoutEffect(() => {
    const element = gridRef.current;
    if (!element) return;
    setWidth(element.clientWidth);
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setWidth(entry.contentRect.width);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const ratios = useMemo(() => [...placeholders.map((item) => clampRatio(item.ratio)), ...items.map(assetRatio)], [placeholders, items]);
  const layout = useMemo(() => layoutCells(ratios, width, size, gap), [ratios, width, size, gap]);

  const measure = useCallback(() => {
    const grid = gridRef.current;
    if (!grid) return;
    const box = scroller?.current;
    const viewTop = box ? box.getBoundingClientRect().top : 0;
    const viewHeight = box ? box.clientHeight : window.innerHeight;
    const offset = viewTop - grid.getBoundingClientRect().top;
    // One screen of overscan above and below keeps fast scrolling free of blank cells.
    const top = Math.floor((offset - viewHeight) / 200) * 200;
    const bottom = Math.ceil((offset + viewHeight * 2) / 200) * 200;
    setView((current) => (current.top === top && current.bottom === bottom ? current : { top, bottom }));
  }, [scroller]);

  useLayoutEffect(measure, [measure, layout.height, width]);

  useEffect(() => {
    const target: HTMLElement | Window = scroller?.current ?? window;
    let frame = 0;
    const onScroll = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    target.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onScroll);
    return () => {
      cancelAnimationFrame(frame);
      target.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onScroll);
    };
  }, [measure, scroller]);

  // Infinite scroll: ask for the next page while the end is still a screen away.
  useEffect(() => {
    if (hasMore && !loading && onEndReached && items.length > 0 && view.bottom >= layout.height) onEndReached();
  }, [hasMore, loading, onEndReached, items.length, view.bottom, layout.height]);

  const visible: number[] = [];
  for (let i = 0; i < ratios.length; i++) {
    if (layout.y[i]! < view.bottom && layout.y[i]! + layout.h[i]! > view.top) visible.push(i);
  }

  const scrollToCell = useCallback(
    (cell: number) => {
      const grid = gridRef.current;
      if (!grid) return;
      const box = scroller?.current;
      const viewTop = box ? box.getBoundingClientRect().top : 0;
      const viewHeight = box ? box.clientHeight : window.innerHeight;
      const top = grid.getBoundingClientRect().top + layout.y[cell]!;
      const bottom = top + layout.h[cell]!;
      const delta = top < viewTop ? top - viewTop - 8 : bottom > viewTop + viewHeight ? Math.min(top - viewTop - 8, bottom - (viewTop + viewHeight) + 8) : 0;
      if (!delta) return;
      if (box) box.scrollTop += delta;
      else window.scrollBy(0, delta);
    },
    [layout, scroller],
  );

  /** The nearest cell straight above or below in the same column. */
  const vertical = (cell: number, down: boolean) => {
    let best = -1;
    for (let i = lead; i < ratios.length; i++) {
      if (layout.column[i] !== layout.column[cell]) continue;
      if (down ? layout.y[i]! > layout.y[cell]! && (best < 0 || layout.y[i]! < layout.y[best]!) : layout.y[i]! < layout.y[cell]! && (best < 0 || layout.y[i]! > layout.y[best]!)) best = i;
    }
    return best;
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const card = (event.target as HTMLElement).closest<HTMLElement>("[data-index]");
    if (!card || !(event.target as HTMLElement).classList.contains("asset-open")) return;
    const index = Number(card.dataset.index);
    const cell = index + lead;
    let target: number;
    if (event.key === "ArrowRight") target = index + 1;
    else if (event.key === "ArrowLeft") target = index - 1;
    else if (event.key === "Home") target = 0;
    else if (event.key === "End") target = items.length - 1;
    else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const next = vertical(cell, event.key === "ArrowDown");
      target = next < 0 ? index : next - lead;
    } else return;
    target = Math.max(0, Math.min(items.length - 1, target));
    if (target === index) return;
    event.preventDefault();
    scrollToCell(target + lead);
    requestAnimationFrame(() =>
      requestAnimationFrame(() => gridRef.current?.querySelector<HTMLElement>(`[data-index="${target}"] .asset-open`)?.focus({ preventScroll: true })),
    );
  };

  const place = (cell: number) => ({ transform: `translate(${layout.x[cell]}px, ${layout.y[cell]}px)`, width: layout.cellWidth, height: layout.h[cell] });

  return (
    <div ref={gridRef} className="vgrid" role="list" aria-label={label} style={{ height: layout.height }} onKeyDown={onKeyDown}>
      {visible.map((cell) => {
        if (cell < lead) {
          const placeholder = placeholders[cell]!;
          return (
            <div key={placeholder.key} className={`asset asset-pending${placeholder.preview ? " has-preview" : ""}`} style={place(cell)} aria-hidden="true">
              {placeholder.preview ? <LivePreview src={placeholder.preview} /> : <Spinner />}
              {placeholder.progress !== null && (
                <span className="pending-progress">
                  <span style={{ transform: `scaleX(${Math.max(0.02, Math.min(1, placeholder.progress))})` }} />
                </span>
              )}
            </div>
          );
        }
        const asset = items[cell - lead]!;
        return (
          <AssetCard
            key={asset.id}
            asset={asset}
            index={cell - lead}
            title={titleOf(asset)}
            fresh={fresh?.has(asset.id)}
            selected={selected?.has(asset.id)}
            selectable={selectable}
            verb={verb}
            // Tiles too small for the caption's buttons only pick.
            actions={actions && layout.cellWidth >= 120}
            onOpen={onOpen}
            onFavorite={onFavorite}
            style={place(cell)}
          />
        );
      })}
    </div>
  );
}
