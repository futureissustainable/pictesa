import { readJson, writeJson } from "../lib/prefs";
import { createStore, useStore } from "../lib/store";
import { Icon } from "./icon";

// Target cell width for every gallery, from 128 to 640 px. Columns stretch to fill the canvas.

const KEY = "pictesa:image-size";

function initialSize(): number {
  const saved = readJson<number>(KEY, 0);
  if (saved >= 128 && saved <= 640 && saved % 16 === 0) return saved;
  const phone = window.matchMedia("(max-width: 560px)").matches;
  return phone ? Math.max(128, Math.floor((window.innerWidth - 24) / 32) * 16) : 384;
}

const sizeStore = createStore<number>(initialSize());
export const useImageSize = () => useStore(sizeStore);

export function SizeSlider() {
  const size = useImageSize();
  return (
    <label className="image-size-control">
      <span className="image-size-label">Image size</span>
      <Icon name="photo_size_select_small" />
      <input
        type="range"
        min="128"
        max="640"
        step="16"
        value={size}
        aria-label="Image size"
        aria-valuetext={`${size} pixel target, fitted to the canvas`}
        onChange={(event) => {
          const next = Number(event.target.value);
          sizeStore.set(next);
          writeJson(KEY, next);
        }}
      />
      <Icon name="photo_size_select_large" />
    </label>
  );
}
