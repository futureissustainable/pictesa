import { useEffect, useMemo, useRef } from "react";
import { toggleFavorite, useAssetList } from "../../lib/assets";
import { useModelNames, useModels } from "../../lib/catalog";
import { useComposer } from "../../lib/composer";
import { kindPlural, requestedRatio } from "../../lib/format";
import { isActive, jobs, previews } from "../../lib/jobs";
import { useStore } from "../../lib/store";
import { Link } from "../../lib/router";
import { openViewer, useSequence } from "../../lib/viewer";
import { PixelMark } from "../../ui/brand";
import { AssetGrid, type GridPlaceholder } from "../../ui/grid";
import { Icon, Spinner } from "../../ui/icon";
import { SizeSlider, useImageSize } from "../../ui/size-slider";
import { Composer } from "./composer";
import { JobsTray } from "./jobs-tray";
import type { Asset } from "../../lib/types";
import { ModeSwitch } from "./mode-switch";

export default function CreatePage() {
  const { mode } = useComposer();
  const size = useImageSize();
  const feedRef = useRef<HTMLElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const list = useAssetList({ kind: mode, source: "generated" });
  const nameOf = useModelNames([mode]);
  const models = useModels(mode);
  const jobList = useStore(jobs);
  const frames = useStore(previews);
  // Runs in progress hold their place at the top of the feed, at the shape they asked for.
  const placeholders = useMemo<GridPlaceholder[]>(
    () =>
      jobList
        .filter((job) => isActive(job) && job.output === mode)
        .flatMap((job) =>
          Array.from({ length: Math.max(1, job.count || 1) }, (_, index) => ({
            key: `${job.id}-${index}`,
            ratio: requestedRatio(job.params ?? {}),
            progress: job.status === "running" ? job.progress : null,
            // Backends stream the first image of a batch only.
            preview: index === 0 ? frames.get(job.id) : undefined,
          })),
        ),
    [jobList, frames, mode],
  );
  const groups = useMemo(() => {
    const byBatch = new Map<string, { id: string; items: Asset[]; pending: GridPlaceholder[] }>();
    const group = (id: string) => {
      let row = byBatch.get(id);
      if (!row) { row = { id, items: [], pending: [] }; byBatch.set(id, row); }
      return row;
    };
    for (const pending of placeholders) {
      const job = jobList.find((j) => pending.key.startsWith(`${j.id}-`));
      group(job?.batch_id ?? pending.key).pending.push(pending);
    }
    for (const asset of list.items) group(asset.batch_id ?? asset.job_id ?? asset.id).items.push(asset);
    const grouped: { id: string; items: Asset[]; pending: GridPlaceholder[]; batch: boolean }[] = [];
    for (const row of byBatch.values()) {
      const batch = row.items.length + row.pending.length > 1;
      const previous = grouped.at(-1);
      if (!batch && previous && !previous.batch) { previous.items.push(...row.items); previous.pending.push(...row.pending); }
      else grouped.push({ ...row, batch });
    }
    return grouped;
  }, [list.items, placeholders, jobList]);
  const noneConfigured = Boolean(models.data && models.data.length && !models.data.some((model) => model.configured));
  useSequence(list.items, list.hasMore, list.loadMore);

  // Notices float just above the composer.
  useEffect(() => {
    const dock = dockRef.current;
    if (!dock) return;
    const observer = new ResizeObserver(() => document.documentElement.style.setProperty("--dock-height", `${dock.offsetHeight}px`));
    observer.observe(dock);
    return () => {
      observer.disconnect();
      document.documentElement.style.removeProperty("--dock-height");
    };
  }, []);

  return (
    <div className="workspace workspace-creation">
      <section className="creation-feed" ref={feedRef} aria-labelledby="feed-title">
        <div className="feed-heading">
          <h1 id="feed-title" className="sr-only">
            Create
          </h1>
          <ModeSwitch active={mode} />
          <SizeSlider />
        </div>
        {list.items.length || placeholders.length ? (
          <div>{groups.map((group, index) => <section className="feed-batch" key={group.id} aria-label={group.batch ? `Batch ${group.id}` : "Result"}>
            {group.batch && <h2>Batch / {group.items.length + group.pending.length} results</h2>}
            <AssetGrid
              items={group.items}
              placeholders={group.pending}
              size={size}
              scroller={feedRef}
              label={`Your ${kindPlural[mode]}`}
              titleOf={(asset) => nameOf(asset.model)}
              fresh={list.fresh}
              hasMore={index === groups.length - 1 && list.hasMore}
              loading={list.loading}
              onEndReached={list.loadMore}
              onOpen={(asset) => openViewer(asset)}
              onFavorite={(asset) => void toggleFavorite(asset)}
            />
          </section>)}</div>
        ) : list.loading ? (
          <div className="feed-loading" role="status">
            <Spinner />
            Loading your {kindPlural[mode]}
          </div>
        ) : list.error ? (
          <div className="empty creation-empty" role="alert">
            <Icon name="cloud_off" />
            <h2>Your {kindPlural[mode]} did not load.</h2>
            <p>{list.error}</p>
            <button type="button" className="primary-button" onClick={list.reload}>
              Try again
              <Icon name="refresh" />
            </button>
          </div>
        ) : noneConfigured ? (
          <div className="empty creation-empty">
            <PixelMark />
            <h2>Connect a provider to start.</h2>
            <p>Add an API key or a local server once. Your keys stay on this machine.</p>
            <Link className="primary-button" to="/settings">
              Open providers
              <Icon name="arrow_forward" />
            </Link>
          </div>
        ) : (
          <div className="empty creation-empty">
            <PixelMark />
            <h2>What’s on your mind?</h2>
            <p>Your first idea starts in the bar below.</p>
          </div>
        )}
      </section>
      <div className="composer-dock" ref={dockRef}>
        <JobsTray />
        <Composer />
      </div>
    </div>
  );
}
