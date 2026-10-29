import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { api, errorMessage, isAbort, uploadFiles } from "../../lib/api";
import { useFullModel, useModels } from "../../lib/catalog";
import { onReconnect, subscribeEvents, emitLocal } from "../../lib/events";
import { estimateCost, money } from "../../lib/format";
import { addJobs, cancelJob, isActive } from "../../lib/jobs";
import { notifyError } from "../../lib/notices";
import { Link, navigate } from "../../lib/router";
import { createStore, useStore } from "../../lib/store";
import type { Asset, Job, ParamValues } from "../../lib/types";
import { videoFit, videoToolModels, videoTools, type VideoTool } from "../../lib/video-tools";
import { openViewer, useSequence } from "../../lib/viewer";
import { Thumb } from "../../ui/asset-card";
import { Icon, Spinner } from "../../ui/icon";
import { LibraryPicker } from "../create/library-picker";
import { ModelPicker } from "../create/model-picker";
import { ModeSwitch } from "../create/mode-switch";
import { ParamField } from "../create/params";

const runs = createStore<{ tool: VideoTool; job: Job }[]>([]);
function update(job: Job) {
  if (runs.get().some((r) => r.job.id === job.id)) runs.set((items) => items.map((r) => (r.job.id === job.id ? { ...r, job } : r)));
}
subscribeEvents({ job: update });
onReconnect(() => {
  for (const { job } of runs.get())
    if (isActive(job))
      void api
        .job(job.id)
        .then(update)
        .catch(() => undefined);
});
const noop = () => undefined;
const seconds = (value: number) => `${value.toFixed(2)}s`;
/** Two decimals, without trailing zeros: 1.5, not 1.50 or 1.4999999. */
const round = (value: number) => String(Math.round(value * 100) / 100);

/** A seconds field that takes free typing and applies the value on Enter or when it loses focus. */
function SecondsField({ label, value, onCommit }: { label: string; value: number; onCommit(value: number): void }) {
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft === null) return;
    const n = Number(draft.replace(",", "."));
    setDraft(null);
    if (draft.trim() && Number.isFinite(n)) onCommit(n);
  };
  return (
    <label className="field">
      <span>{label}</span>
      <input
        type="text"
        inputMode="decimal"
        value={draft ?? round(value)}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape" && draft !== null) {
            e.preventDefault();
            setDraft(null);
          }
        }}
      />
    </label>
  );
}

export default function VideoPage({ id }: { id: string }) {
  const [source, setSource] = useState<Asset | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tool, setTool] = useState<VideoTool>("trim");
  const [picking, setPicking] = useState<"source" | "clips" | "audio" | null>(null);
  const [clips, setClips] = useState<Asset[]>([]);
  const [audio, setAudio] = useState<Asset | null>(null);
  const [busy, setBusy] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [duration, setDuration] = useState(0);
  const [playhead, setPlayhead] = useState(0);
  const [start, setStart] = useState(0);
  const [end, setEnd] = useState(0);
  const [prompt, setPrompt] = useState("");
  const [picked, setPicked] = useState<Partial<Record<VideoTool, string>>>({});
  const [settings, setSettings] = useState<Record<string, ParamValues>>({});
  const player = useRef<HTMLVideoElement>(null);
  const upload = useRef<HTMLInputElement>(null);
  const models = useModels("all");
  const fitting = useMemo(() => videoToolModels(tool, models.data ?? []), [tool, models.data]);
  const listed = fitting.find((m) => m.id === picked[tool]) ?? fitting[0];
  const full = useFullModel(listed);
  const spec = full.spec;
  const info = videoTools.find((t) => t.id === tool)!;
  const params: ParamValues = Object.fromEntries(spec?.params.filter((p) => p.default !== undefined).map((p) => [p.key, p.default!]) ?? []);
  Object.assign(params, spec ? settings[spec.id] : {});
  if (tool === "trim") Object.assign(params, { start, end });
  if (tool === "frame") params.time = Math.min(playhead, Math.max(0, duration - 0.001));
  const estimate = spec ? estimateCost(spec, params, 1) : null;
  const price = estimate ? (estimate.kind === "total" ? money(estimate.usd) : estimate.text) : null;
  const history = useStore(runs);
  const outputs = useMemo(() => history.flatMap((r) => r.job.outputs), [history]);
  useSequence(outputs, false, noop);

  useEffect(() => {
    const controller = new AbortController();
    api
      .asset(id, controller.signal)
      .then((asset) => {
        if (asset.kind !== "video" || asset.deleted_at) throw new Error("Choose a video that is not in the trash.");
        setSource(asset);
        setClips([asset]);
      })
      .catch((reason: unknown) => {
        if (!isAbort(reason)) setError(errorMessage(reason));
      });
    return () => controller.abort();
  }, [id]);

  const seek = (time: number) => {
    setPlayhead(time);
    if (player.current) player.current.currentTime = time;
  };
  const canRun = Boolean(
    source &&
    spec?.configured &&
    videoFit(tool, spec) !== null &&
    !busy &&
    !uploading &&
    !full.loading &&
    (tool !== "trim" || (end > start && end <= duration)) &&
    (tool !== "join" || clips.length >= 2) &&
    (tool !== "add-audio" || audio) &&
    (tool !== "frame" || duration > 0) &&
    (!["extend", "sound"].includes(tool) || !spec.prompt || Boolean(prompt.trim())),
  );

  async function run(requestedTool = tool) {
    const model = requestedTool === "frame" ? videoToolModels("frame", models.data ?? [])[0] : spec;
    if (!source || !model || busy || uploading || (requestedTool === "frame" ? duration <= 0 : !canRun)) return;
    setBusy(true);
    try {
      const role = model.inputs.find((s) => s.kind === "video")?.role ?? "video";
      const inputs = (requestedTool === "join" ? clips : [source]).map((asset) => ({ asset_id: asset.id, role }));
      if (requestedTool === "add-audio" && audio) inputs.push({ asset_id: audio.id, role: "audio" });
      const answer = await api.generate({
        model: model.id,
        prompt: model.prompt ? prompt : "",
        negative_prompt: null,
        params: requestedTool === "frame" ? { time: Math.min(playhead, Math.max(0, duration - 0.001)) } : params,
        inputs,
        count: 1,
      });
      addJobs(answer.jobs);
      runs.set((items) => [...answer.jobs.map((job) => ({ tool: requestedTool, job })), ...items].slice(0, 12));
      // Fast local tools may finish before the POST response and its SSE subscriber catches up.
      for (const job of answer.jobs)
        void api
          .job(job.id)
          .then(update)
          .catch(() => undefined);
    } catch (reason) {
      notifyError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }
  function move(index: number, offset: number) {
    setClips((items) => {
      const next = [...items];
      [next[index], next[index + offset]] = [next[index + offset]!, next[index]!];
      return next;
    });
  }
  async function uploadAudio(files: File[]) {
    if (!files.length) return;
    setUploading(true);
    try {
      const answer = await uploadFiles(files.slice(0, 1));
      answer.assets.forEach((asset) => emitLocal({ asset }));
      const asset = answer.assets.find((a) => a.kind === "audio");
      if (asset) setAudio(asset);
      else notifyError(answer.errors[0]?.error ?? "Choose an audio file.");
    } catch (reason) {
      notifyError(errorMessage(reason));
    } finally {
      setUploading(false);
    }
  }

  return (
    <div className="workspace workspace-edit">
      <div className="feed-heading edit-heading">
        <h1 className="sr-only">Edit video</h1>
        <ModeSwitch active="edit" />
        <button type="button" className="text-button change-video" onClick={() => setPicking("source")} aria-label="Change video" title="Change video">
          <Icon name="photo_library" />
          <span>Change video</span>
        </button>
      </div>
      {error ? (
        <div className="empty" role="alert">
          <h2>This video did not load.</h2>
          <p>{error}</p>
          <button type="button" className="primary-button" onClick={() => setPicking("source")}>
            Choose a video
          </button>
        </div>
      ) : (
        <div className="edit-layout video-layout">
          <div className="video-main">
            <div className="video-stage">
              {source ? (
                <video
                  ref={player}
                  src={source.url}
                  controls
                  playsInline
                  preload="metadata"
                  aria-label="Video preview"
                  onLoadedMetadata={(event) => {
                    const d = event.currentTarget.duration;
                    if (Number.isFinite(d)) {
                      setDuration(d);
                      setEnd(d);
                    }
                  }}
                  onTimeUpdate={(event) => setPlayhead(event.currentTarget.currentTime)}
                />
              ) : (
                <div role="status">
                  <Spinner />
                  Loading the video
                </div>
              )}
            </div>
            <div className="video-timeline">
              <label className="field">
                <span>
                  Playhead{" "}
                  <output>
                    {seconds(playhead)} / {seconds(duration)}
                  </output>
                </span>
                <input
                  type="range"
                  aria-label="Playhead"
                  min={0}
                  max={duration || 1}
                  step={0.01}
                  value={playhead}
                  disabled={!duration}
                  onChange={(e) => seek(Number(e.target.value))}
                />
              </label>
              {tool === "trim" && (
                <div className="video-trim" role="group" aria-label="Trim range">
                  <div
                    className="video-handles"
                    style={
                      {
                        "--trim-start": `${duration ? (start / duration) * 100 : 0}%`,
                        "--trim-end": `${duration ? (end / duration) * 100 : 100}%`,
                      } as CSSProperties
                    }
                  >
                    <input
                      type="range"
                      aria-label="In point"
                      min={0}
                      max={duration || 1}
                      step={0.01}
                      value={start}
                      disabled={!duration}
                      onChange={(e) => {
                        const n = Math.min(Number(e.target.value), Math.max(0, end - 0.01));
                        setStart(n);
                        seek(n);
                      }}
                    />
                    <input
                      type="range"
                      aria-label="Out point"
                      min={0}
                      max={duration || 1}
                      step={0.01}
                      value={end}
                      disabled={!duration}
                      onChange={(e) => {
                        const n = Math.max(Number(e.target.value), start + 0.01);
                        setEnd(n);
                        seek(n);
                      }}
                    />
                  </div>
                  <div className="video-time-fields">
                    <SecondsField
                      label="In (seconds)"
                      value={start}
                      onCommit={(value) => {
                        const n = Math.max(0, Math.min(value, end - 0.01));
                        setStart(n);
                        seek(n);
                      }}
                    />
                    <SecondsField
                      label="Out (seconds)"
                      value={end}
                      onCommit={(value) => {
                        const n = Math.min(duration, Math.max(start + 0.01, value));
                        setEnd(n);
                        seek(n);
                      }}
                    />
                    <output className="hint">Keep {seconds(Math.max(0, end - start))}</output>
                  </div>
                </div>
              )}
              <button
                type="button"
                className="text-button"
                disabled={!duration || busy || !videoToolModels("frame", models.data ?? []).length}
                onClick={() => void run("frame")}
              >
                <Icon name="camera" />
                Save this frame
              </button>
            </div>
          </div>
          <aside className="edit-panel" aria-label="Video tools">
            <div className="edit-tools" role="group" aria-label="Video tool">
              {videoTools.map((item) => (
                <button type="button" className="edit-tool" key={item.id} aria-pressed={tool === item.id} onClick={() => setTool(item.id)}>
                  <Icon name={item.icon} />
                  {item.label}
                </button>
              ))}
            </div>
            <div className="edit-options">
              {tool === "trim" && <p className="hint">Drag the in and out handles or enter seconds to keep part of the video.</p>}
              {tool === "frame" && <p className="hint">Save a PNG at {seconds(playhead)}. Move the playhead to choose another frame.</p>}
              {tool === "smooth" && <p className="hint">Interpolate to twice the frame rate. Slow on CPU.</p>}
              {tool === "join" && (
                <>
                  <ol className="video-clips" aria-label="Clip order">
                    {clips.map((clip, i) => (
                      <li key={`${clip.id}-${i}`}>
                        <Thumb asset={clip} className="thumb" />
                        <span>
                          {i + 1}. {clip.name ?? "Video"}
                        </span>
                        <button type="button" className="icon-button" aria-label={`Move clip ${i + 1} up`} disabled={i === 0} onClick={() => move(i, -1)}>
                          <Icon name="arrow_upward" />
                        </button>
                        <button
                          type="button"
                          className="icon-button"
                          aria-label={`Move clip ${i + 1} down`}
                          disabled={i === clips.length - 1}
                          onClick={() => move(i, 1)}
                        >
                          <Icon name="arrow_downward" />
                        </button>
                        <button
                          type="button"
                          className="icon-button"
                          aria-label={`Remove clip ${i + 1}`}
                          onClick={() => setClips((items) => items.filter((_, n) => n !== i))}
                        >
                          <Icon name="close" />
                        </button>
                      </li>
                    ))}
                  </ol>
                  <button type="button" className="text-button" disabled={clips.length >= 10} onClick={() => setPicking("clips")}>
                    <Icon name="add" />
                    Add clips ({clips.length}/10)
                  </button>
                  <p className="hint">2 to 10 clips. Uses the first clip’s size and frame rate. Clips without audio get silence.</p>
                </>
              )}
              {tool === "add-audio" && (
                <>
                  {audio && (
                    <div className="video-audio">
                      <audio src={audio.url} controls aria-label="Added audio preview" />
                      <button type="button" className="text-button" onClick={() => setAudio(null)}>
                        Remove audio
                      </button>
                    </div>
                  )}
                  <div className="video-audio-pick">
                    <button type="button" className="text-button" onClick={() => setPicking("audio")}>
                      <Icon name="photo_library" />
                      Choose audio
                    </button>
                    <button type="button" className="text-button" disabled={uploading} onClick={() => upload.current?.click()}>
                      <Icon name="upload" />
                      {uploading ? "Uploading" : "Upload audio"}
                    </button>
                  </div>
                  <input
                    ref={upload}
                    type="file"
                    className="sr-only"
                    tabIndex={-1}
                    aria-hidden="true"
                    accept="audio/*"
                    onChange={(e) => {
                      void uploadAudio([...(e.target.files ?? [])]);
                      e.target.value = "";
                    }}
                  />
                  <p className="hint">Audio is padded or cut to the video length. Volume and fades apply to the added track.</p>
                </>
              )}
              {spec?.prompt && (
                <label className="field">
                  <span>{tool === "sound" ? "Describe the sound" : "Describe what happens next"}</span>
                  <textarea value={prompt} rows={3} maxLength={4000} onChange={(e) => setPrompt(e.target.value)} />
                </label>
              )}
              {spec?.params
                .filter((p) => !["start", "end", "time"].includes(p.key))
                .map((p) => (
                  <ParamField
                    key={`${spec.id}-${p.key}`}
                    spec={p}
                    value={params[p.key] ?? null}
                    onChange={(value) =>
                      setSettings((current) => {
                        const next = { ...current[spec.id] };
                        if (value === undefined) delete next[p.key];
                        else next[p.key] = value;
                        return { ...current, [spec.id]: next };
                      })
                    }
                  />
                ))}
            </div>
            {fitting.length ? (
              <div className="edit-model">
                <span className="edit-label">Model</span>
                <ModelPicker
                  models={fitting}
                  model={listed}
                  loading={models.loading}
                  onSelect={(model) => setPicked((current) => ({ ...current, [tool]: model.id }))}
                />
                {full.loading && (
                  <p className="hint" role="status">
                    Loading settings
                  </p>
                )}
                {full.error && (
                  <p role="alert">
                    {full.error}{" "}
                    <button type="button" className="text-button" onClick={full.retry}>
                      Try again
                    </button>
                  </p>
                )}
                {spec && videoFit(tool, spec) === null && <p role="alert">This model does not take the inputs this tool needs. Choose another model.</p>}
              </div>
            ) : (
              <div className="notice edit-none" role="status">
                <p>{models.loading ? "Loading models" : (models.error ?? `${info.label} needs ${info.needs}.`)}</p>
                <Link to="/settings" className="notice-action">
                  Open providers
                </Link>
              </div>
            )}
            <button type="button" className="primary-button generate edit-run" disabled={!canRun} onClick={() => void run()}>
              <span>{busy ? "Starting" : info.label}</span>
              {price && <span className="generate-price">{price}</span>}
              {busy ? <Spinner /> : <Icon name="arrow_forward" />}
            </button>
            {history.length > 0 && (
              <section className="edit-results" aria-label="Results">
                <h3>Results</h3>
                <ul>
                  {history.map(({ tool: madeWith, job }) =>
                    job.status === "succeeded" ? (
                      job.outputs.map((asset) => (
                        <li key={`${job.id}-${asset.id}`} className="edit-result">
                          <button type="button" className="edit-result-open" aria-label="Open result" onClick={() => openViewer(asset)}>
                            <Thumb asset={asset} className="thumb" />
                          </button>
                          <div>
                            <p>{videoTools.find((t) => t.id === madeWith)!.label}</p>
                            <button
                              type="button"
                              className="text-button"
                              onClick={() => {
                                if (asset.kind === "audio") {
                                  setAudio(asset);
                                  setTool("add-audio");
                                } else navigate(`/edit?${asset.kind === "video" ? "video" : "image"}=${encodeURIComponent(asset.id)}`);
                              }}
                            >
                              <Icon name="edit" />
                              Keep editing
                            </button>
                          </div>
                        </li>
                      ))
                    ) : (
                      <li key={job.id} className="edit-result">
                        <span className="edit-result-open">{isActive(job) ? <Spinner /> : <Icon name="error" />}</span>
                        <div>
                          <p>{videoTools.find((t) => t.id === madeWith)!.label}</p>
                          <p className="edit-result-status" role="status">
                            {isActive(job) ? (job.message ?? "Waiting") : (job.error?.message ?? "Canceled")}
                          </p>
                          {isActive(job) && (
                            <button type="button" className="text-button" onClick={() => void cancelJob(job)}>
                              Cancel
                            </button>
                          )}
                        </div>
                      </li>
                    ),
                  )}
                </ul>
              </section>
            )}
          </aside>
        </div>
      )}
      {picking && (
        <LibraryPicker
          kind={picking === "audio" ? "audio" : "video"}
          room={picking === "clips" ? 10 - clips.length : 1}
          title={picking === "clips" ? "Add clips" : picking === "audio" ? "Choose audio" : "Choose a video to edit"}
          onClose={() => setPicking(null)}
          onPick={(assets) => {
            if (picking === "clips") setClips((items) => [...items, ...assets].slice(0, 10));
            else if (picking === "audio") setAudio(assets[0] ?? null);
            else if (assets[0]) navigate(`/edit?video=${encodeURIComponent(assets[0].id)}`);
          }}
        />
      )}
    </div>
  );
}
