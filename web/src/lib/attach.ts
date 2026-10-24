import { errorMessage, uploadFiles } from "./api";
import { addInput, inputKey, patchInput, removeInput, slotInputs, type AttachedInput } from "./composer";
import { emitLocal } from "./events";
import { kindLabel, roleLabel } from "./format";
import { notifyError } from "./notices";
import type { Asset, InputRole, InputSlot, MediaKind, ModelSpec } from "./types";

export function fileKind(file: File): MediaKind | null {
  const type = file.type.split("/")[0];
  return type === "image" || type === "video" || type === "audio" ? type : null;
}

export const acceptFor = (kind: MediaKind) => `${kind}/*`;

/** Uploads files into one slot of the composer, showing real upload progress on each tile. */
export async function uploadInto(mode: MediaKind, role: InputRole, files: File[]) {
  await Promise.all(
    files.map(async (file) => {
      const kind = fileKind(file);
      if (!kind) return;
      const key = inputKey();
      const preview = kind === "image" ? URL.createObjectURL(file) : null;
      const pending = { name: file.name, kind, progress: null, preview };
      addInput(mode, { key, role, upload: pending });
      try {
        const [asset] = await uploadFiles([file], (progress) => patchInput(key, { upload: { ...pending, progress } }));
        if (!asset) throw new Error(`${file.name} did not upload.`);
        patchInput(key, { asset, upload: undefined });
        emitLocal({ asset });
      } catch (error) {
        removeInput(key);
        notifyError(`${file.name}: ${errorMessage(error)}`);
      } finally {
        if (preview) setTimeout(() => URL.revokeObjectURL(preview), 5000);
      }
    }),
  );
}

export interface Placement {
  slot: InputSlot;
  files: File[];
}

/**
 * Sends dropped or pasted files to the first slots that take their kind and still have room.
 * Returns the placements and the files no slot could take.
 */
export function routeFiles(model: ModelSpec, inputs: AttachedInput[], files: File[], preferred?: InputSlot): { placements: Placement[]; rejected: File[] } {
  const room = new Map<InputSlot, number>(model.inputs.map((slot) => [slot, slot.max - slotInputs(slot, inputs).length]));
  const placements = new Map<InputSlot, File[]>();
  const rejected: File[] = [];
  const order = preferred ? [preferred, ...model.inputs.filter((slot) => slot !== preferred)] : model.inputs;
  for (const file of files) {
    const kind = fileKind(file);
    const slot = order.find((candidate) => candidate.kind === kind && (room.get(candidate) ?? 0) > 0);
    if (!slot) {
      rejected.push(file);
      continue;
    }
    room.set(slot, (room.get(slot) ?? 0) - 1);
    placements.set(slot, [...(placements.get(slot) ?? []), file]);
  }
  return { placements: [...placements].map(([slot, list]) => ({ slot, files: list })), rejected };
}

export function rejectionMessage(model: ModelSpec, rejected: File[]): string {
  const kinds = [...new Set(rejected.map((file) => fileKind(file)).filter(Boolean))] as MediaKind[];
  if (!kinds.length) return "Use image, video or audio files.";
  const takes = model.inputs.filter((slot) => kinds.includes(slot.kind));
  if (!takes.length) return `${model.name} does not take ${kinds.map((kind) => kindLabel[kind].toLowerCase()).join(" or ")} files.`;
  return `No room left: ${takes.map((slot) => `${slot.label || roleLabel[slot.role]} takes ${slot.max}`).join(", ")}.`;
}

/** Library assets dragged from a grid card carry this type. */
export const ASSET_DRAG_TYPE = "application/x-pictesa-asset";

export function readDraggedAsset(data: DataTransfer): Asset | null {
  const raw = data.getData(ASSET_DRAG_TYPE);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as Asset;
  } catch {
    return null;
  }
}
