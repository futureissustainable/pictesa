import { useEffect, useState } from "react";
import { api, errorMessage } from "./api";
import { invalidateModels } from "./catalog";
import { notifyError } from "./notices";
import type { Download, ModelSpec } from "./types";

type MissingFile = { name: string; size?: number | null };

/** Model files a ComfyUI template lacks on its server, from the server's last check. */
export function missingModels(model: ModelSpec | undefined): MissingFile[] {
  if (!model?.tags?.includes("needs-models") || model.tags.includes("needs-nodes")) return [];
  const list = model.meta?.downloads;
  return Array.isArray(list) ? (list as MissingFile[]) : [];
}

/** Starts and follows a model's file downloads; reloads the model list once they finish. */
export function useModelDownload(modelId: string | undefined) {
  const [files, setFiles] = useState<Download[]>([]);
  const active = files.some((file) => file.status === "queued" || file.status === "running");

  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => {
      void api.downloads().then(async ({ downloads }) => {
        const mine = downloads.filter((file) => file.model === modelId);
        setFiles(mine);
        if (mine.some((file) => file.status === "queued" || file.status === "running")) return;
        const failed = mine.find((file) => file.error);
        if (failed) notifyError(failed.error ?? "The download failed.");
        await api.refreshModels("comfyui").catch(() => undefined);
        invalidateModels();
      }, () => undefined);
    }, 1500);
    return () => clearInterval(timer);
  }, [active, modelId]);

  const start = async () => {
    if (!modelId) return;
    try {
      const { downloads } = await api.startDownload(modelId);
      if (downloads.length) {
        setFiles(downloads);
      } else {
        await api.refreshModels("comfyui");
        invalidateModels();
      }
    } catch (error) {
      notifyError(errorMessage(error));
    }
  };

  const total = files.reduce((sum, file) => sum + (file.total ?? 0), 0);
  const done = files.reduce((sum, file) => sum + file.done, 0);
  return { active, percent: total ? Math.floor((done * 100) / total) : 0, start };
}
