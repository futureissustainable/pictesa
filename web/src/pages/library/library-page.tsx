import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { toggleFavorite, useAssetList } from "../../lib/assets";
import { useModelNames, useModels } from "../../lib/catalog";
import { kindIcon } from "../../lib/format";
import { Link, navigate, useLocation, withParams } from "../../lib/router";
import { MAX_CHARACTER_IMAGES } from "../../lib/characters";
import { assetTitle } from "../../lib/names";
import { notify } from "../../lib/notices";
import type { Asset, AssetQuery, AssetSource, MediaKind } from "../../lib/types";
import { openViewer, useSequence } from "../../lib/viewer";
import { Choice, Control } from "../../ui/control";
import { AssetGrid } from "../../ui/grid";
import { Icon, Spinner } from "../../ui/icon";
import { SizeSlider, useImageSize } from "../../ui/size-slider";

const CharacterEditor = lazy(() => import("../characters/characters-page").then((module) => ({ default: module.CharacterEditor })));

const kindLabelPlural: Record<MediaKind, string> = { image: "Images", video: "Videos", audio: "Audio" };

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
  // Selecting images to save them as a character.
  const [selecting, setSelecting] = useState(false);
  const [picked, setPicked] = useState<Map<string, Asset>>(new Map());
  const [saving, setSaving] = useState(false);
  const stopSelecting = () => {
    setSelecting(false);
    setPicked(new Map());
  };
  const togglePick = (asset: Asset) => {
    if (asset.kind !== "image") return notify("Characters are made of images.");
    setPicked((current) => {
      const next = new Map(current);
      if (next.has(asset.id)) next.delete(asset.id);
      else if (next.size < MAX_CHARACTER_IMAGES) next.set(asset.id, asset);
      else notify(`A character holds up to ${MAX_CHARACTER_IMAGES} images.`);
      return next;
    });
  };
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
  const title = query.trash ? "Trash" : query.favorite ? "Favorites" : query.source === "upload" ? "Uploads" : "Library";
  const intro = query.trash
    ? "Restore what you need, or delete it for good."
    : query.favorite
      ? "The ones worth coming back to."
      : query.source === "upload"
        ? "Your references, all in one place."
        : "Everything you have made, ready for the next step.";
  const filtered = Boolean(query.q || query.kind || query.model || (query.source && query.source !== "upload"));

  return (
    <div className="workspace">
      <div className="intro">
        <div>
          <h1>{title}.</h1>
          <p>{intro}</p>
        </div>
        <button type="button" className="icon-button" aria-label="Refresh" title="Refresh" onClick={list.reload}>
          <Icon name="refresh" />
        </button>
      </div>
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
        {!query.trash && (
          <button type="button" className={`filter-button ${selecting ? "active" : ""}`} aria-pressed={selecting} onClick={() => (selecting ? stopSelecting() : setSelecting(true))} title="Select images to save as a character">
            <Icon name="checklist" />
            <span className="segmented-label">Select</span>
          </button>
        )}
        {/* Phones have no sidebar: characters and trash sit behind one button. */}
        <span className="mobile-only">
          <Control label="More" icon="more_horiz" caret={false} placement="below">
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
          <button type="button" className="primary-button" onClick={list.reload}>
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
            <button type="button" className="primary-button" onClick={() => set({ q: null, kind: null, model: null, source: query.source === "upload" ? "upload" : null })}>
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
        <div className="selection-bar" role="region" aria-label="Selection">
          <p>
            {picked.size ? `${picked.size} selected` : "Select images"}
            <span className="selection-limit">, up to {MAX_CHARACTER_IMAGES}</span>
          </p>
          <button type="button" className="text-button" onClick={stopSelecting}>
            Cancel
          </button>
          <button type="button" className="primary-button" disabled={!picked.size} onClick={() => setSaving(true)}>
            <Icon name="face" />
            Save as character
          </button>
        </div>
      )}
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
