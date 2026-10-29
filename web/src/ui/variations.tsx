import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { addJobs } from "../lib/jobs";
import { notify, notifyError } from "../lib/notices";
import { Icon, Spinner } from "./icon";

/** How long the button rests after queueing, so a second click does not queue 4 more by accident. */
const REST_MS = 4000;

export function Variations({ jobId, compact = false }: { jobId: string; compact?: boolean }) {
  const [state, setState] = useState<"idle" | "busy" | "queued">("idle");
  useEffect(() => {
    if (state !== "queued") return;
    const timer = setTimeout(() => setState("idle"), REST_MS);
    return () => clearTimeout(timer);
  }, [state]);
  const label = state === "queued" ? "4 variations queued" : "Variations: 4 new results";
  return (
    <button
      type="button"
      className={compact ? "icon-button" : "text-button"}
      disabled={state !== "idle"}
      aria-label={label}
      title={label}
      onClick={async () => {
        setState("busy");
        try {
          const result = await api.variations(jobId);
          addJobs(result.jobs);
          notify("4 variations queued.");
          setState("queued");
        } catch (error) {
          notifyError(errorMessage(error));
          setState("idle");
        }
      }}
    >
      {state === "busy" ? <Spinner /> : <Icon name={state === "queued" ? "check" : "casino"} />}
      {!compact && (state === "queued" ? "Queued" : "Variations")}
    </button>
  );
}
