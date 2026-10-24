import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { PHONE, useMediaQuery } from "../lib/prefs";
import type { Asset } from "../lib/types";
import { AssetCard } from "./asset-card";

export interface GridProps {
  items: Asset[];
  /** Target cell width in px; equal columns stretch to fill the width. */
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
}

interface Range {
  start: number;
  end: number;
}

/**
 * A virtualized square-cell gallery. Only rows near the viewport are in the DOM, so a library of
 * thousands stays fast. Arrow keys move between cards and scroll them into view.
 */
export function AssetGrid({ items, size, scroller, label, titleOf, onOpen, onFavorite, hasMore, loading, onEndReached, fresh, selected, selectable }: GridProps) {
  const gridRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [range, setRange] = useState<Range>({ start: 0, end: 8 });
  const phone = useMediaQuery(PHONE);
  const gap = phone ? 4 : 6;

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

  // The slider sets a target width: use the column count whose width lands closest to it.
  const fit = width ? Math.max(1, Math.round((width + gap) / (size + gap))) : 1;
  // Collapse unused columns when only a few items remain, so cells fill the canvas.
  const columns = Math.max(1, Math.min(fit, items.length || 1));
  const cell = width ? (width - gap * (columns - 1)) / columns : size;
  const rowHeight = cell + gap;
  const rows = Math.ceil(items.length / columns);

  const measure = useCallback(() => {
    const grid = gridRef.current;
    if (!grid) return;
    const box = scroller?.current;
    const viewTop = box ? box.getBoundingClientRect().top : 0;
    const viewHeight = box ? box.clientHeight : window.innerHeight;
    const offset = viewTop - grid.getBoundingClientRect().top;
    const start = Math.max(0, Math.floor((offset - viewHeight * 0.75) / rowHeight));
    const end = Math.min(rows, Math.ceil((offset + viewHeight * 1.75) / rowHeight));
    setRange((current) => (current.start === start && current.end === end ? current : { start, end }));
  }, [rowHeight, rows, scroller]);

  useLayoutEffect(measure, [measure, items.length, width]);

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

  // Infinite scroll: ask for the next page while the last rows are still a screen away.
  useEffect(() => {
    if (hasMore && !loading && onEndReached && rows > 0 && range.end >= rows - 1) onEndReached();
  }, [hasMore, loading, onEndReached, range.end, rows]);

  const scrollToIndex = useCallback(
    (index: number) => {
      const grid = gridRef.current;
      if (!grid) return;
      const row = Math.floor(index / columns);
      const box = scroller?.current;
      const gridTop = grid.getBoundingClientRect().top;
      const viewTop = box ? box.getBoundingClientRect().top : 0;
      const viewHeight = box ? box.clientHeight : window.innerHeight;
      const top = gridTop + row * rowHeight;
      const delta = top < viewTop ? top - viewTop - 8 : top + cell > viewTop + viewHeight ? top + cell - (viewTop + viewHeight) + 8 : 0;
      if (!delta) return;
      if (box) box.scrollTop += delta;
      else window.scrollBy(0, delta);
    },
    [cell, columns, rowHeight, scroller],
  );

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const card = (event.target as HTMLElement).closest<HTMLElement>("[data-index]");
    if (!card || !(event.target as HTMLElement).classList.contains("asset-open")) return;
    const index = Number(card.dataset.index);
    const moves: Record<string, number> = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: columns, ArrowUp: -columns, Home: -index, End: items.length - 1 - index };
    const move = moves[event.key];
    if (move === undefined) return;
    const target = Math.max(0, Math.min(items.length - 1, index + move));
    if (target === index) return;
    event.preventDefault();
    scrollToIndex(target);
    requestAnimationFrame(() =>
      requestAnimationFrame(() => gridRef.current?.querySelector<HTMLElement>(`[data-index="${target}"] .asset-open`)?.focus({ preventScroll: true })),
    );
  };

  const first = range.start * columns;
  const visible = items.slice(first, range.end * columns);

  return (
    <div ref={gridRef} className="vgrid" style={{ height: rows ? rows * rowHeight - gap : 0 }} onKeyDown={onKeyDown}>
      <div
        role="list"
        aria-label={label}
        className="gallery"
        style={{ transform: `translateY(${range.start * rowHeight}px)`, gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`, gap }}
      >
        {visible.map((asset, offset) => (
          <AssetCard
            key={asset.id}
            asset={asset}
            index={first + offset}
            title={titleOf(asset)}
            fresh={fresh?.has(asset.id)}
            selected={selected?.has(asset.id)}
            selectable={selectable}
            onOpen={onOpen}
            onFavorite={onFavorite}
          />
        ))}
      </div>
    </div>
  );
}
