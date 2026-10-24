import { api, errorMessage } from "./api";
import { onReconnect, subscribeEvents } from "./events";
import { notifyError } from "./notices";
import { createStore } from "./store";
import type { Job, JobStatus } from "./types";

// Jobs shown in the tray above the composer: running ones, fresh failures and just-finished ones.

export const jobs = createStore<Job[]>([]);

const rank: Record<JobStatus, number> = { queued: 0, running: 1, succeeded: 2, failed: 2, canceled: 2 };
const dismissed = new Set<string>();
const doneTimers = new Map<string, ReturnType<typeof setTimeout>>();
/** How long a finished job stays in the tray after its outputs reach the grid. */
const READY_MS = 4000;

export const isActive = (job: Job) => job.status === "queued" || job.status === "running";

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
  if (job.status === "succeeded" && !doneTimers.has(job.id)) {
    doneTimers.set(job.id, setTimeout(() => removeJob(job.id), READY_MS));
  }
}

function removeJob(id: string) {
  doneTimers.delete(id);
  jobs.set((list) => list.filter((item) => item.id !== id));
}

export function dismissJob(id: string) {
  dismissed.add(id);
  removeJob(id);
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
/** Restores running jobs after a reload or a dropped event stream. */
export function loadActiveJobs(): Promise<void> {
  loading ??= api
    .jobs({ limit: 50 })
    .then(({ jobs: list }) => {
      const tracked = new Set(jobs.get().map((job) => job.id));
      for (const job of [...list].reverse()) {
        if (isActive(job) || tracked.has(job.id)) upsertJob(job);
      }
    })
    .catch(() => undefined)
    .finally(() => {
      loading = null;
    });
  return loading;
}

subscribeEvents({ job: upsertJob });
onReconnect(() => void loadActiveJobs());
