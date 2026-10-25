import { useRef, useState, type DragEvent } from "react";
import { acceptFor, ASSET_DRAG_TYPE, readDraggedAsset, rejectionMessage, routeFiles, slotScope, uploadErrors, uploadInto, type UploadError } from "../../lib/attach";
import { useCharacters } from "../../lib/characters";
import { addInput, inputKey, removeInput, setCharacter, slotInputs, takesCharacter, useComposer, type AttachedInput } from "../../lib/composer";
import { kindPlural, plural, roleIcon, roleLabel } from "../../lib/format";
import { Link } from "../../lib/router";
import { notifyError } from "../../lib/notices";
import { useStore } from "../../lib/store";
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
  const scopes = new Set(model.inputs.map((slot) => slotScope(mode, slot.role)));
  const refused = useStore(uploadErrors).filter((item) => scopes.has(item.scope));
  return (
    <div className="slots-area">
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
        <CharacterSlot model={model} mode={mode} invalid={invalidRole === "character"} />
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
      <UploadErrors errors={refused} />
    </div>
  );
}

/** Refused uploads, next to the drop zones. Each line names the file and the server's reason. */
export function UploadErrors({ errors }: { errors: UploadError[] }) {
  if (!errors.length) return null;
  return (
    <ul className="upload-errors" role="alert">
      {errors.map((item) => (
        <li key={item.id}>
          <Icon name="error" />
          <span>
            <strong>{item.name}</strong> {item.error}
          </span>
        </li>
      ))}
    </ul>
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

/** The character next to the reference zone. Models without image references cannot use one. */
function CharacterSlot({ model, mode, invalid }: { model: ModelSpec; mode: MediaKind; invalid: boolean }) {
  const selectedId = useComposer().characterByMode[mode] ?? null;
  const { data } = useCharacters();
  const selected = data?.find((character) => character.id === selectedId) ?? null;
  const usable = takesCharacter(model);
  const reason = `${model.name} takes no reference images, so it cannot use a character.`;
  const cover = selected?.assets[0];
  const remove = selected && (
    <button type="button" className="slot-remove" aria-label="Remove the character" onClick={() => setCharacter(mode, null)}>
      <Icon name="close" />
    </button>
  );
  return (
    <div className={`slot-group character-slot ${invalid ? "invalid" : ""}`} role="group" aria-label="Character">
      {usable ? (
        <Control
          label={selected ? `Character: ${selected.name}` : "Add a character"}
          className={`slot-add ${selected ? "chosen" : ""}`}
          showTitle={false}
          placement="above"
          size="wide"
          trigger={
            <>
              {cover ? <Thumb asset={cover} /> : <Icon name="face" />}
              <span className="slot-label">{selected?.name ?? "Character"}</span>
            </>
          }
        >
          <p className="control-title">Character</p>
          <div className="control-list">
            {selected && (
              <Choice icon="close" onPick={() => setCharacter(mode, null)}>
                No character
              </Choice>
            )}
            {(data ?? []).map((character) => (
              <Choice key={character.id} selected={character.id === selectedId} onPick={() => setCharacter(mode, character.id)} className="character-choice">
                <span className="character-choice-row">
                  {character.assets[0] ? <Thumb asset={character.assets[0]} /> : <Icon name="face" />}
                  <span>
                    {character.name}
                    <small>
                      {character.kind === "brand" ? "Brand kit, " : ""}
                      {plural(character.assets.length, "image", "images")}
                    </small>
                  </span>
                </span>
              </Choice>
            ))}
          </div>
          {data && !data.length && <p className="control-help">No characters yet. Select images in the library and save them as a character.</p>}
          <p className="control-help">
            <Link to="/characters">Manage characters</Link>
          </p>
        </Control>
      ) : (
        <span className="control-trigger slot-add disabled" title={reason} aria-disabled="true">
          {cover ? <Thumb asset={cover} /> : <Icon name="face" />}
          <span className="slot-label">{selected ? "Not used" : "Character"}</span>
          <span className="sr-only">{reason}</span>
        </span>
      )}
      {remove}
    </div>
  );
}
