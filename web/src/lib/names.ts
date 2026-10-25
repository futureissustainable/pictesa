import type { Asset } from "./types";

/** Title for an asset: its supplied model label, catalog model or original file name. */
export const assetTitle = (asset: Asset, nameOf: (id: string | null | undefined) => string) =>
  asset.model_label || (asset.source === "generated" ? nameOf(asset.model) : asset.name || (asset.source === "upload" ? `Uploaded ${asset.kind}` : `Imported ${asset.kind}`));
