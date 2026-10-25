import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { api, errorMessage, isAbort, withQuery } from "../../lib/api";
import { deleteForever, restoreAsset, toggleFavorite, trashAsset } from "../../lib/assets";
import { fallbackModelName, useFullModel, useModels, usePresets } from "../../lib/catalog";
import { subscribeEvents } from "../../lib/events";
import { bytes, clock, dimensions, formatDate, formatDateTime, kindLabel, money, optionLabel, roleLabel } from "../../lib/format";
import { prefersReducedMotion } from "../../lib/prefs";
import { inputUses, reuseSettings, sendToCreate } from "../../lib/reuse";
import { Link, navigate } from "../../lib/router";
import { useSession } from "../../lib/session";
import { cliCommand, curlCommand, requestFromJob } from "../../lib/snippets";
import { useStore } from "../../lib/store";
import type { Asset, AssetDetail, Job, ModelSpec, ParamValue } from "../../lib/types";
import { closeViewer, sequence, stepViewer } from "../../lib/viewer";
import { assetTitle } from "../../lib/names";
import { CodeBlock, CopyButton } from "../../ui/code";
import { Choice, Control } from "../../ui/control";
import { Dialog } from "../../ui/dialog";
import { Icon, Spinner } from "../../ui/icon";

/** Full-size viewer over any page (`?asset=<id>`). Arrow keys and swipes step through the page's list. */
export default function Viewer({ id }: { id: string }) {
  const seq = useStore(sequence);
  const [detail, setDetail] = useState<AssetDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const listed = seq?.items.find((item) => item.id === id) ?? null;
  const asset: Asset | null = detail?.id === id ? detail : listed;
  const job = detail?.id === id ? detail.job : null;
  const index = seq ? seq.items.findIndex((item) => item.id === id) : -1;
  const previous = index > 0 ? seq!.items[index - 1] : undefined;
  const next = index >= 0 && seq ? seq.items[index + 1] : undefined;

  useEffect(() => {
    const controller = new AbortController();
    setError(null);
    api
      .asset(id, controller.signal)
      .then(setDetail)
      .catch((reason: unknown) => {
        if (!isAbort(reason)) setError(errorMessage(reason));
      });
    return () => controller.abort();
  }, [id]);

  // Favorite, trash and restore changes made anywhere show up here.
  useEffect(
    () =>
      subscribeEvents({
        asset: (changed) => {
          if (changed.id === id) setDetail((current) => (current && current.id === id ? { ...current, ...changed } : current));
        },
        removed: (removedId) => {
          if (removedId === id) closeViewer();
        },
      }),
    [id],
  );

  // Near the end of a loaded list, fetch the next page so the right arrow keeps working.
  useEffect(() => {
    if (seq && index >= 0 && index >= seq.items.length - 3 && seq.hasMore) seq.loadMore();
  }, [seq, index]);

  const onKeyDown = (event: KeyboardEvent<HTMLDialogElement>) => {
    const target = event.target as HTMLElement;
    if (["INPUT", "TEXTAREA", "SELECT", "VIDEO", "AUDIO"].includes(target.tagName) || target.closest(":popover-open")) return;
    if (event.key === "ArrowLeft" && previous) {
      event.preventDefault();
      stepViewer(previous.id);
    } else if (event.key === "ArrowRight" && next) {
      event.preventDefault();
      stepViewer(next.id);
    }
  };

  const swipe = useRef<{ x: number; y: number } | null>(null);
  const onPointerDown = (event: PointerEvent) => {
    if (event.pointerType !== "mouse") swipe.current = { x: event.clientX, y: event.clientY };
  };
  const onPointerUp = (event: PointerEvent) => {
    const start = swipe.current;
    swipe.current = null;
    if (!start) return;
    const dx = event.clientX - start.x;
    if (Math.abs(dx) < 60 || Math.abs(dx) < Math.abs(event.clientY - start.y) * 1.5) return;
    if (dx > 0 && previous) stepViewer(previous.id);
    if (dx < 0 && next) stepViewer(next.id);
  };

  const title = job?.model_name ?? (asset ? assetTitle(asset, (id) => fallbackModelName(id ?? "")) : "Loading");

  return (
    <Dialog title={title} size="full" hideTitle onClose={closeViewer} onKeyDown={onKeyDown} className="viewer">
      <div className="viewer-layout">
        <div className="viewer-stage" onPointerDown={onPointerDown} onPointerUp={onPointerUp} onPointerCancel={() => (swipe.current = null)}>
          {asset ? <Media key={asset.id} asset={asset} /> : error ? <p className="viewer-missing">{error}</p> : <Spinner />}
          {previous && (
            <button type="button" className="viewer-nav viewer-prev" aria-label="Previous" title="Previous (Left arrow)" onClick={() => stepViewer(previous.id)}>
              <Icon name="chevron_left" />
            </button>
          )}
          {next && (
            <button type="button" className="viewer-nav viewer-next" aria-label="Next" title="Next (Right arrow)" onClick={() => stepViewer(next.id)}>
              <Icon name="chevron_right" />
            </button>
          )}
        </div>
        <aside className="viewer-info" aria-label="Details">
          <header className="viewer-header">
            <p className="viewer-title">
              {asset?.model ? (
                <Link to={`/library?model=${encodeURIComponent(asset.model)}`} replace title="Show everything from this model">
                  {title}
                </Link>
              ) : (
                title
              )}
            </p>
            <button type="button" className="icon-button" aria-label="Close" onClick={closeViewer}>
              <Icon name="close" />
            </button>
          </header>
          {asset && <Details asset={asset} job={job} loading={!detail && !error} />}
        </aside>
      </div>
    </Dialog>
  );
}

function Media({ asset }: { asset: Asset }) {
  if (asset.kind === "video") {
    return <video className="viewer-media" src={asset.url} poster={asset.thumb_url ?? undefined} controls loop playsInline preload="metadata" autoPlay={!prefersReducedMotion()} />;
  }
  if (asset.kind === "audio") {
    return (
      <div className="viewer-audio">
        <Icon name="graphic_eq" />
        <audio src={asset.url} controls preload="metadata" />
      </div>
    );
  }
  return (
    <img
      className="viewer-media"
      src={asset.url}
      alt={asset.prompt ?? kindLabel[asset.kind]}
      decoding="async"
      style={asset.thumb_url ? { backgroundImage: `url("${asset.thumb_url}")` } : undefined}
    />
  );
}

function Details({ asset, job, loading }: { asset: Asset; job: Job | null; loading: boolean }) {
  const session = useSession();
  const hosted = session.status === "ready" && session.me.mode === "hosted";
  const image = useModels("image");
  const video = useModels("video");
  const audio = useModels("audio");
  const presets = usePresets(job?.output ?? asset.kind).data;
  const [working, setWorking] = useState(false);
  const models: ModelSpec[] = useMemo(() => [...(image.data ?? []), ...(video.data ?? []), ...(audio.data ?? [])], [image.data, video.data, audio.data]);
  const listedModel = models.find((model) => model.id === (job?.model ?? asset.model));
  const spec = useFullModel(listedModel).spec;
  const uses = useMemo(() => inputUses(asset.kind), [asset.kind, models]);
  const trashed = Boolean(asset.deleted_at);
  const request = job ? requestFromJob(job) : null;
  const preset = job?.preset ? (presets?.find((item) => item.id === job.preset)?.name ?? job.preset) : null;

  const run = async (action: () => Promise<unknown>) => {
    setWorking(true);
    try {
      await action();
    } finally {
      setWorking(false);
    }
  };

  const params = job
    ? Object.entries(job.params ?? {}).filter(([key, value]) => value !== null && value !== "" && key !== "width" && key !== "height")
    : Object.entries(asset.params ?? {}).filter(([, value]) => value !== null && value !== "");

  return (
    <div className="viewer-body">
      <div className="viewer-actions">
        {!trashed && (
          <button
            type="button"
            className={`icon-button ${asset.favorite ? "selected" : ""}`}
            aria-pressed={asset.favorite}
            aria-label={asset.favorite ? "Remove from favorites" : "Add to favorites"}
            title="Favorite"
            onClick={() => void toggleFavorite(asset)}
          >
            <Icon name="favorite" filled={asset.favorite} />
          </button>
        )}
        <a className="icon-button" href={withQuery(asset.url, { download: 1 })} download aria-label="Download original" title="Download original">
          <Icon name="download" />
        </a>
        {!trashed ? (
          <button type="button" className="icon-button" aria-label="Move to trash" title="Move to trash" onClick={() => void run(async () => (await trashAsset(asset)) && closeViewer())}>
            <Icon name="delete" />
          </button>
        ) : null}
      </div>

      {trashed ? (
        <div className="notice viewer-trash">
          <p>In the trash since {formatDate(asset.deleted_at!)}.</p>
          <div className="detail-actions">
            <button type="button" className="text-button" disabled={working} onClick={() => void run(() => restoreAsset(asset))}>
              <Icon name="restore_from_trash" />
              Restore
            </button>
            <button
              type="button"
              className="text-button destructive"
              disabled={working}
              onClick={() => {
                if (confirm("Delete this file for good? This cannot be undone.")) void run(() => deleteForever(asset));
              }}
            >
              <Icon name="delete_forever" />
              Delete for good
            </button>
          </div>
        </div>
      ) : (
        <div className="detail-actions">
          {(job || listedModel) && (
            <button type="button" className="text-button" disabled={working} onClick={() => void run(() => reuseSettings({ ...asset, job }))}>
              <Icon name="replay" />
              Reuse settings
            </button>
          )}
          {(asset.kind === "image" || asset.kind === "video") && (
            <button type="button" className="text-button" onClick={() => navigate(`/edit?${asset.kind === "video" ? "video" : "image"}=${encodeURIComponent(asset.id)}`)}>
              <Icon name="edit" />
              {asset.kind === "video" ? "Edit video" : "Edit"}
            </button>
          )}
          {asset.kind === "image" && uses.some((use) => use.mode === "video" && use.role === "start_frame") && (
            <button type="button" className="text-button" disabled={working} onClick={() => void run(() => sendToCreate(asset, "video", "start_frame"))}>
              <Icon name="animation" />
              Animate
            </button>
          )}
          {uses.length > 0 && (
            <Control label="Use as input" icon="add_photo_alternate" value="Use as" className="text-control">
              {uses.map((use) => (
                <Choice key={`${use.mode}-${use.role}`} onPick={() => void run(() => sendToCreate(asset, use.mode, use.role))}>
                  {use.label}
                </Choice>
              ))}
            </Control>
          )}
          {request && (
            <Control label="Copy for agents" icon="terminal" value="Copy for agents" caret={false} size="large" className="text-control" panelClassName="agent-panel">
              <p className="control-help">Run this generation again from a terminal or an agent.</p>
              <CodeBlock code={cliCommand(request)} label="CLI command" caption="Pictesa CLI" />
              <CodeBlock code={curlCommand(request, hosted)} label="curl command" caption="HTTP API" />
            </Control>
          )}
        </div>
      )}

      {loading && !job && asset.source === "generated" ? (
        <p className="viewer-loading" role="status">
          <Spinner />
          Loading details
        </p>
      ) : null}

      {(job?.prompt || asset.prompt) && (
        <section className="detail-prompt" aria-label="Prompt">
          <p>{job?.prompt || asset.prompt}</p>
          <CopyButton text={job?.prompt || asset.prompt || ""} label="Copy prompt" />
        </section>
      )}
      {(job?.negative_prompt || asset.negative_prompt) && (
        <p className="detail-negative">
          <span>Avoid</span> {job?.negative_prompt || asset.negative_prompt}
        </p>
      )}

      <dl className="detail-data">
        {asset.origin && <div><dt>Origin</dt><dd>{asset.origin}</dd></div>}
        {!job && asset.model && <div><dt>Model ID</dt><dd>{asset.model}</dd></div>}
        {asset.source_url && <div><dt>Source</dt><dd>{/^https?:\/\//i.test(asset.source_url) ? <a href={asset.source_url} target="_blank" rel="noreferrer">{asset.source_url}</a> : asset.source_url}</dd></div>}
        <div>
          <dt>Created</dt>
          <dd>{formatDateTime(asset.created_at)}</dd>
        </div>
        {dimensions(asset.width, asset.height) && (
          <div>
            <dt>Dimensions</dt>
            <dd>{dimensions(asset.width, asset.height)}</dd>
          </div>
        )}
        {asset.duration_ms ? (
          <div>
            <dt>Length</dt>
            <dd>{clock(asset.duration_ms)}</dd>
          </div>
        ) : null}
        <div>
          <dt>File</dt>
          <dd>
            {bytes(asset.size)}, {(asset.mime.split("/")[1] ?? asset.mime).toUpperCase()}
          </dd>
        </div>
        {typeof job?.cost_usd === "number" && (
          <div>
            <dt>Cost</dt>
            <dd>{money(job.cost_usd)}</dd>
          </div>
        )}
        {preset && (
          <div>
            <dt>Preset</dt>
            <dd>{preset}</dd>
          </div>
        )}
        {params.map(([key, value]) => {
          const paramSpec = spec?.params.find((item) => item.key === key);
          return (
            <div key={key}>
              <dt>{paramSpec?.label ?? key.charAt(0).toUpperCase() + key.slice(1).replaceAll("_", " ")}</dt>
              <dd>{paramSpec && (value === null || ["string", "number", "boolean"].includes(typeof value)) ? optionLabel(paramSpec, value as ParamValue) : typeof value === "object" ? JSON.stringify(value) : String(value)}</dd>
            </div>
          );
        })}
      </dl>

      {asset.made_from && (
        <section className="detail-inputs" aria-label="Made from">
          <h3>Made from</h3>
          <ul>
            <li>
              <button type="button" className="detail-input" onClick={() => stepViewer(asset.made_from!)} aria-label="Open the original image">
                <img src={`/thumbs/${encodeURIComponent(asset.made_from)}`} alt="" loading="lazy" onError={(event) => (event.currentTarget.style.visibility = "hidden")} />
                <span>Original</span>
              </button>
            </li>
          </ul>
        </section>
      )}
      {job && job.inputs?.length > 0 && (
        <section className="detail-inputs" aria-label="Inputs">
          <h3>Inputs</h3>
          <ul>
            {job.inputs.map((input) => (
              <li key={`${input.role}-${input.asset_id}`}>
                <button type="button" className="detail-input" onClick={() => stepViewer(input.asset_id)} aria-label={`Open ${roleLabel[input.role].toLowerCase()}`}>
                  <img src={`/thumbs/${encodeURIComponent(input.asset_id)}`} alt="" loading="lazy" onError={(event) => (event.currentTarget.style.visibility = "hidden")} />
                  <span>{roleLabel[input.role]}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
