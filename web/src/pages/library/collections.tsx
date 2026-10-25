import { useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { notify } from "../../lib/notices";
import { Link } from "../../lib/router";
import type { Collection } from "../../lib/types";
import { Dialog } from "../../ui/dialog";
import { Icon } from "../../ui/icon";

export function CollectionEditor({ collection, onClose }: { collection?: Collection; onClose(changed: boolean): void }) {
  const [name, setName] = useState(collection?.name ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return <Dialog title={collection ? "Rename collection" : "New collection"} onClose={() => onClose(false)}>
    <form className="library-form" onSubmit={async (event) => {
      event.preventDefault(); setBusy(true); setError(null);
      try {
        if (collection) await api.renameCollection(collection.id, name);
        else await api.createCollection(name);
        onClose(true);
      } catch (reason) { setError(errorMessage(reason)); } finally { setBusy(false); }
    }}>
      <label className="field"><span>Name</span><input autoFocus required maxLength={100} value={name} onChange={(event) => setName(event.target.value)} /></label>
      {error && <p role="alert" className="notice error">{error}</p>}
      <div className="dialog-actions"><button type="button" className="text-button" onClick={() => onClose(false)}>Cancel</button><button className="primary-button" disabled={busy || !name.trim()}>Save</button></div>
    </form>
  </Dialog>;
}

export function Collections({ items, reload }: { items: Collection[]; reload(): void }) {
  const [editing, setEditing] = useState<Collection | null | undefined>();
  const [deleting, setDeleting] = useState<Collection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  return <>
    <div className="collection-heading"><p className="hint">An asset can belong to several collections.</p><button type="button" className="primary-button" onClick={() => setEditing(null)}><Icon name="add" />New collection</button></div>
    {!items.length && <div className="empty"><Icon name="folder_open" /><h2>No collections yet.</h2><p>Create one, then select assets in the library to add them.</p></div>}
    <ul className="token-list">
      {items.map((collection) => <li className="connection-row" key={collection.id}>
        <Link className="collection-link" to={`/library?collection=${encodeURIComponent(collection.id)}`}><Icon name="folder_open" /><span>{collection.name}<small>{collection.asset_count} assets</small></span></Link>
        <button type="button" className="icon-button" aria-label={`Rename ${collection.name}`} onClick={() => setEditing(collection)}><Icon name="edit" /></button>
        <button type="button" className="icon-button" aria-label={`Delete ${collection.name}`} onClick={() => { setDeleting(collection); setError(null); }}><Icon name="delete" /></button>
      </li>)}
    </ul>
    {editing !== undefined && <CollectionEditor collection={editing ?? undefined} onClose={(changed) => { setEditing(undefined); if (changed) reload(); }} />}
    {deleting && <Dialog title={`Delete ${deleting.name}?`} onClose={() => setDeleting(null)}><div className="library-form"><p>The assets stay in your library.</p>{error && <p role="alert" className="notice error">{error}</p>}<div className="dialog-actions"><button type="button" className="text-button" onClick={() => setDeleting(null)}>Cancel</button><button type="button" className="primary-button" disabled={busy} onClick={async () => { setBusy(true); try { await api.deleteCollection(deleting.id); setDeleting(null); reload(); notify("Collection deleted."); } catch (reason) { setError(errorMessage(reason)); } finally { setBusy(false); } }}>Delete collection</button></div></div></Dialog>}
  </>;
}
