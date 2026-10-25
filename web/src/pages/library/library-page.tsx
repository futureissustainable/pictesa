import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { toggleFavorite, useAssetList } from "../../lib/assets";
import { useModelNames, useModels, useProviders } from "../../lib/catalog";
import { kindIcon } from "../../lib/format";
import { Link, navigate, useLocation, withParams } from "../../lib/router";
import { MAX_CHARACTER_IMAGES } from "../../lib/characters";
import { assetTitle } from "../../lib/names";
import { subscribeEvents } from "../../lib/events";
import { api, downloadZip, errorMessage } from "../../lib/api";
import { notify, notifyError } from "../../lib/notices";
import type { Asset, AssetQuery, AssetSource, Collection, MediaKind } from "../../lib/types";
import { openViewer, useSequence } from "../../lib/viewer";
import { Choice, Control } from "../../ui/control";
import { Dialog } from "../../ui/dialog";
import { Collections } from "./collections";
import { AssetGrid } from "../../ui/grid";
import { Icon, Spinner } from "../../ui/icon";
import { SizeSlider, useImageSize } from "../../ui/size-slider";

const CharacterEditor = lazy(() => import("../characters/characters-page").then((module) => ({ default: module.CharacterEditor })));

const kindLabelPlural: Record<MediaKind, string> = { image: "Images", video: "Videos", audio: "Audio", lora: "LoRAs" };

const sources: { value: AssetSource | ""; label: string }[] = [
  { value: "", label: "All sources" },
  { value: "generated", label: "Generated" },
  { value: "upload", label: "Uploaded" },
  { value: "import", label: "Imported" },
];

function queryFrom(url: URL): AssetQuery {
  const params = url.searchParams;
  const kind = params.get("kind");
  const source = params.get("source");
  return {
    kind: kind === "image" || kind === "video" || kind === "audio" ? kind : undefined,
    favorite: params.get("favorite") === "true" || undefined,
    source: source === "generated" || source === "upload" || source === "import" ? source : undefined,
    model: params.get("model") || undefined,
    collection: params.get("collection") || undefined,
    tag: params.get("tag") || undefined,
    provider: params.get("provider") || undefined,
    cost_min: params.has("cost_min") ? Number(params.get("cost_min")) : undefined,
    cost_max: params.has("cost_max") ? Number(params.get("cost_max")) : undefined,
    trash: params.get("trash") === "true" || undefined,
    q: params.get("q") || undefined,
  };
}

export default function LibraryPage() {
  const url = useLocation();
  const query = useMemo(() => queryFrom(url), [url]);
  const list = useAssetList(query);
  const size = useImageSize();
  const nameOf = useModelNames(["image", "video", "audio"]);
  const audio = useModels("audio");
  const [search, setSearch] = useState(query.q ?? "");
  const searchRef = useRef<HTMLInputElement>(null);
  const providers = useProviders().data ?? [];
  const [collections, setCollections] = useState<Collection[]>([]);
  const [tags, setTags] = useState<string[]>([]);
  const [organizationError, setOrganizationError] = useState<string | null>(null);
  const loadOrganization = () => {
    setOrganizationError(null);
    Promise.all([api.collections(), api.tags()]).then(([groups, labels]) => {
      setCollections(groups.collections); setTags(labels.tags);
    }).catch((error) => setOrganizationError(errorMessage(error)));
  };
  useEffect(() => { loadOrganization(); return subscribeEvents({ library: loadOrganization }); }, []);
  const [tagging, setTagging] = useState(false);
  const [tag, setTag] = useState("");
  const [busy, setBusy] = useState(false);
  const [selectionTags, setSelectionTags] = useState<string[]>([]);
  const showCollections = url.searchParams.get("collections") === "true";
  const currentCollection = collections.find((item) => item.id === query.collection);
  // One selection mode serves organization, downloads and character creation.
  const [selecting, setSelecting] = useState(false);
  const [picked, setPicked] = useState<Map<string, Asset>>(new Map());
  const [saving, setSaving] = useState(false);
  const stopSelecting = () => {
    setSelecting(false);
    setPicked(new Map());
  };
  const togglePick = (asset: Asset) => {
    if (!picked.has(asset.id) && picked.size >= 100) return notify("Select up to 100 assets.");
    setPicked((current) => {
      const next = new Map(current);
      if (next.has(asset.id)) next.delete(asset.id);
      else next.set(asset.id, asset);
      return next;
    });
  };
  // Trashed images cannot become a character: opening the trash ends a selection.
  if (query.trash && selecting) stopSelecting();
  useEffect(() => { setPicked(new Map()); setSelecting(false); }, [url.pathname, url.search]);
  const perform = async (action: () => Promise<unknown>, message: string, end = true) => {
    setBusy(true);
    try { await action(); notify(message); list.reload(); loadOrganization(); if (end) stopSelecting(); }
    catch (error) { notifyError(errorMessage(error)); }
    finally { setBusy(false); }
  };
  const ids = [...picked.keys()];
  const canSaveCharacter = picked.size > 0 && picked.size <= MAX_CHARACTER_IMAGES && [...picked.values()].every((asset) => asset.kind === "image");
  useSequence(list.items, list.hasMore, list.loadMore);

  // Typing updates the URL after a short pause; the URL is the source of truth for filters.
  useEffect(() => {
    if ((query.q ?? "") === search.trim()) return;
    const timer = setTimeout(() => navigate(withParams({ q: search.trim() || null }), { replace: true }), 250);
    return () => clearTimeout(timer);
  }, [search, query.q]);
  useEffect(() => setSearch(query.q ?? ""), [query.q]);

  // "/" jumps to search, as in most libraries.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;
      if (event.key === "/" && !event.metaKey && !event.ctrlKey && !["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) && !target.isContentEditable && !document.querySelector("dialog[open]")) {
        event.preventDefault();
        searchRef.current?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const set = (changes: Record<string, string | null>) => navigate(withParams(changes), { replace: true });
  const kinds: (MediaKind | "")[] = audio.data?.length ? ["", "image", "video", "audio"] : ["", "image", "video"];
  const title = showCollections ? "Collections" : query.collection ? currentCollection?.name ?? "Collection" : query.trash ? "Trash" : query.favorite ? "Favorites" : query.source === "upload" ? "Uploads" : "Library";
  const intro = showCollections ? "Named groups for your work." : query.collection ? "Select assets to organize this collection." : query.trash
    ? "Restore what you need, or delete it for good."
    : query.favorite
      ? "The ones worth coming back to."
      : query.source === "upload"
        ? "Your references, all in one place."
        : "Everything you have made, ready for the next step.";
  const filtered = Boolean(query.q || query.kind || query.model || query.provider || query.tag || query.cost_min !== undefined || query.cost_max !== undefined || (query.source && query.source !== "upload"));

  return (
    <div className="workspace">
      <div className="intro">
        <div>
          <h1>{title}.</h1>
          <p>{intro}</p>
        </div>
        <button type="button" className="icon-button" aria-label="Refresh" title="Refresh" onClick={() => { list.reload(); loadOrganization(); }}>
          <Icon name="refresh" />
        </button>
      </div>
      {organizationError && <p className="notice error" role="alert">{organizationError} <button type="button" className="text-button" onClick={loadOrganization}>Try again</button></p>}
      {showCollections ? <Collections items={collections} reload={loadOrganization} /> : <>
      {query.collection && <Link className="text-button" to="/library?collections=true"><Icon name="arrow_back" />All collections</Link>}
      <div className="gallery-toolbar">
        <SizeSlider />
        <div className="segmented" role="group" aria-label="Kind">
          {kinds.map((kind) => (
            <button
              key={kind || "all"}
              type="button"
              aria-pressed={(query.kind ?? "") === kind}
              aria-label={kind ? kindLabelPlural[kind] : "Everything"}
              title={kind ? kindLabelPlural[kind] : "Everything"}
              onClick={() => set({ kind: kind || null })}
            >
              {kind ? <Icon name={kindIcon[kind]} filled={query.kind === kind} /> : "All"}
            </button>
          ))}
        </div>
        {!query.trash && (
          <Control label="Source" icon="filter_list" value={sources.find((item) => item.value === (query.source ?? ""))?.label} className={query.source ? "has-changes" : ""}>
            {sources.map((item) => (
              <Choice key={item.value || "all"} selected={(query.source ?? "") === item.value} onPick={() => set({ source: item.value || null })}>
                {item.label}
              </Choice>
            ))}
          </Control>
        )}
        <ModelFilter value={query.model} nameOf={nameOf} onChange={(model) => set({ model })} />
        <Control label="Filters" icon="filter_list" value={query.tag || query.provider || (query.cost_min !== undefined || query.cost_max !== undefined ? "Cost" : "All")} className={query.tag || query.provider || query.cost_min !== undefined || query.cost_max !== undefined ? "has-changes" : ""}>
          <div className="more-fields library-filters">
            <label className="field"><span>Provider</span><select aria-label="Provider" value={query.provider ?? ""} onChange={(event) => set({ provider: event.target.value || null })}><option value="">All providers</option>{providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.name}</option>)}</select></label>
            <label className="field"><span>Tag</span><select aria-label="Tag" value={query.tag ?? ""} onChange={(event) => set({ tag: event.target.value || null })}><option value="">All tags</option>{[...new Set([...tags, ...(query.tag ? [query.tag] : [])])].map((tag) => <option key={tag} value={tag}>{tag}</option>)}</select></label>
            <label className="field"><span>Minimum cost (USD)</span><input type="number" min="0" step="any" value={query.cost_min ?? ""} onChange={(event) => set({ cost_min: event.target.value || null })} /></label>
            <label className="field"><span>Maximum cost (USD)</span><input type="number" min="0" step="any" value={query.cost_max ?? ""} onChange={(event) => set({ cost_max: event.target.value || null })} /></label>
            <button type="button" className="text-button" onClick={() => set({ provider: null, tag: null, cost_min: null, cost_max: null })}>Clear filters</button>
          </div>
        </Control>
        {!query.trash && (
          <button type="button" className={`filter-button ${selecting ? "active" : ""}`} aria-pressed={selecting} onClick={() => (selecting ? stopSelecting() : setSelecting(true))} title="Select assets for bulk actions">
            <Icon name="checklist" />
            <span className="segmented-label">Select</span>
          </button>
        )}
        {/* Phones have no sidebar: characters and trash sit behind one button. */}
        <span className="mobile-only">
          <Control label="More" icon="more_horiz" caret={false} placement="below">
            <Choice icon="folder_open" onPick={() => navigate("/library?collections=true")}>Collections</Choice>
            <Choice icon="face" onPick={() => navigate("/characters")}>
              Characters
            </Choice>
            <Choice icon="delete" selected={Boolean(query.trash)} onPick={() => navigate(query.trash ? "/library" : "/library?trash=true")}>
              Trash
            </Choice>
          </Control>
        </span>
        <label className="search">
          <Icon name="search" />
          <span className="sr-only">Search prompts</span>
          <input
            ref={searchRef}
            type="search"
            value={search}
            placeholder="Search prompts"
            aria-keyshortcuts="/"
            onChange={(event) => setSearch(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape" && search) {
                event.preventDefault();
                setSearch("");
              }
            }}
          />
        </label>
      </div>
      {list.items.length ? (
        <AssetGrid
          items={list.items}
          size={size}
          label={title}
          titleOf={(asset) => assetTitle(asset, nameOf)}
          fresh={list.fresh}
          hasMore={list.hasMore}
          loading={list.loading}
          onEndReached={list.loadMore}
          selectable={selecting}
          selected={selecting ? new Set(picked.keys()) : undefined}
          onOpen={(asset) => (selecting ? togglePick(asset) : openViewer(asset))}
          onFavorite={query.trash ? undefined : (asset) => void toggleFavorite(asset)}
        />
      ) : list.loading ? (
        <div className="empty" role="status">
          <Spinner />
          <p>Loading</p>
        </div>
      ) : list.error ? (
        <div className="empty" role="alert">
          <Icon name="cloud_off" />
          <h2>The library did not load.</h2>
          <p>{list.error}</p>
          <button type="button" className="primary-button" onClick={() => { list.reload(); loadOrganization(); }}>
            Try again
            <Icon name="refresh" />
          </button>
        </div>
      ) : (
        <div className="empty">
          <Icon name={filtered ? "search_off" : query.trash ? "delete" : query.favorite ? "favorite" : "photo_library"} />
          <h2>{filtered ? "Nothing matches." : query.trash ? "Nothing in the trash." : query.favorite ? "No favorites yet." : "A little empty, for now."}</h2>
          <p>
            {filtered
              ? "Try another word or clear the filters."
              : query.favorite
                ? "Tap the heart on anything you want to keep close."
                : query.trash
                  ? "Deleted items wait here until you remove them for good."
                  : "Everything you make or upload appears here."}
          </p>
          {filtered ? (
            <button type="button" className="primary-button" onClick={() => set({ q: null, kind: null, model: null, provider: null, tag: null, cost_min: null, cost_max: null, source: query.source === "upload" ? "upload" : null })}>
              Clear filters
              <Icon name="close" />
            </button>
          ) : (
            !query.trash && (
              <Link className="primary-button" to="/">
                Create something
                <Icon name="arrow_forward" />
              </Link>
            )
          )}
        </div>
      )}
      {selecting && (
        <div className="selection-bar library-selection" role="region" aria-label="Selection" aria-busy={busy}>
          <p>{picked.size} selected<span className="selection-limit">, up to 100</span></p>
          <button type="button" className="text-button" disabled={busy} onClick={() => setPicked(new Map(list.items.slice(0, 100).map((item) => [item.id, item])))}>Select loaded</button>
          <fieldset disabled={busy || !picked.size} className="bulk-actions">
            <Control label="Actions" icon="more_horiz" placement="above">
              <Choice icon="favorite" onPick={() => void perform(() => api.bulkAssets(ids, "favorite"), "Added to favorites.")}>Favorite</Choice>
              <Choice onPick={() => void perform(() => api.bulkAssets(ids, "unfavorite"), "Removed from favorites.")}>Remove favorite</Choice>
              <Choice onPick={() => { setTag(""); setTagging(true); setSelectionTags([]); if (ids.length === 1) void api.assetTags(ids[0]!).then((data) => setSelectionTags(data.tags)).catch((error) => notifyError(errorMessage(error))); }}>Tag</Choice>
              <Choice icon="delete" onPick={() => void perform(() => api.bulkAssets(ids, "trash"), "Moved to trash.")}>Move to trash</Choice>
              <Choice icon="download" onPick={() => void perform(() => downloadZip(ids), "ZIP downloaded.")}>Download ZIP</Choice>
              {query.collection && <Choice onPick={() => void perform(() => api.collectionAssets(query.collection!, ids, true), "Removed from collection.")}>Remove from collection</Choice>}
              <Choice icon="face" disabled={!canSaveCharacter} onPick={() => setSaving(true)}>Save as character (up to {MAX_CHARACTER_IMAGES} images)</Choice>
            </Control>
            <Control label="Add to collection" icon="folder_open" placement="above">
              {collections.length ? collections.map((collection) => <Choice key={collection.id} onPick={() => void perform(() => api.collectionAssets(collection.id, ids), "Added to collection.")}>{collection.name}</Choice>) : <p className="hint">Create a collection from Collections first.</p>}
            </Control>
          </fieldset>
          <button type="button" className="text-button" disabled={busy} onClick={stopSelecting}>Cancel</button>
        </div>
      )}
      {tagging && <Dialog title="Tag selected assets" onClose={() => setTagging(false)}>
        <div className="library-form">
          <label className="field"><span>Tag</span><input autoFocus list="library-tags" maxLength={60} value={tag} onChange={(event) => setTag(event.target.value)} /></label>
          <datalist id="library-tags">{tags.map((item) => <option key={item} value={item} />)}</datalist>
          {selectionTags.length > 0 && <p className="hint">Current tags: {selectionTags.join(", ")}</p>}
          <div className="dialog-actions">
            <button type="button" className="text-button" disabled={busy || !tag.trim()} onClick={() => void perform(async () => { await api.tagAssets(ids, tag, true); setTagging(false); }, "Tag removed.")}>Remove tag</button>
            <button type="button" className="primary-button" disabled={busy || !tag.trim()} onClick={() => void perform(async () => { await api.tagAssets(ids, tag); setTagging(false); }, "Tag added.")}>Add tag</button>
          </div>
        </div>
      </Dialog>}
      {saving && (
        <Suspense fallback={null}>
          <CharacterEditor
            character={null}
            initial={[...picked.values()]}
            onClose={(saved) => {
              setSaving(false);
              if (saved) stopSelecting();
            }}
          />
        </Suspense>
      )}
      {list.loading && list.items.length > 0 && (
        <div className="load-more" role="status">
          <Spinner />
          <span className="sr-only">Loading more</span>
        </div>
      )}
      </>}
    </div>
  );
}

function ModelFilter({ value, nameOf, onChange }: { value?: string; nameOf(id: string | null | undefined): string; onChange(model: string | null): void }) {
  const image = useModels("image");
  const video = useModels("video");
  const audio = useModels("audio");
  const [q, setQ] = useState("");
  const all = [...(image.data ?? []), ...(video.data ?? []), ...(audio.data ?? [])];
  const matches = all.filter((model) => `${model.name} ${model.vendor ?? ""} ${model.provider}`.toLowerCase().includes(q.trim().toLowerCase())).slice(0, 60);
  return (
    <Control label="Model" value={value ? nameOf(value) : "All models"} className={value ? "has-changes" : ""} size="wide" onOpenChange={(open) => !open && setQ("")}>
      <label className="search control-search">
        <Icon name="search" />
        <span className="sr-only">Search models</span>
        <input type="search" value={q} placeholder="Search models" data-autofocus="" onChange={(event) => setQ(event.target.value)} />
      </label>
      <div className="control-list">
        <Choice selected={!value} onPick={() => onChange(null)}>
          All models
        </Choice>
        {matches.map((model) => (
          <Choice key={model.id} selected={value === model.id} onPick={() => onChange(model.id)}>
            {model.name}
            <small>{model.vendor ?? model.provider}</small>
          </Choice>
        ))}
      </div>
    </Control>
  );
}
