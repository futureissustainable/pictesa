import { useState, type FormEvent } from "react";
import { api, ApiError, errorMessage } from "../../lib/api";
import { MAX_CHARACTER_IMAGES, removeCharacter, upsertCharacter, useCharacters } from "../../lib/characters";
import { plural } from "../../lib/format";
import { notify, notifyError } from "../../lib/notices";
import { sendCharacterToCreate } from "../../lib/reuse";
import { Link } from "../../lib/router";
import type { Asset, Character } from "../../lib/types";
import { Thumb } from "../../ui/asset-card";
import { Dialog } from "../../ui/dialog";
import { Icon, Spinner } from "../../ui/icon";
import { LibraryPicker } from "../create/library-picker";

/** Cover: the first image large, the next two stacked beside it. */
export function CharacterCover({ character, className = "" }: { character: Character; className?: string }) {
  const images = character.assets.slice(0, 3);
  return (
    <span className={`character-cover cover-${Math.max(1, images.length)} ${className}`} aria-hidden="true">
      {images.length ? images.map((asset) => <Thumb key={asset.id} asset={asset} />) : <Icon name="face" />}
    </span>
  );
}

export default function CharactersPage() {
  const { data, error } = useCharacters();
  const [editing, setEditing] = useState<Character | "new" | null>(null);

  return (
    <div className="workspace">
      <div className="intro characters-intro">
        <div>
          <h1>Characters.</h1>
          <p>A person, a product or a style, kept as a set of reference images. Pick one in Create to keep the look.</p>
        </div>
        <button type="button" className="primary-button" onClick={() => setEditing("new")}>
          <Icon name="add" />
          New character
        </button>
      </div>
      {!data ? (
        <div className={error ? "empty" : "page-loading"} role={error ? "alert" : "status"}>
          {error ? <Icon name="cloud_off" /> : <Spinner />}
          {error ? <p>{error}</p> : "Loading characters"}
        </div>
      ) : data.length ? (
        <ul className="character-grid" aria-label="Characters">
          {data.map((character) => (
            <CharacterCard key={character.id} character={character} onEdit={() => setEditing(character)} />
          ))}
        </ul>
      ) : (
        <div className="empty">
          <Icon name="face" />
          <h2>No characters yet.</h2>
          <p>Select 1 to {MAX_CHARACTER_IMAGES} images in the library and save them as a character, or start one here.</p>
          <Link className="primary-button" to="/library?kind=image">
            Open the library
            <Icon name="arrow_forward" />
          </Link>
        </div>
      )}
      {editing && <CharacterEditor character={editing === "new" ? null : editing} onClose={() => setEditing(null)} />}
    </div>
  );
}

function CharacterCard({ character, onEdit }: { character: Character; onEdit(): void }) {
  const [busy, setBusy] = useState(false);
  const remove = async () => {
    if (!confirm(`Delete ${character.name}? The images stay in the library.`)) return;
    setBusy(true);
    try {
      await api.deleteCharacter(character.id);
      removeCharacter(character.id);
      notify(`${character.name} deleted.`);
    } catch (error) {
      notifyError(errorMessage(error));
      setBusy(false);
    }
  };
  return (
    <li className="character-card">
      <button type="button" className="character-open" onClick={onEdit} aria-label={`Edit ${character.name}`} title="Edit">
        <CharacterCover character={character} />
        <span className="character-edit" aria-hidden="true">
          <Icon name="edit" />
        </span>
      </button>
      <div className="character-body">
        <p className="character-name">{character.name}</p>
        <p className="character-meta">{plural(character.assets.length, "image", "images")}</p>
        {character.description && <p className="character-description">{character.description}</p>}
      </div>
      <div className="character-actions">
        <button type="button" className="text-button" aria-label={`Use ${character.name} in Create`} onClick={() => void sendCharacterToCreate(character)}>
          <Icon name="arrow_upward" />
          Use in Create
        </button>
        <button type="button" className="icon-button" aria-label={`Delete ${character.name}`} title="Delete" disabled={busy} onClick={() => void remove()}>
          <Icon name="delete" />
        </button>
      </div>
    </li>
  );
}

/** Create or edit a character: name, description and 1 to 14 images. */
export function CharacterEditor({ character, initial = [], onClose }: { character: Character | null; initial?: Asset[]; onClose(saved?: boolean): void }) {
  const [name, setName] = useState(character?.name ?? "");
  const [description, setDescription] = useState(character?.description ?? "");
  const [images, setImages] = useState<Asset[]>(character?.assets ?? initial.slice(0, MAX_CHARACTER_IMAGES));
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ text: string; field?: string } | null>(null);
  const room = MAX_CHARACTER_IMAGES - images.length;

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    if (!name.trim()) return setError({ text: "Give the character a short name.", field: "name" });
    if (!images.length) return setError({ text: "Add at least one image.", field: "asset_ids" });
    setBusy(true);
    setError(null);
    try {
      const body = { name: name.trim(), description: description.trim(), asset_ids: images.map((asset) => asset.id) };
      const saved = character ? await api.updateCharacter(character.id, body) : await api.createCharacter(body);
      upsertCharacter(saved);
      notify(character ? `${saved.name} saved.` : `${saved.name} is ready. Pick it in Create.`);
      onClose(true);
    } catch (reason) {
      setError({ text: errorMessage(reason), field: reason instanceof ApiError ? reason.field : undefined });
      setBusy(false);
    }
  };

  return (
    <Dialog title={character ? `Edit ${character.name}` : "New character"} size="wide" onClose={() => onClose(false)} className="character-editor">
      <form onSubmit={(event) => void save(event)} noValidate>
        <div className="character-fields">
          <label className="field">
            <span>Name</span>
            <input type="text" value={name} maxLength={80} placeholder="Maya, Oak chair, Riso print" aria-invalid={error?.field === "name" || undefined} onChange={(event) => setName(event.target.value)} />
          </label>
          <label className="field">
            <span>
              Description <small>Optional</small>
            </span>
            <input type="text" value={description} maxLength={500} placeholder="What should stay the same" onChange={(event) => setDescription(event.target.value)} />
          </label>
        </div>
        <div className="field">
          <span className="field-top">
            <span>Images</span>
            <small>
              {images.length} of {MAX_CHARACTER_IMAGES}
            </small>
          </span>
          <ul className={`character-images ${error?.field === "asset_ids" ? "invalid" : ""}`}>
            {images.map((asset) => (
              <li key={asset.id} className="slot-tile">
                <Thumb asset={asset} />
                <button type="button" className="slot-remove" aria-label="Remove this image" onClick={() => setImages((list) => list.filter((item) => item.id !== asset.id))}>
                  <Icon name="close" />
                </button>
              </li>
            ))}
            {room > 0 && (
              <li>
                <button type="button" className="control-trigger slot-add" onClick={() => setPicking(true)}>
                  <Icon name="add_photo_alternate" />
                  <span className="slot-label">Add images</span>
                </button>
              </li>
            )}
          </ul>
        </div>
        {error && (
          <p className="notice error" role="alert">
            {error.text}
          </p>
        )}
        <div className="dialog-actions">
          <button type="button" className="text-button" onClick={() => onClose(false)}>
            Cancel
          </button>
          <button type="submit" className="primary-button" disabled={busy}>
            {busy ? <Spinner /> : <Icon name="check" />}
            {character ? "Save" : "Save character"}
          </button>
        </div>
      </form>
      {picking && (
        <LibraryPicker
          kind="image"
          room={room}
          title="Add images"
          onClose={() => setPicking(false)}
          onPick={(assets) => setImages((list) => [...list, ...assets.filter((asset) => !list.some((item) => item.id === asset.id))].slice(0, MAX_CHARACTER_IMAGES))}
        />
      )}
    </Dialog>
  );
}
