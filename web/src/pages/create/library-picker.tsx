import { useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { useAssetList } from "../../lib/assets";
import { useModelNames } from "../../lib/catalog";
import { emitLocal } from "../../lib/events";
import { kindPlural } from "../../lib/format";
import { notifyError } from "../../lib/notices";
import type { Asset, MediaKind } from "../../lib/types";
import { Dialog } from "../../ui/dialog";
import { AssetGrid } from "../../ui/grid";
import { assetTitle } from "../../lib/names";
import { Icon, Spinner } from "../../ui/icon";

/** Pick inputs from the library, or import one from a link. */
export function LibraryPicker({ kind, room, title, onPick, onClose }: { kind: MediaKind; room: number; title: string; onPick(assets: Asset[]): void; onClose(): void }) {
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Map<string, Asset>>(new Map());
  const [link, setLink] = useState("");
  const [importing, setImporting] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const list = useAssetList({ kind, q: query || undefined });
  const nameOf = useModelNames([kind]);
  const multiple = room > 1;

  useEffect(() => {
    const timer = setTimeout(() => setQuery(search.trim()), 250);
    return () => clearTimeout(timer);
  }, [search]);

  const finish = (assets: Asset[]) => {
    onPick(assets.slice(0, room));
    onClose();
  };

  const importLink = async () => {
    if (!link.trim() || importing) return;
    setImporting(true);
    try {
      const { assets } = await api.importAsset(link.trim());
      assets.forEach((asset) => emitLocal({ asset }));
      const fitting = assets.filter((asset) => asset.kind === kind);
      if (!fitting.length) notifyError(`That link has no ${kindPlural[kind]}.`);
      else finish(fitting);
    } catch (error) {
      notifyError(errorMessage(error));
    } finally {
      setImporting(false);
    }
  };

  return (
    <Dialog title={title} size="wide" onClose={onClose} className="picker">
      <div className="picker-toolbar">
        <label className="search">
          <Icon name="search" />
          <span className="sr-only">Search {kindPlural[kind]}</span>
          <input type="search" value={search} placeholder={`Search ${kindPlural[kind]} by prompt`} onChange={(event) => setSearch(event.target.value)} />
        </label>
        {/* Not a <form>: the picker opens inside the composer's form. */}
        <div className="import-link">
          <label className="search">
            <Icon name="link" />
            <span className="sr-only">Import from a link</span>
            <input
              type="url"
              value={link}
              placeholder="Or paste a link to import"
              onChange={(event) => setLink(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void importLink();
                }
              }}
            />
          </label>
          <button type="button" className="text-button" disabled={!link.trim() || importing} onClick={() => void importLink()}>
            {importing ? <Spinner /> : <Icon name="download" />}
            Import
          </button>
        </div>
      </div>
      <div className="picker-body" ref={scrollRef}>
        {list.items.length ? (
          <AssetGrid
            items={list.items}
            size={150}
            scroller={scrollRef}
            label={`Your ${kindPlural[kind]}`}
            titleOf={(asset) => assetTitle(asset, nameOf)}
            selectable={multiple}
            selected={new Set(selected.keys())}
            hasMore={list.hasMore}
            loading={list.loading}
            onEndReached={list.loadMore}
            onOpen={(asset) => {
              if (!multiple) return finish([asset]);
              setSelected((current) => {
                const next = new Map(current);
                if (next.has(asset.id)) next.delete(asset.id);
                else if (next.size < room) next.set(asset.id, asset);
                return next;
              });
            }}
          />
        ) : (
          <div className="empty picker-empty" role="status">
            {list.loading ? <Spinner /> : <Icon name={query ? "search_off" : "photo_library"} />}
            <p>{list.loading ? "Loading" : list.error ? list.error : query ? "Nothing matches that search." : `No ${kindPlural[kind]} yet. Upload one or import a link.`}</p>
          </div>
        )}
      </div>
      {multiple && (
        <div className="picker-footer">
          <span>
            {selected.size} of {room} selected
          </span>
          <button type="button" className="primary-button" disabled={!selected.size} onClick={() => finish([...selected.values()])}>
            Add
            <Icon name="add" />
          </button>
        </div>
      )}
    </Dialog>
  );
}
