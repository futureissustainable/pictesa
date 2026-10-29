import { useEffect, useRef, useState } from "react";
import { api } from "../../lib/api";
import { clock, kindIcon, kindLabel, money } from "../../lib/format";
import { compareBatches, comparing, dismissCompare, type CompareBatch } from "../../lib/compare";
import { connection } from "../../lib/events";
import { cancelJob, dismissFinished, dismissJob, isActive, jobs, previews, retryJob } from "../../lib/jobs";
import { cancelRun, dismissRun, jobsInRuns, resultJobs, runs, useRecipes } from "../../lib/recipes";
import { useStore } from "../../lib/store";
import type { Job, RecipeRun } from "../../lib/types";
import { openViewer } from "../../lib/viewer";
import { Thumb } from "../../ui/asset-card";
import { Dialog } from "../../ui/dialog";
import { Icon, Spinner } from "../../ui/icon";
import { LivePreview } from "../../ui/live-preview";

/** Live generation cards above the composer, fed by GET /api/events. */
export function JobsTray() {
  const list = useStore(jobs);
  const runList = useStore(runs);
  const batches = useStore(compareBatches);
  const viewing = useStore(comparing);
  const [now, setNow] = useState(() => Date.now());
  // While the server is away, running cards stop their clocks: nothing is known about them.
  const offline = useStore(connection) === "reconnecting";
  const [announcement, setAnnouncement] = useState("");
  const previous = useRef(new Map<string, string>());
  const running = list.some(isActive) || runList.some((run) => run.status === "running");
  // A recipe's step jobs are shown inside its run card.
  const inRuns = jobsInRuns(runList);
  const compared = new Set(batches.map((batch) => batch.id));
  const loose = list.filter((job) => !inRuns.has(job.id) && !compared.has(job.batch_id));
  const ended = loose.filter((job) => job.status === "failed" || job.status === "canceled").length;

  // Elapsed time ticks once a second, only while something runs.
  useEffect(() => {
    if (!running || offline) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running, offline]);

  // Announce finished and failed jobs once, not every progress tick.
  useEffect(() => {
    const seen = previous.current;
    for (const job of list) {
      const before = seen.get(job.id);
      if (before && before !== job.status) {
        if (job.status === "succeeded") setAnnouncement(`${kindLabel[job.output]} ready from ${job.model_name}.`);
        if (job.status === "failed") setAnnouncement(`${job.model_name} failed: ${job.error?.message ?? "no reason given"}.`);
      }
      seen.set(job.id, job.status);
    }
    for (const run of runList) {
      const before = seen.get(run.id);
      if (before && before !== run.status) {
        if (run.status === "succeeded") setAnnouncement(`${run.recipe_name} finished.`);
        if (run.status === "failed") setAnnouncement(`${run.recipe_name} failed: ${run.error ?? "no reason given"}.`);
      }
      seen.set(run.id, run.status);
    }
  }, [list, runList]);

  return (
    <>
      <p className="sr-only" role="status">
        {announcement}
      </p>
      {(loose.length > 0 || runList.length > 0 || batches.length > 0) && (
        <section className="jobs-tray" aria-label="Generations">
          <ul>
            {ended > 1 && (
              <li className="jobs-clear">
                <button type="button" className="text-button" onClick={dismissFinished}>
                  <Icon name="close" />
                  Dismiss all
                </button>
              </li>
            )}
            {batches.map((batch) => (
              <CompareCard key={batch.id} batch={batch} />
            ))}
            {runList.map((run) => (
              <RunCard key={run.id} run={run} now={now} jobs={list} />
            ))}
            {loose.map((job) => (
              <JobCard key={job.id} job={job} now={now} offline={offline} />
            ))}
          </ul>
        </section>
      )}
      {viewing && batches.some((batch) => batch.id === viewing) && <CompareDialog batch={batches.find((batch) => batch.id === viewing)!} now={now} />}
    </>
  );
}

function statusText(job: Job, now: number, offline = false): string {
  if (offline && isActive(job)) return "Waiting for the server";
  switch (job.status) {
    // The number leads, so it stays visible when the message is cut short.
    case "queued":
      return `${clock(now - job.created_at)}, ${job.message || "Queued"}`;
    case "running":
      if (job.progress !== null && job.progress !== undefined) return `${Math.round(job.progress * 100)}%${job.message ? `, ${job.message}` : ""}`;
      return `${clock(now - (job.started_at ?? job.created_at))}, ${job.message || "Generating"}`;
    case "succeeded":
      return job.outputs?.length > 1 ? `${job.outputs.length} ready` : "Ready";
    case "failed":
      return job.error?.message || "Failed";
    case "canceled":
      return "Canceled";
  }
}

function JobCard({ job, now, offline }: { job: Job; now: number; offline: boolean }) {
  const active = isActive(job);
  const output = job.outputs?.[0];
  const determinate = job.status === "running" && typeof job.progress === "number";
  const preview = useStore(previews).get(job.id);
  return (
    <li className={`job-card job-${job.status}`} title={job.prompt || undefined}>
      {output ? (
        <button type="button" className="job-visual" aria-label={`Open the ${kindLabel[job.output].toLowerCase()}`} onClick={() => openViewer(output)}>
          <Thumb asset={output} />
        </button>
      ) : (
        <span className="job-visual" aria-hidden="true">
          {preview ? <LivePreview src={preview} /> : active ? <Spinner /> : <Icon name={job.status === "failed" ? "error" : job.status === "canceled" ? "block" : kindIcon[job.output]} />}
        </span>
      )}
      <div className="job-text">
        <p className="job-model">{job.model_name}</p>
        <p className="job-status" title={statusText(job, now, offline)}>
          {statusText(job, now, offline)}
        </p>
      </div>
      <div className="job-actions">
        {active && (
          <button type="button" className="icon-button" aria-label={`Cancel ${job.model_name}`} title="Cancel" onClick={() => void cancelJob(job)}>
            <Icon name="close" />
          </button>
        )}
        {(job.status === "failed" || job.status === "canceled") && (
          <>
            <button type="button" className="icon-button" aria-label={`Retry ${job.model_name}`} title="Retry" onClick={() => void retryJob(job)}>
              <Icon name="replay" />
            </button>
            <button type="button" className="icon-button" aria-label="Dismiss" title="Dismiss" onClick={() => dismissJob(job.id)}>
              <Icon name="close" />
            </button>
          </>
        )}
        {job.status === "succeeded" && <Icon name="check_circle" className="job-done" />}
      </div>
      {job.status === "running" ? (
        <span className={`job-progress ${determinate ? "" : "indeterminate"}`} aria-hidden="true">
          <span style={determinate ? { transform: `scaleX(${Math.max(0.02, Math.min(1, job.progress!))})` } : undefined} />
        </span>
      ) : null}
    </li>
  );
}

/** One card per app run: the current step, its jobs' real progress, and how much of a set is ready. */
function RunCard({ run, now, jobs: list }: { run: RecipeRun; now: number; jobs: Job[] }) {
  const recipes = useRecipes().data;
  const recipe = recipes?.find((item) => item.id === run.recipe_id);
  const stepIds = run.step_jobs?.[run.step] ?? [run.jobs[Math.min(run.step, run.jobs.length - 1)]].filter((id): id is string => Boolean(id));
  // Finished jobs leave the tray store after a moment; a step job that is gone has finished.
  const stepJobs = stepIds.map((id) => list.find((job) => job.id === id));
  const firstId = resultJobs(run)[0];
  const [fetched, setFetched] = useState<Job | null>(null);
  const firstResult = list.find((job) => job.id === firstId) ?? fetched;
  useEffect(() => {
    if (run.status === "succeeded" && firstId && !list.some((job) => job.id === firstId)) void api.job(firstId).then(setFetched).catch(() => undefined);
    // Only when the run finishes; later store changes do not matter.
  }, [run.status, firstId]);
  const output = run.status === "succeeded" ? firstResult?.outputs?.[0] : undefined;
  const label = recipe?.steps[run.step]?.label ?? `Step ${run.step + 1}`;
  const running = run.status === "running";
  const live = stepJobs.find((job) => job?.status === "running") ?? stepJobs[0];
  const preview = useStore(previews).get(live?.id ?? "");
  // A set counts finished items as done; a single job reports its own progress.
  const shares = stepJobs.map((job) => (!job || !isActive(job) ? 1 : job.status === "running" && typeof job.progress === "number" ? job.progress : null));
  const known = shares.length > 0 && shares.every((share) => share !== null);
  const stepProgress = known ? (shares as number[]).reduce((sum, share) => sum + share, 0) / shares.length : null;
  const fraction = running && stepProgress !== null ? (run.step + stepProgress) / Math.max(1, run.steps) : null;
  const ready = stepJobs.filter((job) => !job || job.status === "succeeded").length;
  const where = `step ${run.step + 1} of ${run.steps}: ${label}`;
  const status = running
    ? stepIds.length > 1
      ? `${ready} of ${stepIds.length} ready, ${where}`
      : `${stepProgress !== null ? `${Math.round(stepProgress * 100)}%` : clock(now - (live?.started_at ?? live?.created_at ?? run.updated_at))}, ${where}`
    : run.status === "succeeded"
      ? resultJobs(run).length > 1
        ? `${resultJobs(run).length} ready`
        : "Ready"
      : run.status === "failed"
        ? run.error || "Failed"
        : "Canceled";
  return (
    <li className={`job-card job-${run.status === "running" ? "running" : run.status} run-card`}>
      {output ? (
        <button type="button" className="job-visual" aria-label={`Open the result of ${run.recipe_name}`} onClick={() => openViewer(output)}>
          <Thumb asset={output} />
        </button>
      ) : (
        <span className="job-visual" aria-hidden="true">
          {running && preview ? <LivePreview src={preview} /> : running ? <Spinner /> : <Icon name={run.status === "failed" ? "error" : run.status === "canceled" ? "block" : "apps"} />}
        </span>
      )}
      <div className="job-text">
        <p className="job-model">{run.recipe_name}</p>
        <p className="job-status">{status}</p>
      </div>
      <div className="job-actions">
        {running && (
          <button type="button" className="icon-button" aria-label={`Cancel ${run.recipe_name}`} title="Cancel" onClick={() => void cancelRun(run)}>
            <Icon name="close" />
          </button>
        )}
        {(run.status === "failed" || run.status === "canceled") && (
          <button type="button" className="icon-button" aria-label="Dismiss" title="Dismiss" onClick={() => dismissRun(run.id)}>
            <Icon name="close" />
          </button>
        )}
        {run.status === "succeeded" && <Icon name="check_circle" className="job-done" />}
      </div>
      {running ? (
        <span className={`job-progress ${fraction !== null ? "" : "indeterminate"}`} aria-hidden="true">
          <span style={fraction !== null ? { transform: `scaleX(${Math.max(0.02, Math.min(1, fraction))})` } : undefined} />
        </span>
      ) : null}
    </li>
  );
}

/** A compare batch in the tray: how many models are done, and the way into the side-by-side view. */
function CompareCard({ batch }: { batch: CompareBatch }) {
  const done = batch.jobs.filter((job) => !isActive(job)).length;
  const ready = batch.jobs.filter((job) => job.status === "succeeded").length;
  const active = done < batch.jobs.length;
  const known = batch.jobs.map((job) => (job.status === "running" && typeof job.progress === "number" ? job.progress : isActive(job) ? null : 1));
  const fraction = known.every((value) => value !== null) ? (known as number[]).reduce((sum, value) => sum + value, 0) / known.length : null;
  return (
    <li className={`job-card compare-card ${active ? "job-running" : "job-succeeded"}`} title={batch.prompt || undefined}>
      <button type="button" className="job-visual" aria-label="Show the comparison" onClick={() => comparing.set(batch.id)}>
        {active ? <Spinner /> : <Icon name="grid_view" />}
      </button>
      <div className="job-text">
        <p className="job-model">Compare {batch.jobs.length} models</p>
        <p className="job-status">{active ? `${ready} of ${batch.jobs.length} ready` : ready === batch.jobs.length ? "All ready" : `${ready} of ${batch.jobs.length} ready, ${batch.jobs.length - ready} failed`}</p>
      </div>
      <div className="job-actions">
        <button type="button" className="text-button" onClick={() => comparing.set(batch.id)}>
          View
        </button>
        {!active && (
          <button type="button" className="icon-button" aria-label="Dismiss" title="Dismiss" onClick={() => dismissCompare(batch.id)}>
            <Icon name="close" />
          </button>
        )}
      </div>
      {active ? (
        <span className={`job-progress ${fraction !== null ? "" : "indeterminate"}`} aria-hidden="true">
          <span style={fraction !== null ? { transform: `scaleX(${Math.max(0.02, Math.min(1, fraction))})` } : undefined} />
        </span>
      ) : null}
    </li>
  );
}

/** The same prompt on every model, side by side, with the model name under each result. */
function CompareDialog({ batch, now }: { batch: CompareBatch; now: number }) {
  const frames = useStore(previews);
  return (
    <Dialog title="Compare models" size="wide" onClose={() => comparing.set(null)} className="compare-dialog">
      {batch.prompt && <p className="compare-prompt">{batch.prompt}</p>}
      <ul className="compare-grid" style={{ gridTemplateColumns: `repeat(${Math.min(batch.jobs.length, 4)}, minmax(0, 1fr))` }}>
        {batch.jobs.map((job) => {
          const outputs = job.outputs ?? [];
          const frame = frames.get(job.id);
          return (
            <li key={job.id} className="compare-cell">
              <div className="compare-media">
                {outputs.length ? (
                  outputs.map((asset) => (
                    <button key={asset.id} type="button" className="asset-open" aria-label={`Open the result from ${job.model_name}`} onClick={() => openViewer(asset)}>
                      <Thumb asset={asset} />
                    </button>
                  ))
                ) : (
                  <div className={`compare-state${frame ? " has-preview" : ""}`} role="status">
                    {frame ? <LivePreview src={frame} /> : isActive(job) ? <Spinner /> : <Icon name={job.status === "failed" ? "error" : "block"} />}
                    <span>{statusText(job, now)}</span>
                  </div>
                )}
              </div>
              <p className="compare-model">{job.model_name}</p>
              {typeof job.cost_usd === "number" && job.cost_usd > 0 && <p className="compare-cost">{money(job.cost_usd)}</p>}
            </li>
          );
        })}
      </ul>
    </Dialog>
  );
}
