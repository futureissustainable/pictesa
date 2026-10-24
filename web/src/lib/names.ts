import type { Asset } from "./types";

/** Title for an asset: the model for generated work, the original file name for uploads and imports. */
export const assetTitle = (asset: Asset, nameOf: (id: string | null | undefined) => string) =>
  asset.source === "generated" ? nameOf(asset.model) : asset.name || (asset.source === "upload" ? `Uploaded ${asset.kind}` : `Imported ${asset.kind}`);
