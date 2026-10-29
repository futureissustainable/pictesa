import { api, errorMessage } from "./api";
import { refreshBillingSoon } from "./catalog";
import { onReconnect, subscribeEvents, type PreviewEvent } from "./events";
import { notifyError } from "./notices";
import { readJson, writeJson } from "./prefs";
import { createStore } from "./store";
import type { Job, JobStatus } from "./types";

// Jobs shown in the tray above the composer: running ones, fresh failures and just-finished ones.

export const jobs = createStore<Job[]>([]);

const rank: Record<JobStatus, number> = { queued: 0, running: 1, succeeded: 2, failed: 2, canceled: 2 };
/** Dismissed failures stay dismissed after a reload. */
const DISMISSED_KEY = "pictesa:dismissed-jobs";
const dismissed = new Set<string>(readJson<string[]>(DISMISSED_KEY, []));
/** Failures this recent come back to the tray after a reload, until dismissed. */
const RECENT_FAILURE_MS = 60 * 60 * 1000;
const doneTimers = new Map<string, ReturnType<typeof setTimeout>>();
/** How long a finished job stays in the tray after its outputs reach the grid. */
const READY_MS = 4000;

export const isActive = (job: Job) => job.status === "queued" || job.status === "running";

/** The latest in-progress image per running job, as an object URL fetched once for every view. */
export const previews = createStore<ReadonlyMap<string, string>>(new Map());
const previewVersions = new Map<string, number>();
/** Old frames stay valid a moment so a crossfade can finish on them. */
const releaseUrl = (url: string) => setTimeout(() => URL.revokeObjectURL(url), 2000);

async function loadPreview({ job_id: id, version, url }: PreviewEvent) {
  const job = jobs.get().find((item) => item.id === id);
  if ((job && !isActive(job)) || (previewVersions.get(id) ?? 0) >= version) return;
  previewVersions.set(id, version);
  // A newer frame or the end of the job can arrive while this one loads.
  const current = () => previewVersions.get(id) === version;
  try {
    const response = await fetch(url, { credentials: "same-origin" });
    if (!response.ok || !current()) return;
    const objectUrl = URL.createObjectURL(await response.blob());
    const image = new Image();
    image.src = objectUrl;
    await image.decode().catch(() => undefined);
    if (!current()) return URL.revokeObjectURL(objectUrl);
    const previous = previews.get().get(id);
    previews.set((map) => new Map(map).set(id, objectUrl));
    if (previous) releaseUrl(previous);
  } catch {
    // A missed frame is harmless: the next one replaces it.
  }
}

function dropPreview(id: string) {
  previewVersions.delete(id);
  const previous = previews.get().get(id);
  if (!previous) return;
  previews.set((map) => {
    const next = new Map(map);
    next.delete(id);
    return next;
  });
  releaseUrl(previous);
}

/** `track` adds the job even when it already finished (a POST answer that failed at once). */
export function upsertJob(job: Job, track = false) {
  if (dismissed.has(job.id)) return;
  const list = jobs.get();
  const index = list.findIndex((item) => item.id === job.id);
  if (index === -1) {
    // Jobs started elsewhere (CLI, agents) appear while they run; their results arrive as assets.
    if (!track && !isActive(job)) return;
    jobs.set([job, ...list]);
  } else {
    const current = list[index]!;
    // Events can arrive before the POST answer or out of order: never move a job backwards.
    if (rank[job.status] < rank[current.status]) return;
    jobs.set(list.map((item) => (item.id === job.id ? job : item)));
  }
  if (!isActive(job)) dropPreview(job.id);
  if (job.status === "succeeded" && !doneTimers.has(job.id)) {
    doneTimers.set(job.id, setTimeout(() => removeJob(job.id), READY_MS));
    refreshBillingSoon();
  }
}

function removeJob(id: string) {
  doneTimers.delete(id);
  dropPreview(id);
  jobs.set((list) => list.filter((item) => item.id !== id));
}

export function dismissJob(id: string) {
  dismissed.add(id);
  writeJson(DISMISSED_KEY, [...dismissed].slice(-200));
  removeJob(id);
}

/** Clears every failed and canceled card from the tray. */
export function dismissFinished() {
  for (const job of jobs.get()) if (job.status === "failed" || job.status === "canceled") dismissed.add(job.id);
  writeJson(DISMISSED_KEY, [...dismissed].slice(-200));
  jobs.set((list) => list.filter((job) => job.status !== "failed" && job.status !== "canceled"));
}

export function addJobs(created: Job[]) {
  for (const job of [...created].reverse()) upsertJob(job, true);
}

export async function cancelJob(job: Job) {
  try {
    upsertJob(await api.cancelJob(job.id));
  } catch (error) {
    notifyError(errorMessage(error));
  }
}

export async function retryJob(job: Job) {
  try {
    const { jobs: created } = await api.retryJob(job.id);
    dismissJob(job.id);
    addJobs(created);
  } catch (error) {
    notifyError(errorMessage(error));
  }
}

let loading: Promise<void> | null = null;
/** Restores running jobs, and failures of the last hour, after a reload or a dropped event stream. */
export function loadActiveJobs(): Promise<void> {
  loading ??= api
    .jobs({ limit: 50 })
    .then(({ jobs: list }) => {
      const tracked = new Set(jobs.get().map((job) => job.id));
      for (const job of [...list].reverse()) {
        const recentFailure = job.status === "failed" && Date.now() - (job.finished_at ?? job.created_at) < RECENT_FAILURE_MS;
        if (isActive(job) || tracked.has(job.id)) upsertJob(job);
        else if (recentFailure) upsertJob(job, true);
      }
    })
    .catch(() => undefined)
    .finally(() => {
      loading = null;
    });
  return loading;
}

subscribeEvents({ job: upsertJob, preview: (preview) => void loadPreview(preview) });
onReconnect(() => void loadActiveJobs());
