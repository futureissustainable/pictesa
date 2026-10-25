import { memo } from "react";
import { useTextDraft } from "../../lib/boards";
import { roleLabel } from "../../lib/format";
import { isActive, jobs } from "../../lib/jobs";
import { useStore } from "../../lib/store";
import type { Asset, BoardEdge, BoardNode, StepRun } from "../../lib/types";
import { Thumb } from "../../ui/asset-card";
import { Icon, Spinner } from "../../ui/icon";

export interface NodeActions {
  run(id: string, downstream: boolean): void;
  cancel(id: string): void;
  /** `seen`: the note's revision when the typing started. */
  text(id: string, text: string, seen: number): void;
  open(asset: Asset): void;
}

export const runLabel: Record<StepRun["status"], string> = {
  waiting: "Waiting for earlier steps",
  starting: "Starting",
  running: "Running",
  succeeded: "Done",
  failed: "Failed",
  canceled: "Canceled",
};

export const runActive = (run?: StepRun) => Boolean(run && ["waiting", "starting", "running"].includes(run.status));

/** Height of an asset node from its file's shape, kept between a wide strip and a tall card. */
export function assetHeight(width: number, asset?: Asset): number {
  const ratio = asset?.width && asset.height ? asset.height / asset.width : 1;
  return width * Math.min(3, Math.max(0.25, ratio));
}

/** Progress of a run from this person's own job events; other members' runs show their status only. */
function useRunProgress(run?: StepRun): number | null {
  const list = useStore(jobs);
  if (!run || run.status !== "running") return null;
  const mine = list.filter((job) => run.jobs?.includes(job.id) && isActive(job));
  const known = mine.map((job) => job.progress).filter((value): value is number => typeof value === "number");
  return known.length ? known.reduce((sum, value) => sum + value, 0) / known.length : null;
}

export const AssetBody = memo(function AssetBody({ node, asset, title }: { node: BoardNode; asset?: Asset; title: string }) {
  if (!asset) {
    return (
      <div className="board-asset board-missing" style={{ height: node.w * 0.6 }}>
        <Icon name="hide_image" />
        <span>The file is gone.</span>
      </div>
    );
  }
  return (
    <div className="board-asset" style={{ height: assetHeight(node.w, asset) }}>
      <Thumb asset={asset} eager />
      {asset.kind !== "image" && (
        <span className="asset-badge">
          <Icon name={asset.kind === "video" ? "play_arrow" : "graphic_eq"} />
        </span>
      )}
      {asset.deleted_at && <span className="board-trashed">In the trash</span>}
      <span className="sr-only">{title}</span>
    </div>
  );
});

/** A note, edited in place. */
export const NoteBody = memo(function NoteBody({ node, editable, actions }: { node: BoardNode; editable: boolean; actions: NodeActions }) {
  const draft = useTextDraft(node.text ?? "", node.rev, (text, seen) => actions.text(node.id, text, seen));
  return (
    <>
      <div className="board-node-head" data-drag>
        <Icon name="sticky_note_2" />
        <span>Note</span>
      </div>
      <textarea
        className="board-note-text"
        aria-label="Note"
        value={draft.text}
        readOnly={!editable}
        maxLength={10000}
        placeholder={editable ? "Write a note" : ""}
        onChange={(event) => draft.change(event.target.value)}
        onBlur={draft.flush}
      />
    </>
  );
});

export const StepBody = memo(function StepBody({
  node,
  run,
  inputs,
  feedsSteps,
  modelName,
  takesPrompt,
  editable,
  actions,
}: {
  node: BoardNode;
  run?: StepRun;
  inputs: BoardEdge[];
  feedsSteps: boolean;
  modelName: string;
  /** False for models without a prompt (a background remover): the prompt line goes. */
  takesPrompt: boolean;
  editable: boolean;
  actions: NodeActions;
}) {
  const progress = useRunProgress(run);
  const active = runActive(run);
  const roles = [...new Set(inputs.map((edge) => (edge.role ? roleLabel[edge.role] : "Input")))];
  return (
    <>
      <div className="board-node-head" data-drag>
        <Icon name="auto_awesome" />
        <span className="board-step-model">{modelName || "Choose a model"}</span>
        {run && (
          <span className={`board-run-status ${run.status}`} role="status">
            {active && <Spinner />}
            {run.status === "running" && progress !== null ? `${Math.round(progress * 100)}%` : runLabel[run.status]}
          </span>
        )}
      </div>
      {(takesPrompt || node.prompt) && (
        <p className={`board-step-prompt ${node.prompt ? "" : "empty"}`} data-drag>
          {node.prompt || "No prompt yet."}
        </p>
      )}
      {run?.error && run.status !== "succeeded" && <p className="board-step-error">{run.error}</p>}
      {run?.error && run.status === "succeeded" && <p className="board-step-note">{run.error}</p>}
      <div className="board-step-foot">
        <span className="board-step-inputs" data-drag>
          {inputs.length ? `${roles.join(", ")} (${inputs.length})` : "No inputs"}
          {(node.count ?? 1) > 1 && `, ${node.count} results`}
        </span>
        {editable &&
          (active ? (
            <button type="button" className="text-button" onClick={() => actions.cancel(node.id)}>
              <Icon name="close" />
              Cancel
            </button>
          ) : (
            <>
              {feedsSteps && (
                <button type="button" className="icon-button" aria-label="Run this step and the steps after it" title="Run with later steps" onClick={() => actions.run(node.id, true)}>
                  <Icon name="fast_forward" />
                </button>
              )}
              <button type="button" className="primary-button generate board-run" onClick={() => actions.run(node.id, false)}>
                Run
                <Icon name="play_arrow" />
              </button>
            </>
          ))}
      </div>
      {run?.status === "running" && (
        <div className={`job-progress ${progress === null ? "indeterminate" : ""}`} aria-hidden="true">
          <span style={progress === null ? undefined : { transform: `scaleX(${progress})` }} />
        </div>
      )}
    </>
  );
});
