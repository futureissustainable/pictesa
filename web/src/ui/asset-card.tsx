import { memo, useState, type CSSProperties, type DragEvent } from "react";
import { ASSET_DRAG_TYPE } from "../lib/attach";
import { clock, dimensions, kindIcon } from "../lib/format";
import { prefersReducedMotion } from "../lib/prefs";
import type { Asset } from "../lib/types";
import { Icon } from "./icon";

/** Thumbnail with fallbacks: the server answers 404 until a thumbnail exists. */
export function Thumb({ asset, className = "", eager = false }: { asset: Asset; className?: string; eager?: boolean }) {
  const [step, setStep] = useState<"thumb" | "original" | "none">(asset.thumb_url ? "thumb" : asset.kind === "audio" ? "none" : "original");
  if (asset.kind === "audio" || step === "none") {
    return (
      <span className={`thumb-placeholder ${className}`}>
        <Icon name={kindIcon[asset.kind]} />
      </span>
    );
  }
  if (step === "original" && asset.kind === "video") {
    // No poster yet: the first frame of the video stands in.
    return <video className={className} src={`${asset.url}#t=0.1`} preload="metadata" muted playsInline aria-hidden="true" tabIndex={-1} />;
  }
  return (
    <img
      className={className}
      src={step === "thumb" ? asset.thumb_url! : asset.url}
      alt=""
      loading={eager ? "eager" : "lazy"}
      decoding="async"
      draggable={false}
      onError={() => setStep(step === "thumb" ? "original" : "none")}
    />
  );
}

export interface CardProps {
  asset: Asset;
  index: number;
  title: string;
  fresh?: boolean;
  selected?: boolean;
  selectable?: boolean;
  /** What a click does, for the accessible name: "Open" by default. */
  verb?: string;
  onOpen(asset: Asset, index: number): void;
  onFavorite?(asset: Asset): void;
  /** Position and size from the masonry layout. */
  style?: CSSProperties;
}

function startDrag(event: DragEvent, asset: Asset) {
  event.dataTransfer.effectAllowed = "copy";
  event.dataTransfer.setData(ASSET_DRAG_TYPE, JSON.stringify(asset));
  event.dataTransfer.setData("text/uri-list", new URL(asset.url, location.origin).href);
}

export const AssetCard = memo(function AssetCard({ asset, index, title, fresh, selected, selectable, verb = "Open", onOpen, onFavorite, style }: CardProps) {
  const [playing, setPlaying] = useState(false);
  const detail = asset.kind === "image" ? dimensions(asset.width, asset.height) : asset.duration_ms ? clock(asset.duration_ms) : null;
  const label = asset.prompt ? `${title}: ${asset.prompt.slice(0, 120)}` : title;
  return (
    <div role="listitem" className={`asset ${fresh ? "asset-fresh" : ""} ${selected ? "asset-selected" : ""}`} data-index={index} style={style}>
      <button
        type="button"
        className="asset-open"
        aria-label={selectable ? `Select ${label}` : `${verb} ${label}`}
        aria-pressed={selectable ? Boolean(selected) : undefined}
        draggable={!selectable}
        onDragStart={(event) => startDrag(event, asset)}
        onClick={() => onOpen(asset, index)}
        onPointerEnter={(event) => {
          if (asset.kind === "video" && event.pointerType === "mouse" && !prefersReducedMotion()) setPlaying(true);
        }}
        onPointerLeave={() => setPlaying(false)}
      >
        <Thumb key={asset.thumb_url ?? "none"} asset={asset} />
        {playing && <video className="asset-preview" src={asset.url} preload="metadata" muted loop playsInline autoPlay aria-hidden="true" tabIndex={-1} />}
        {asset.kind !== "image" && (
          <span className="asset-badge">
            <Icon name={asset.kind === "video" ? "play_arrow" : "graphic_eq"} />
            {asset.duration_ms ? clock(asset.duration_ms) : null}
          </span>
        )}
        {selectable && selected && (
          <span className="asset-check">
            <Icon name="check" />
          </span>
        )}
      </button>
      {!selectable && (
        <div className="asset-meta">
          <div className="asset-title">
            <p>{title}</p>
            {detail && <small>{detail}</small>}
          </div>
          {onFavorite && (
            <button
              type="button"
              className={`icon-button ${asset.favorite ? "selected" : ""}`}
              aria-label={asset.favorite ? "Remove from favorites" : "Add to favorites"}
              aria-pressed={asset.favorite}
              onClick={() => onFavorite(asset)}
            >
              <Icon name="favorite" filled={asset.favorite} />
            </button>
          )}
        </div>
      )}
    </div>
  );
});
