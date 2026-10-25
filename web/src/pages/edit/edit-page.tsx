import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, errorMessage, isAbort } from "../../lib/api";
import { useFullModel, useModels } from "../../lib/catalog";
import {
  editTools,
  EXPAND_PROMPT,
  expandRatios,
  imageRole,
  lightDirections,
  lightMoods,
  lightParams,
  paddingForRatio,
  relightPrompt,
  shapeParams,
  toolModels,
  unfitReason,
  type EditTool,
  type LightDirection,
} from "../../lib/edit-tools";
import { onReconnect, subscribeEvents } from "../../lib/events";
import { estimateCost, money } from "../../lib/format";
import { addJobs, cancelJob, isActive } from "../../lib/jobs";
import { notify, notifyError } from "../../lib/notices";
import { Link, navigate, useLocation } from "../../lib/router";
import { createStore, useStore } from "../../lib/store";
import type { Asset, AssetDetail, GenerationRequest, Job, JobInput, ModelSpec, Padding, ParamValues } from "../../lib/types";
import { openViewer, useSequence } from "../../lib/viewer";
import { Icon, Spinner } from "../../ui/icon";
import { LibraryPicker } from "../create/library-picker";
import { ModelPicker } from "../create/model-picker";
import { ModeSwitch } from "../create/mode-switch";
import VideoPage from "./video-page";
import { MaskCanvas, type MaskHandle } from "./mask-canvas";

// The edit view (`/edit?image=<id>`): paint, expand, relight or cut out any library image.

/** Runs started here this session, newest first, so results survive switching images. */
const editRuns = createStore<{ tool: EditTool; job: Job }[]>([]);
/** The model picked per tool, for this session. */
const pickedModels = createStore<Partial<Record<EditTool, string>>>({});

function trackRun(tool: EditTool, job: Job) {
  editRuns.set((runs) => [{ tool, job }, ...runs.filter((run) => run.job.id !== job.id)].slice(0, 12));
}

function updateRun(job: Job) {
  if (editRuns.get().some((run) => run.job.id === job.id)) editRuns.set((runs) => runs.map((run) => (run.job.id === job.id ? { ...run, job } : run)));
}

subscribeEvents({ job: updateRun });
onReconnect(() => {
  for (const run of editRuns.get()) if (isActive(run.job)) void api.job(run.job.id).then(updateRun).catch(() => undefined);
});

const isTool = (value: string | null): value is EditTool => editTools.some((tool) => tool.id === value);

export default function EditPage() {
  const url = useLocation();
  const id = url.searchParams.get("image");
  const tool = url.searchParams.get("tool");
  const [picking, setPicking] = useState<"image" | "video" | null>(null);
  const choose = (asset: Asset) => navigate(`/edit?${asset.kind === "video" ? "video" : "image"}=${encodeURIComponent(asset.id)}${isTool(tool) ? `&tool=${tool}` : ""}`);

  const video = url.searchParams.get("video");
  if (video) return <VideoPage key={video} id={video} />;

  return (
    <div className="workspace workspace-edit">
      <div className="feed-heading edit-heading">
        <h1 className="sr-only">Edit</h1>
        <ModeSwitch active="edit" />
        {id && (
          <button type="button" className="text-button change-image" onClick={() => setPicking("image")} aria-label="Change image" title="Change image">
            <Icon name="photo_library" />
            <span>Change image</span>
          </button>
        )}
      </div>
      {id ? (
        <Editor key={id} id={id} initialTool={isTool(tool) ? tool : "inpaint"} onChoose={() => setPicking("image")} />
      ) : (
        <div className="empty edit-empty">
          <Icon name="edit" />
          <h2>Choose an image or video to edit.</h2>
          <p>Retouch an image, or trim, join and finish a video.</p>
          <button type="button" className="primary-button" onClick={() => setPicking("image")}>
            Choose an image
            <Icon name="photo_library" />
          </button>
          <button type="button" className="text-button" onClick={() => setPicking("video")}><Icon name="movie" />Choose a video</button>
        </div>
      )}
      {picking && <LibraryPicker kind={picking} room={1} title={`Choose ${picking === "image" ? "an image" : "a video"} to edit`} onClose={() => setPicking(null)} onPick={([asset]) => asset && choose(asset)} />}
    </div>
  );
}

function Editor({ id, initialTool, onChoose }: { id: string; initialTool: EditTool; onChoose(): void }) {
  const [source, setSource] = useState<AssetDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tool, setTool] = useState<EditTool>(initialTool);
  const [busy, setBusy] = useState(false);
  const [hasPaint, setHasPaint] = useState(false);
  const canvas = useRef<MaskHandle>(null);

  // Tool settings.
  // Inpaint and expand each keep their own words.
  const [prompts, setPrompts] = useState<Partial<Record<EditTool, string>>>({});
  const prompt = prompts[tool] ?? "";
  const setPrompt = (value: string) => setPrompts((current) => ({ ...current, [tool]: value }));
  const [expandBy, setExpandBy] = useState<"ratio" | "pixels">("ratio");
  const [ratio, setRatio] = useState("16:9");
  const [sides, setSides] = useState<Padding>({ left: 128, top: 0, right: 128, bottom: 0 });
  const [direction, setDirection] = useState<LightDirection>("left");
  const [mood, setMood] = useState(lightMoods[1]!);
  const [lightText, setLightText] = useState("");

  useEffect(() => {
    const controller = new AbortController();
    api
      .asset(id, controller.signal)
      .then(setSource)
      .catch((reason: unknown) => {
        if (!isAbort(reason)) setLoadError(errorMessage(reason));
      });
    return () => controller.abort();
  }, [id]);

  const models = useModels("image");
  const fitting = useMemo(() => toolModels(tool, models.data ?? []), [tool, models.data]);
  const picked = useStore(pickedModels)[tool];
  const listed = fitting.find((model) => model.id === picked) ?? fitting[0];
  const full = useFullModel(listed);
  const spec = full.spec;
  const problem = spec ? unfitReason(tool, spec) : null;
  const info = editTools.find((item) => item.id === tool)!;
  const onPaintChange = useCallback((value: boolean) => setHasPaint(value), []);

  const width = source?.width ?? 0;
  const height = source?.height ?? 0;
  const padding: Padding | null = tool !== "expand" || !width || !height ? null : expandBy === "ratio" ? paddingForRatio(width, height, ratio) : sides;
  const grows = Boolean(padding && padding.left + padding.top + padding.right + padding.bottom > 0);

  const runs = useStore(editRuns);
  const outputs = useMemo(() => runs.flatMap((run) => run.job.outputs ?? []), [runs]);
  useSequence(outputs, false, noop);

  async function run() {
    if (!source || !spec || busy) return;
    if (!spec.configured) return navigate(`/settings#provider-${encodeURIComponent(spec.provider)}`);
    if (problem) return notifyError(problem);
    if (tool === "inpaint" && !canvas.current?.hasPaint()) return notifyError("Paint over the area to change first.");
    if (tool === "inpaint" && spec.prompt && !prompt.trim()) return notifyError("Describe what goes in the painted area.");
    if (tool === "expand" && !grows) return notifyError("Add space on at least one side.");
    setBusy(true);
    try {
      const role = imageRole(spec);
      let inputs: JobInput[] = [{ asset_id: source.id, role }];
      let text = "";
      let params: ParamValues = shapeParams(spec, width, height);
      if (tool === "inpaint") {
        const blob = await canvas.current!.exportMask();
        if (!blob) return notifyError("Paint over the area to change first.");
        const mask = await api.saveMask(source.id, blob);
        inputs.push({ asset_id: mask.id, role: "mask" });
        text = prompt.trim();
      } else if (tool === "expand") {
        const grown = await api.expandCanvas(source.id, padding!);
        inputs = [
          { asset_id: grown.image.id, role },
          { asset_id: grown.mask.id, role: "mask" },
        ];
        text = prompt.trim() || EXPAND_PROMPT;
        params = shapeParams(spec, grown.image.width, grown.image.height);
      } else if (tool === "relight") {
        text = relightPrompt(spec, direction, mood, lightText);
        params = { ...params, ...lightParams(spec, direction) };
      }
      const body: GenerationRequest = { model: spec.id, prompt: spec.prompt ? text : "", negative_prompt: null, params, inputs, count: 1 };
      const { jobs } = await api.generate(body);
      addJobs(jobs);
      jobs.forEach((job) => trackRun(tool, job));
    } catch (error) {
      if (error instanceof ApiError && error.field === "billing") {
        notify(errorMessage(error), { tone: "error", action: { label: "Add funds", run: () => navigate("/settings/billing") } });
      } else notifyError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  if (loadError) {
    return (
      <div className="empty edit-empty" role="alert">
        <Icon name="error" />
        <h2>This image did not load.</h2>
        <p>{loadError}</p>
        <button type="button" className="primary-button" onClick={onChoose}>
          Choose another image
          <Icon name="photo_library" />
        </button>
      </div>
    );
  }
  if (source && (source.kind !== "image" || source.deleted_at)) {
    return (
      <div className="empty edit-empty" role="alert">
        <Icon name="hide_image" />
        <h2>{source.deleted_at ? "This image is in the trash." : "Only images can be edited."}</h2>
        <button type="button" className="primary-button" onClick={onChoose}>
          Choose an image
          <Icon name="photo_library" />
        </button>
      </div>
    );
  }

  const estimate = spec ? estimateCost(spec, {}, 1) : null;
  const price = estimate ? (estimate.kind === "total" ? money(estimate.usd) : estimate.text) : null;
  const ready = Boolean(spec?.configured && !problem && !busy && source);

  return (
    <div className="edit-layout">
      <div className="edit-main">{source ? <MaskCanvas ref={canvas} asset={source} paint={tool === "inpaint"} frame={padding} onPaintChange={onPaintChange} /> : <div className="page-loading" role="status"><Spinner />Loading the image</div>}</div>
      <aside className="edit-panel" aria-label="Edit settings">
        <div className="edit-tools" role="group" aria-label="Tool">
          {editTools.map((item) => (
            <button key={item.id} type="button" className="edit-tool" aria-pressed={tool === item.id} onClick={() => setTool(item.id)}>
              <Icon name={item.icon} filled={tool === item.id} />
              <span>{item.label}</span>
            </button>
          ))}
        </div>

        <div className="edit-options">
          {tool === "inpaint" && (
            <>
              <p className="hint">{hasPaint ? "Painted. Describe what goes there." : "Paint over the area to change."}</p>
              <label className="field">
                <span>What goes in the painted area</span>
                <textarea rows={3} maxLength={4000} value={prompt} placeholder="A wooden bench under the window" onChange={(event) => setPrompt(event.target.value)} />
              </label>
            </>
          )}
          {tool === "expand" && (
            <>
              <div className="segmented segmented-small" role="group" aria-label="Expand by">
                <button type="button" aria-pressed={expandBy === "ratio"} onClick={() => setExpandBy("ratio")}>
                  Aspect ratio
                </button>
                <button type="button" aria-pressed={expandBy === "pixels"} onClick={() => setExpandBy("pixels")}>
                  Pixels per side
                </button>
              </div>
              {expandBy === "ratio" ? (
                <div className="edit-chips" role="group" aria-label="Aspect ratio">
                  {expandRatios.map((value) => (
                    <button key={value} type="button" className="chip" aria-pressed={ratio === value} onClick={() => setRatio(value)}>
                      {value}
                    </button>
                  ))}
                </div>
              ) : (
                <div className="edit-sides">
                  {(["left", "top", "right", "bottom"] as const).map((side) => (
                    <label key={side} className="field">
                      <span>{side.charAt(0).toUpperCase() + side.slice(1)}</span>
                      <input
                        type="number"
                        inputMode="numeric"
                        min={0}
                        max={4096}
                        step={8}
                        value={sides[side]}
                        onChange={(event) => setSides((current) => ({ ...current, [side]: Math.max(0, Math.min(4096, Math.round(Number(event.target.value) || 0))) }))}
                      />
                    </label>
                  ))}
                </div>
              )}
              {padding && (
                <p className="hint">
                  {grows ? `New size ${width + padding.left + padding.right} x ${height + padding.top + padding.bottom} px` : "The image already has this shape. Pick another ratio."}
                </p>
              )}
              <label className="field">
                <span>Describe the new area (optional)</span>
                <textarea rows={2} maxLength={4000} value={prompt} placeholder="More of the forest" onChange={(event) => setPrompt(event.target.value)} />
              </label>
            </>
          )}
          {tool === "relight" && (
            <>
              <fieldset className="light-pad">
                <legend>Light from</legend>
                <div className="light-grid">
                  {lightDirections.map((item) => (
                    <button key={item.id} type="button" aria-pressed={direction === item.id} aria-label={`Light from the ${item.label}`} title={`From the ${item.label}`} onClick={() => setDirection(item.id)}>
                      {direction === item.id ? <Icon name="light_mode" filled /> : <span className="light-dot" aria-hidden="true" />}
                    </button>
                  ))}
                </div>
              </fieldset>
              <div className="edit-chips" role="group" aria-label="Mood">
                {lightMoods.map((value) => (
                  <button key={value} type="button" className="chip" aria-pressed={mood === value} onClick={() => setMood(mood === value ? "" : value)}>
                    {value}
                  </button>
                ))}
              </div>
              <label className="field">
                <span>Or describe the light</span>
                <input type="text" maxLength={300} value={lightText} placeholder="Warm window light, soft shadows" onChange={(event) => setLightText(event.target.value)} />
              </label>
            </>
          )}
          {tool === "remove_background" && <p className="hint">Keeps the subject and makes the background transparent. The result is a PNG.</p>}
        </div>

        <ModelChoice tool={tool} fitting={fitting} listed={listed} loading={models.loading} full={full} problem={problem} needs={info.needs} label={info.label} />

        <button type="button" className="primary-button generate edit-run" disabled={!ready} onClick={() => void run()} aria-label={`${info.label}${estimate?.kind === "total" ? `, estimated ${money(estimate.usd)}` : ""}`}>
          <span>{busy ? "Starting" : info.label}</span>
          {price && <span className="generate-price">{price}</span>}
          {busy ? <Spinner /> : <Icon name="arrow_forward" />}
        </button>

        <Results />
      </aside>
    </div>
  );
}

const noop = () => undefined;

function ModelChoice({
  tool,
  fitting,
  listed,
  loading,
  full,
  problem,
  needs,
  label,
}: {
  tool: EditTool;
  fitting: ModelSpec[];
  listed: ModelSpec | undefined;
  loading: boolean;
  full: { loading: boolean; error: string | null; retry(): void };
  problem: string | null;
  needs: string;
  label: string;
}) {
  if (!fitting.length) {
    return (
      <div className="notice edit-none" role="status">
        {loading ? (
          <p>Loading models</p>
        ) : (
          <>
            <p>
              No model for {label.toLowerCase()} is set up. It needs {needs}.
            </p>
            <Link to="/settings" className="notice-action">
              Open providers
            </Link>
          </>
        )}
      </div>
    );
  }
  return (
    <div className="edit-model">
      <span className="edit-label">Model</span>
      <ModelPicker models={fitting} model={listed} loading={loading} onSelect={(model) => pickedModels.set((current) => ({ ...current, [tool]: model.id }))} />
      {full.loading && (
        <p className="hint" role="status">
          <Spinner />
          Loading its settings
        </p>
      )}
      {full.error && (
        <p className="prompt-error" role="alert">
          {full.error}{" "}
          <button type="button" className="notice-action" onClick={full.retry}>
            Try again
          </button>
        </p>
      )}
      {problem && (
        <p className="prompt-error" role="alert">
          {problem}
        </p>
      )}
    </div>
  );
}

function Results() {
  const runs = useStore(editRuns);
  if (!runs.length) return null;
  const label = (tool: EditTool) => editTools.find((item) => item.id === tool)!.label;
  return (
    <section className="edit-results" aria-label="Results">
      <h3>Results</h3>
      <ul>
        {runs.map(({ tool, job }) =>
          job.status === "succeeded" ? (
            job.outputs.map((output) => (
              <li key={output.id} className="edit-result">
                <button type="button" className="edit-result-open" onClick={() => openViewer(output)} aria-label={`Open the ${label(tool).toLowerCase()} result`}>
                  <img src={output.thumb_url ?? output.url} alt="" loading="lazy" />
                </button>
                <div>
                  <p>{label(tool)}</p>
                  <button type="button" className="text-button" onClick={() => navigate(`/edit?image=${encodeURIComponent(output.id)}&tool=${tool}`)}>
                    <Icon name="edit" />
                    Keep editing
                  </button>
                </div>
              </li>
            ))
          ) : (
            <li key={job.id} className="edit-result">
              <span className="edit-result-open edit-result-wait" aria-hidden="true">
                {isActive(job) ? <Spinner /> : <Icon name="error" />}
              </span>
              <div>
                <p>{label(tool)}</p>
                {isActive(job) ? (
                  <p className="edit-result-status" role="status">
                    {job.status === "queued" ? "Waiting" : typeof job.progress === "number" ? `${Math.round(job.progress * 100)}%` : (job.message ?? "Running")}
                    <button type="button" className="text-button" onClick={() => void cancelJob(job)}>
                      Cancel
                    </button>
                  </p>
                ) : (
                  <p className="edit-result-status error">{job.status === "canceled" ? "Canceled." : (job.error?.message ?? "It failed.")}</p>
                )}
              </div>
            </li>
          ),
        )}
      </ul>
    </section>
  );
}
