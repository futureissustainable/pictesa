import { useState } from "react";
import { api, errorMessage } from "../lib/api";
import { addJobs } from "../lib/jobs";
import { notify, notifyError } from "../lib/notices";
import { Icon, Spinner } from "./icon";

export function Variations({ jobId, compact = false }: { jobId: string; compact?: boolean }) {
  const [busy, setBusy] = useState(false);
  return (
    <button
      type="button"
      className={compact ? "icon-button" : "text-button"}
      disabled={busy}
      aria-label="Variations: 4 new results"
      title="Variations: 4 new results"
      onClick={async () => {
        setBusy(true);
        try {
          const result = await api.variations(jobId);
          addJobs(result.jobs);
          notify("4 variations queued.");
        } catch (error) {
          notifyError(errorMessage(error));
        } finally {
          setBusy(false);
        }
      }}
    >
      {busy ? <Spinner /> : <Icon name="casino" />}
      {!compact && "Variations"}
    </button>
  );
}
