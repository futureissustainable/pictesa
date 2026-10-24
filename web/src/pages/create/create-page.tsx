import { useEffect, useRef } from "react";
import { toggleFavorite, useAssetList } from "../../lib/assets";
import { useModelNames, useModels } from "../../lib/catalog";
import { setMode, useComposer } from "../../lib/composer";
import { kindIcon, kindLabel, kindPlural } from "../../lib/format";
import { Link } from "../../lib/router";
import type { MediaKind } from "../../lib/types";
import { openViewer, useSequence } from "../../lib/viewer";
import { PixelMark } from "../../ui/brand";
import { AssetGrid } from "../../ui/grid";
import { Icon, Spinner } from "../../ui/icon";
import { SizeSlider, useImageSize } from "../../ui/size-slider";
import { Composer } from "./composer";
import { JobsTray } from "./jobs-tray";

export default function CreatePage() {
  const { mode } = useComposer();
  const size = useImageSize();
  const feedRef = useRef<HTMLElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const list = useAssetList({ kind: mode, source: "generated" });
  const nameOf = useModelNames([mode]);
  const models = useModels(mode);
  const audio = useModels("audio");
  const modes: MediaKind[] = audio.data?.length ? ["image", "video", "audio"] : ["image", "video"];
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
          <div className="segmented mode-switch" role="group" aria-label="What to create">
            {modes.map((kind) => (
              <button key={kind} type="button" aria-pressed={mode === kind} onClick={() => setMode(kind)}>
                <Icon name={kindIcon[kind]} filled={mode === kind} />
                <span>{kindLabel[kind]}</span>
              </button>
            ))}
          </div>
          <SizeSlider />
        </div>
        {list.items.length ? (
          <AssetGrid
            items={list.items}
            size={size}
            scroller={feedRef}
            label={`Your ${kindPlural[mode]}`}
            titleOf={(asset) => nameOf(asset.model)}
            fresh={list.fresh}
            hasMore={list.hasMore}
            loading={list.loading}
            onEndReached={list.loadMore}
            onOpen={(asset) => openViewer(asset)}
            onFavorite={(asset) => void toggleFavorite(asset)}
          />
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
