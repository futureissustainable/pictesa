import { subscribeEvents } from "./events";
import { createStore } from "./store";
import type { Job } from "./types";

// A compare run sends one prompt to several models in one batch (`compare` on POST
// /api/generations). The batch is kept here, with its jobs, so the side-by-side view
// still has every result after the tray lets the finished jobs go.

export interface CompareBatch {
  id: string;
  prompt: string;
  jobs: Job[];
  created_at: number;
}

export const compareBatches = createStore<CompareBatch[]>([]);
/** The batch shown in the side-by-side dialog. */
export const comparing = createStore<string | null>(null);

export const MAX_COMPARE = 7;

export function trackCompare(batchId: string, jobs: Job[]) {
  if (!jobs.length) return;
  compareBatches.set((list) => [{ id: batchId, prompt: jobs[0]!.prompt, jobs, created_at: Date.now() }, ...list.filter((batch) => batch.id !== batchId)].slice(0, 12));
}

export function dismissCompare(batchId: string) {
  compareBatches.set((list) => list.filter((batch) => batch.id !== batchId));
  if (comparing.get() === batchId) comparing.set(null);
}

const rank: Record<Job["status"], number> = { queued: 0, running: 1, succeeded: 2, failed: 2, canceled: 2 };

subscribeEvents({
  job: (job) => {
    const list = compareBatches.get();
    const batch = list.find((item) => item.id === job.batch_id);
    if (!batch) return;
    const index = batch.jobs.findIndex((item) => item.id === job.id);
    if (index >= 0 && rank[job.status] < rank[batch.jobs[index]!.status]) return;
    const jobs = index >= 0 ? batch.jobs.map((item) => (item.id === job.id ? job : item)) : [...batch.jobs, job];
    compareBatches.set(list.map((item) => (item.id === batch.id ? { ...batch, jobs } : item)));
  },
});
