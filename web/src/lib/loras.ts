import { useEffect, useState } from "react";
import { api, errorMessage } from "./api";
import { subscribeEvents, onReconnect } from "./events";
import type { Lora, ModelSpec } from "./types";

export const familyLabel = (family: string) => ({ flux1: "FLUX.1", qwen_image: "Qwen Image", qwen_image_2512: "Qwen Image 2512", wan21: "Wan 2.1" })[family] ?? family;
export function loraFamily(model?: ModelSpec): string | null {
  return model?.meta?.supports_loras === true && typeof model.meta.lora_family === "string" ? model.meta.lora_family : null;
}
export function useLoras(family?: string) {
  const [data, setData] = useState<Lora[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const load = () => { void api.loras(family).then(({ loras }) => { if (live) { setData(loras); setError(null); } }).catch((reason) => { if (live) setError(errorMessage(reason)); }); };
    load();
    const stop = subscribeEvents({ lora: load, library: load, job: (job) => { if (job.output === "lora" && job.status === "succeeded") load(); }, asset: (asset) => { if (asset.kind === "lora") load(); } });
    const reconnect = onReconnect(load);
    return () => { live = false; stop(); reconnect(); };
  }, [family]);
  return { data, error };
}
