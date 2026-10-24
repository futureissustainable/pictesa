import { useEffect, useRef, useState } from "react";
import { clock, kindIcon, kindLabel } from "../../lib/format";
import { cancelJob, dismissJob, isActive, jobs, retryJob } from "../../lib/jobs";
import { useStore } from "../../lib/store";
import type { Job } from "../../lib/types";
import { openViewer } from "../../lib/viewer";
import { Thumb } from "../../ui/asset-card";
import { Icon, Spinner } from "../../ui/icon";

/** Live generation cards above the composer, fed by GET /api/events. */
export function JobsTray() {
  const list = useStore(jobs);
  const [now, setNow] = useState(() => Date.now());
  const [announcement, setAnnouncement] = useState("");
  const previous = useRef(new Map<string, Job["status"]>());
  const running = list.some(isActive);

  // Elapsed time ticks once a second, only while something runs.
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);

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
  }, [list]);

  return (
    <>
      <p className="sr-only" role="status">
        {announcement}
      </p>
      {list.length > 0 && (
        <section className="jobs-tray" aria-label="Generations">
          <ul>
            {list.map((job) => (
              <JobCard key={job.id} job={job} now={now} />
            ))}
          </ul>
        </section>
      )}
    </>
  );
}

function statusText(job: Job, now: number): string {
  switch (job.status) {
    case "queued":
      return `${job.message || "Queued"}, ${clock(now - job.created_at)}`;
    case "running":
      if (job.progress !== null && job.progress !== undefined) return `${Math.round(job.progress * 100)}%${job.message ? `, ${job.message}` : ""}`;
      return `${job.message || "Generating"}, ${clock(now - (job.started_at ?? job.created_at))}`;
    case "succeeded":
      return job.outputs?.length > 1 ? `${job.outputs.length} ready` : "Ready";
    case "failed":
      return job.error?.message || "Failed";
    case "canceled":
      return "Canceled";
  }
}

function JobCard({ job, now }: { job: Job; now: number }) {
  const active = isActive(job);
  const output = job.outputs?.[0];
  const determinate = job.status === "running" && typeof job.progress === "number";
  return (
    <li className={`job-card job-${job.status}`} title={job.prompt || undefined}>
      {output ? (
        <button type="button" className="job-visual" aria-label={`Open the ${kindLabel[job.output].toLowerCase()}`} onClick={() => openViewer(output)}>
          <Thumb asset={output} />
        </button>
      ) : (
        <span className="job-visual" aria-hidden="true">
          {active ? <Spinner /> : <Icon name={job.status === "failed" ? "error" : job.status === "canceled" ? "block" : kindIcon[job.output]} />}
        </span>
      )}
      <div className="job-text">
        <p className="job-model">{job.model_name}</p>
        <p className="job-status">{statusText(job, now)}</p>
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
