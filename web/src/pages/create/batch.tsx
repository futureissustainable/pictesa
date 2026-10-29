import { useEffect, useId, useMemo, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { money } from "../../lib/format";
import { addJobs } from "../../lib/jobs";
import { notify } from "../../lib/notices";
import type { BatchEstimate, BatchRequest, GenerationRequest, ModelSpec, ParamValue } from "../../lib/types";
import { Dialog } from "../../ui/dialog";
import { Spinner } from "../../ui/icon";

const tabs = ["Prompt list", "Seeds", "Grid"];

export function BatchDialog({ model, request, onClose }: { model: ModelSpec; request: GenerationRequest; onClose: () => void }) {
  const id = useId();
  const [tab, setTab] = useState(0);
  const [prompts, setPrompts] = useState(request.prompt);
  const [format, setFormat] = useState<"lines" | "csv">("lines");
  const seed = model.params.find((p) => p.type === "seed" || (p.key === "seed" && p.type === "int"));
  const [sweep, setSweep] = useState(false);
  const [start, setStart] = useState(String(seed ? (request.params[seed.key] ?? 0) : 0));
  const [count, setCount] = useState("4");
  const [axes, setAxes] = useState<{ key: string; text: string }[]>([]);
  const [preview, setPreview] = useState<{ signature: string; estimate: BatchEstimate } | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const body = useMemo(() => {
    // A parameter just added has no values yet: it joins the estimate once it has some.
    const grid = axes.map((axis) => ({
      key: axis.key,
      values: axis.text
        .split("\n")
        .filter((v) => v.trim())
        .map((text): ParamValue => {
          try {
            return JSON.parse(text) as ParamValue;
          } catch {
            return text.trim();
          }
        }),
    })).filter((axis) => axis.values.length > 0);
    return {
      ...request,
      count: 1,
      compare: [],
      ...(model.prompt === false && !prompts.trim() ? {} : { prompt_list: prompts, prompt_format: format }),
      grid,
      ...(sweep ? { seeds: { start: Number(start), count: Number(count) } } : {}),
    } satisfies BatchRequest;
  }, [request, prompts, format, axes, sweep, start, count, model.prompt]);
  const signature = JSON.stringify(body);
  useEffect(() => {
    let current = true;
    setError("");
    setPreview(null);
    if (
      sweep &&
      (!start.trim() ||
        !count.trim() ||
        !Number.isSafeInteger(Number(start)) ||
        Number(start) < 0 ||
        !Number.isInteger(Number(count)) ||
        Number(count) < 1 ||
        Number(count) > 64)
    ) {
      setError("Use a nonnegative whole seed and a count from 1 to 64.");
      return;
    }
    const timer = setTimeout(() => {
      void api
        .estimateBatch(body)
        .then((estimate) => {
          if (current) setPreview({ signature, estimate });
        })
        .catch((e) => {
          if (current) setError(errorMessage(e));
        });
    }, 300);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [body, signature, sweep, start, count]);
  const choices = model.params.filter((p) => !(sweep && p.key === seed?.key));
  return (
    <Dialog title="Batch" onClose={onClose} size="wide" className="batch-dialog">
      <div className="batch-body">
        <div
          className="batch-tabs"
          role="tablist"
          aria-label="Batch options"
          onKeyDown={(event) => {
            const next =
              event.key === "ArrowRight"
                ? (tab + 1) % 3
                : event.key === "ArrowLeft"
                  ? (tab + 2) % 3
                  : event.key === "Home"
                    ? 0
                    : event.key === "End"
                      ? 2
                      : null;
            if (next === null) return;
            event.preventDefault();
            setTab(next);
            document.getElementById(`${id}-tab-${next}`)?.focus();
          }}
        >
          {tabs.map((label, i) => (
            <button
              key={label}
              id={`${id}-tab-${i}`}
              type="button"
              role="tab"
              aria-selected={tab === i}
              aria-controls={`${id}-panel-${i}`}
              tabIndex={tab === i ? 0 : -1}
              onClick={() => setTab(i)}
            >
              {label}
            </button>
          ))}
        </div>
        <div id={`${id}-panel-0`} role="tabpanel" aria-labelledby={`${id}-tab-0`} hidden={tab !== 0} className="more-fields">
          <label className="field">
            <span>Format</span>
            <select aria-label="Format" value={format} onChange={(e) => setFormat(e.target.value as "lines" | "csv")}>
              <option value="lines">One prompt per line</option>
              <option value="csv">CSV first column, no header</option>
            </select>
          </label>
          <label className="field">
            <span>Prompts</span>
            <textarea
              aria-label="Prompts"
              rows={6}
              value={prompts}
              maxLength={1000000}
              onChange={(e) => setPrompts(e.target.value)}
              placeholder="A cabin at dawn&#10;A cabin at dusk"
            />
          </label>
        </div>
        <div id={`${id}-panel-1`} role="tabpanel" aria-labelledby={`${id}-tab-1`} hidden={tab !== 1} className="more-fields">
          {seed ? (
            <>
              <label className="prompt-row">
                <input type="checkbox" checked={sweep} onChange={(e) => setSweep(e.target.checked)} />
                Sweep seeds
              </label>
              <div className="batch-seeds">
                <label className="field">
                  <span>Start seed</span>
                  <input
                    type="number"
                    min={0}
                    max={Number.MAX_SAFE_INTEGER}
                    step={1}
                    disabled={!sweep}
                    value={start}
                    onChange={(e) => setStart(e.target.value)}
                  />
                </label>
                <label className="field">
                  <span>Seed count</span>
                  <input type="number" min={1} max={64} step={1} disabled={!sweep} value={count} onChange={(e) => setCount(e.target.value)} />
                </label>
              </div>
              <p className="control-help">Each prompt runs once for every seed, starting at your chosen seed.</p>
            </>
          ) : (
            <p className="control-help">This model has no seed parameter. Repeat prompts for independent runs.</p>
          )}
        </div>
        <div id={`${id}-panel-2`} role="tabpanel" aria-labelledby={`${id}-tab-2`} hidden={tab !== 2} className="more-fields">
          <p className="control-help">Choose up to 3 parameters. Write one value per line. Every combination runs.</p>
          {axes.map((axis, index) => (
            <div className="batch-axis" key={index}>
              <label className="field">
                <span>Parameter {index + 1}</span>
                <select
                  aria-label={`Parameter ${index + 1}`}
                  value={axis.key}
                  onChange={(e) => setAxes((items) => items.map((a, i) => (i === index ? { ...a, key: e.target.value } : a)))}
                >
                  <option value="">Choose parameter</option>
                  {choices.map((p) => (
                    <option key={p.key} value={p.key} disabled={axes.some((a, i) => i !== index && a.key === p.key)}>
                      {p.label}
                    </option>
                  ))}
                </select>
              </label>
              <label className="field">
                <span>Values for parameter {index + 1}</span>
                <textarea
                  aria-label={`Values for parameter ${index + 1}`}
                  rows={3}
                  value={axis.text}
                  onChange={(e) => setAxes((items) => items.map((a, i) => (i === index ? { ...a, text: e.target.value } : a)))}
                  placeholder="One value per line"
                />
              </label>
              <button
                type="button"
                className="text-button"
                aria-label={`Remove parameter ${index + 1}`}
                onClick={() => setAxes((items) => items.filter((_, i) => i !== index))}
              >
                Remove
              </button>
            </div>
          ))}
          <button
            type="button"
            className="text-button"
            disabled={axes.length >= 3 || axes.length >= choices.length}
            onClick={() => setAxes((items) => [...items, { key: "", text: "" }])}
          >
            Add parameter
          </button>
        </div>
        <footer className="batch-summary">
          <div role="status" aria-live="polite">
            {preview && preview.signature === signature ? (
              <>
                <strong>
                  {preview.estimate.run_count} {preview.estimate.run_count === 1 ? "run" : "runs"}
                </strong>
                <p>
                  {preview.estimate.total_estimated_cost_usd === null ? "Cost unknown" : `${money(preview.estimate.total_estimated_cost_usd)} estimated total`}
                </p>
              </>
            ) : error ? (
              <p className="prompt-error" role="alert">
                {error}
              </p>
            ) : (
              <span>
                <Spinner /> Checking count and cost
              </span>
            )}
            <small>Prompts × seeds × grid. Maximum 64 runs.</small>
          </div>
          <button
            type="button"
            className="primary-button"
            disabled={busy || !preview || preview.signature !== signature}
            onClick={async () => {
              setBusy(true);
              setError("");
              try {
                const created = await api.generateBatch(body);
                addJobs(created.jobs);
                notify(`${created.jobs.length} batch runs queued.`);
                onClose();
              } catch (e) {
                setError(errorMessage(e));
                setPreview(null);
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? <Spinner /> : "Run batch"}
          </button>
        </footer>
      </div>
    </Dialog>
  );
}
