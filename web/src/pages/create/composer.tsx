import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type DragEvent } from "react";
import { api, ApiError, errorMessage } from "../../lib/api";
import { ASSET_DRAG_TYPE, readDraggedAsset, rejectionMessage, routeFiles, uploadInto } from "../../lib/attach";
import { defaultModel, useModels, usePresets, useProviders } from "../../lib/catalog";
import {
  activeInputs,
  addInput,
  effectiveParams,
  inputKey,
  requestParams,
  resetParams,
  selectModel,
  setCount,
  setNegativePrompt,
  setParam,
  setPrompt,
  slotInputs,
  useComposer,
} from "../../lib/composer";
import { estimateCost, kindPlural, money, plural } from "../../lib/format";
import { addJobs } from "../../lib/jobs";
import { notify, notifyError } from "../../lib/notices";
import { navigate } from "../../lib/router";
import type { GenerationRequest, ParamValues } from "../../lib/types";
import { Choice, Control } from "../../ui/control";
import { Icon, Spinner } from "../../ui/icon";
import { InputSlots, slotLabel } from "./inputs";
import { ModelPicker, providerName } from "./model-picker";
import { ParamControl, ParamField } from "./params";
import { PresetsStrip } from "./presets";

const COUNTS = [1, 2, 3, 4];
const NO_PARAMS: ParamValues = {};

const isEditable = (element: Element | null) =>
  element instanceof HTMLElement && (element.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName));

/** The prompt-first composer. Everything below the prompt is generated from the selected ModelSpec. */
export function Composer() {
  const draft = useComposer();
  const { mode } = draft;
  const models = useModels(mode);
  const providers = useProviders().data;
  const presets = usePresets(mode).data;
  const list = models.data ?? [];
  const model = list.find((item) => item.id === draft.modelByMode[mode]) ?? defaultModel(list);
  const preset = presets?.find((item) => item.id === draft.presetByMode[mode]) ?? null;
  const explicit = (model && draft.paramsByModel[model.id]) || NO_PARAMS;
  const values = useMemo(() => (model ? effectiveParams(model, explicit, preset) : {}), [model, explicit, preset]);
  const inputs = draft.inputsByMode[mode];
  const attached = activeInputs(model, inputs);
  const uploading = attached.some((input) => input.upload);
  const estimate = model ? estimateCost(model, values, draft.count) : null;
  const [busy, setBusy] = useState(false);
  const [invalid, setInvalid] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const promptRef = useRef<HTMLTextAreaElement>(null);

  // Remember the default model so the choice is stable across reloads.
  useEffect(() => {
    if (model && draft.modelByMode[mode] !== model.id) selectModel(mode, model);
  }, [model, mode, draft.modelByMode]);

  // Settings loaded from the library: put the cursor in the prompt.
  useEffect(() => {
    if (draft.revision) promptRef.current?.focus({ preventScroll: true });
  }, [draft.revision]);

  useEffect(() => setInvalid(null), [model?.id, mode]);

  const attachFiles = (files: File[]) => {
    if (!model || !files.length) return;
    const { placements, rejected } = routeFiles(model, inputs, files);
    placements.forEach((placement) => void uploadInto(mode, placement.slot.role, placement.files));
    if (rejected.length) notifyError(rejectionMessage(model, rejected));
  };

  // Pasting files anywhere on the page (outside other fields) attaches them.
  const attachRef = useRef(attachFiles);
  attachRef.current = attachFiles;
  useEffect(() => {
    const onPaste = (event: globalThis.ClipboardEvent) => {
      const files = [...(event.clipboardData?.files ?? [])];
      if (!files.length || isEditable(document.activeElement)) return;
      event.preventDefault();
      attachRef.current(files);
    };
    document.addEventListener("paste", onPaste);
    return () => document.removeEventListener("paste", onPaste);
  }, []);

  const onPromptPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const files = [...event.clipboardData.files];
    if (!files.length) return;
    event.preventDefault();
    attachFiles(files);
  };

  const onDrop = (event: DragEvent) => {
    event.preventDefault();
    setDragging(false);
    if (!model) return;
    const asset = readDraggedAsset(event.dataTransfer);
    if (asset) {
      const slot = model.inputs.find((candidate) => candidate.kind === asset.kind && slotInputs(candidate, inputs).length < candidate.max);
      if (!slot) return notifyError(`${model.name} has no free slot for ${kindPlural[asset.kind]}.`);
      return addInput(mode, { key: inputKey(), role: slot.role, asset });
    }
    attachFiles([...event.dataTransfer.files]);
  };

  async function submit() {
    if (!model || busy) return;
    if (!model.configured) {
      navigate(`/settings#provider-${encodeURIComponent(model.provider)}`);
      return;
    }
    if (uploading) {
      notify("Wait for the upload to finish.");
      return;
    }
    const missing = model.inputs.find((slot) => slotInputs(slot, inputs).filter((input) => input.asset).length < slot.min);
    if (missing) {
      setInvalid(`inputs.${missing.role}`);
      notifyError(`Add ${missing.min > 1 ? `${missing.min} ${kindPlural[missing.kind]} for` : "a"} ${slotLabel(missing).toLowerCase()}.`);
      return;
    }
    if (model.prompt && !draft.prompt.trim() && !attached.length) {
      setInvalid("prompt");
      notifyError("Describe what to create.");
      promptRef.current?.focus();
      return;
    }
    setInvalid(null);
    setBusy(true);
    const body: GenerationRequest = {
      model: model.id,
      prompt: model.prompt ? draft.prompt.trim() : "",
      negative_prompt: model.negative_prompt && draft.negativePrompt.trim() ? draft.negativePrompt.trim() : null,
      params: requestParams(model, explicit),
      inputs: attached.flatMap((input) => (input.asset ? [{ asset_id: input.asset.id, role: input.role }] : [])),
      count: draft.count,
      ...(preset ? { preset: preset.id } : {}),
    };
    try {
      const { jobs } = await api.generate(body);
      addJobs(jobs);
    } catch (error) {
      if (error instanceof ApiError && error.field) setInvalid(error.field);
      notifyError(errorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  const main = model?.params.filter((spec) => !spec.advanced) ?? [];
  const advanced = model?.params.filter((spec) => spec.advanced) ?? [];
  const changedAdvanced = advanced.some((spec) => spec.key in explicit) || Boolean(model?.negative_prompt && draft.negativePrompt.trim());
  const providerLabel = model ? providerName(model.provider, providers) : "";
  const price = estimate ? (estimate.kind === "total" ? money(estimate.usd) : estimate.text) : null;
  const generateLabel = !model
    ? "Choose a model first"
    : !model.configured
      ? `Set up ${providerLabel} to use ${model.name}`
      : `Generate ${plural(draft.count, model.output, kindPlural[model.output])}${estimate?.kind === "total" ? `, estimated ${money(estimate.usd)}` : ""}`;

  return (
    <form
      aria-label="Composer"
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      {model && <PresetsStrip mode={mode} selected={preset?.id ?? null} />}
      <div
        className={`composer ${dragging ? "dragging" : ""}`}
        onDragOver={(event) => {
          if (!event.dataTransfer.types.includes("Files") && !event.dataTransfer.types.includes(ASSET_DRAG_TYPE)) return;
          event.preventDefault();
          setDragging(true);
        }}
        onDragLeave={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false);
        }}
        onDrop={onDrop}
      >
        {model && model.inputs.length > 0 && <InputSlots model={model} mode={mode} inputs={inputs} invalidRole={invalid?.startsWith("inputs.") ? invalid.slice(7) : null} />}
        <div className="prompt-line">
          {model?.prompt === false ? (
            <p className="prompt-off">
              <Icon name="info" />
              {model.name} works from its inputs. No prompt needed.
            </p>
          ) : (
            <>
              <label className="sr-only" htmlFor="prompt">
                Prompt
              </label>
              <textarea
                id="prompt"
                ref={promptRef}
                className="prompt"
                rows={2}
                maxLength={50000}
                value={draft.prompt}
                aria-invalid={invalid === "prompt" || undefined}
                placeholder={mode === "video" ? "Describe the shot and the motion" : mode === "audio" ? "Describe the sound" : "Describe your image"}
                onChange={(event) => setPrompt(event.target.value)}
                onPaste={onPromptPaste}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                    event.preventDefault();
                    void submit();
                  }
                }}
              />
            </>
          )}
        </div>
        <div className="composer-toolbar">
          <div className="composer-controls" aria-label="Generation settings" role="group">
            <ModelPicker
              models={list}
              model={model}
              loading={models.loading}
              onSelect={(next) => {
                if (model) selectModel(mode, next, model);
                else selectModel(mode, next);
              }}
            />
            {model &&
              main.map((spec) => (
                <ParamControl
                  key={spec.key}
                  spec={spec}
                  value={values[spec.key] ?? null}
                  invalid={invalid === `params.${spec.key}`}
                  onChange={(value) => setParam(model.id, spec.key, value)}
                />
              ))}
            {model && (
              <Control label={`Number of ${kindPlural[model.output]}`} value={plural(draft.count, model.output, kindPlural[model.output])}>
                {COUNTS.map((count) => (
                  <Choice key={count} selected={draft.count === count} onPick={() => setCount(count)}>
                    {plural(count, model.output, kindPlural[model.output])}
                  </Choice>
                ))}
                {model.max_outputs < draft.count && <p className="control-help">{model.name} makes {model.max_outputs} per run, so this starts several runs.</p>}
              </Control>
            )}
            {model && (advanced.length > 0 || model.negative_prompt) && (
              <Control
                label="More settings"
                icon="more_horiz"
                caret={false}
                className={changedAdvanced ? "has-changes" : ""}
                invalid={Boolean(invalid?.startsWith("params.") && advanced.some((spec) => `params.${spec.key}` === invalid))}
                size="wide"
              >
                <div className="more-fields">
                  {model.negative_prompt && (
                    <label className="field">
                      <span>Avoid</span>
                      <textarea rows={2} maxLength={4000} value={draft.negativePrompt} placeholder="Things to leave out (negative prompt)" onChange={(event) => setNegativePrompt(event.target.value)} />
                    </label>
                  )}
                  {advanced.map((spec) => (
                    <ParamField key={spec.key} spec={spec} value={values[spec.key] ?? null} onChange={(value) => setParam(model.id, spec.key, value)} />
                  ))}
                  {Object.keys(explicit).length > 0 && (
                    <button type="button" className="text-button" onClick={() => resetParams(model.id)}>
                      <Icon name="restart_alt" />
                      Reset to defaults
                    </button>
                  )}
                </div>
              </Control>
            )}
          </div>
          <button type="submit" className="primary-button generate" disabled={busy || !model} aria-label={generateLabel} aria-keyshortcuts="Control+Enter Meta+Enter">
            <span className="generate-text">{busy ? "Starting" : model && !model.configured ? "Set up" : "Generate"}</span>
            {price && model?.configured && <span className="generate-price">{price}</span>}
            {busy ? <Spinner /> : <Icon name={model && !model.configured ? "key" : "arrow_upward"} />}
          </button>
        </div>
      </div>
      {models.error && (
        <p className="notice error composer-error" role="alert">
          {models.error}
        </p>
      )}
    </form>
  );
}
