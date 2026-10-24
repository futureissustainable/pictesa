import { useRef, useState, type DragEvent } from "react";
import { acceptFor, ASSET_DRAG_TYPE, readDraggedAsset, rejectionMessage, routeFiles, uploadInto } from "../../lib/attach";
import { addInput, inputKey, removeInput, slotInputs, type AttachedInput } from "../../lib/composer";
import { kindPlural, roleIcon, roleLabel } from "../../lib/format";
import { notifyError } from "../../lib/notices";
import type { Asset, InputSlot, MediaKind, ModelSpec } from "../../lib/types";
import { Thumb } from "../../ui/asset-card";
import { Choice, Control } from "../../ui/control";
import { Icon, Spinner } from "../../ui/icon";
import { LibraryPicker } from "./library-picker";

export const slotLabel = (slot: InputSlot) => slot.label || roleLabel[slot.role];

const hasPayload = (event: DragEvent) => event.dataTransfer.types.includes("Files") || event.dataTransfer.types.includes(ASSET_DRAG_TYPE);

/** Drop zones generated from the model's input slots. */
export function InputSlots({ model, mode, inputs, invalidRole }: { model: ModelSpec; mode: MediaKind; inputs: AttachedInput[]; invalidRole: string | null }) {
  const [picking, setPicking] = useState<InputSlot | null>(null);
  const pickRoom = picking ? picking.max - slotInputs(picking, inputs).length : 0;
  return (
    <div className="slots" role="group" aria-label="Inputs">
      {model.inputs.map((slot) => (
        <SlotGroup
          key={`${slot.role}-${slot.kind}`}
          slot={slot}
          model={model}
          mode={mode}
          inputs={inputs}
          invalid={invalidRole === slot.role}
          onPick={() => setPicking(slot)}
        />
      ))}
      {picking && (
        <LibraryPicker
          kind={picking.kind}
          room={Math.max(1, pickRoom)}
          title={`Choose ${pickRoom > 1 ? kindPlural[picking.kind] : `a ${picking.kind}`} for ${slotLabel(picking).toLowerCase()}`}
          onClose={() => setPicking(null)}
          onPick={(assets) => assets.forEach((asset) => addInput(mode, { key: inputKey(), role: picking.role, asset }))}
        />
      )}
    </div>
  );
}

function SlotGroup({ slot, model, mode, inputs, invalid, onPick }: { slot: InputSlot; model: ModelSpec; mode: MediaKind; inputs: AttachedInput[]; invalid: boolean; onPick(): void }) {
  const filled = slotInputs(slot, inputs);
  const label = slotLabel(slot);
  const room = slot.max - filled.length;
  const fileRef = useRef<HTMLInputElement>(null);
  const [over, setOver] = useState(false);

  const drop = (event: DragEvent) => {
    event.preventDefault();
    event.stopPropagation();
    setOver(false);
    const asset = readDraggedAsset(event.dataTransfer);
    if (asset) return placeDragged(asset);
    const files = [...event.dataTransfer.files];
    const { placements, rejected } = routeFiles(model, inputs, files, slot);
    placements.forEach((placement) => void uploadInto(mode, placement.slot.role, placement.files));
    if (rejected.length) notifyError(rejectionMessage(model, rejected));
  };

  const placeDragged = (asset: Asset) => {
    if (asset.kind !== slot.kind) return notifyError(`${label} takes ${kindPlural[slot.kind]}.`);
    if (filled.some((input) => input.asset?.id === asset.id)) return;
    if (room <= 0) return notifyError(`${label} is full.`);
    addInput(mode, { key: inputKey(), role: slot.role, asset });
  };

  const dropProps = {
    onDragOver: (event: DragEvent) => {
      if (!hasPayload(event)) return;
      event.preventDefault();
      event.stopPropagation();
      event.dataTransfer.dropEffect = "copy";
      setOver(true);
    },
    onDragLeave: () => setOver(false),
    onDrop: drop,
  };

  return (
    <div className={`slot-group ${over ? "drop-over" : ""} ${invalid ? "invalid" : ""}`} {...dropProps} aria-label={`${label}${slot.min ? ", required" : ""}`} role="group">
      {filled.map((input) => (
        <InputTile key={input.key} input={input} label={label} />
      ))}
      {room > 0 && (
        <Control
          label={`Add ${label.toLowerCase()}`}
          className={`slot-add ${slot.min > filled.length ? "required" : ""}`}
          showTitle={false}
          placement="above"
          trigger={
            <>
              <Icon name={roleIcon[slot.role]} />
              <span className="slot-label">{label}</span>
              {slot.max > 1 && filled.length > 0 && (
                <small className="slot-count">
                  {filled.length}/{slot.max}
                </small>
              )}
            </>
          }
        >
          <Choice icon="upload" onPick={() => fileRef.current?.click()}>
            Upload {slot.max - filled.length > 1 ? kindPlural[slot.kind] : `a ${slot.kind}`}
          </Choice>
          <Choice icon="photo_library" onPick={onPick}>
            Choose from library
          </Choice>
          <p className="control-help">Or drop or paste files on the composer.</p>
        </Control>
      )}
      <input
        ref={fileRef}
        type="file"
        className="sr-only"
        tabIndex={-1}
        aria-hidden="true"
        accept={acceptFor(slot.kind)}
        multiple={room > 1}
        onChange={(event) => {
          const files = [...(event.target.files ?? [])].slice(0, room);
          event.target.value = "";
          if (files.length) void uploadInto(mode, slot.role, files);
        }}
      />
    </div>
  );
}

function InputTile({ input, label }: { input: AttachedInput; label: string }) {
  const progress = input.upload?.progress;
  return (
    <div className="slot-tile" title={input.asset?.prompt ?? input.upload?.name ?? label}>
      {input.asset ? (
        <Thumb asset={input.asset} />
      ) : input.upload?.preview ? (
        <img src={input.upload.preview} alt="" />
      ) : (
        <span className="thumb-placeholder">
          <Icon name={roleIcon[input.role]} />
        </span>
      )}
      {input.upload && (
        <span className="slot-progress" role="status" aria-label={`Uploading ${input.upload.name}`}>
          {typeof progress === "number" ? `${Math.round(progress * 100)}%` : <Spinner />}
        </span>
      )}
      <span className="slot-caption">{label}</span>
      <button type="button" className="slot-remove" aria-label={`Remove ${label.toLowerCase()}`} onClick={() => removeInput(input.key)}>
        <Icon name="close" />
      </button>
    </div>
  );
}
