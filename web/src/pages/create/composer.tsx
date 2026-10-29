import { useEffect, useMemo, useRef, useState, type ClipboardEvent, type DragEvent } from "react";
import { api, ApiError, errorMessage } from "../../lib/api";
import { ASSET_DRAG_TYPE, fileKind, readDraggedAsset, rejectionMessage, routeFiles, uploadInto } from "../../lib/attach";
import { defaultModel, useFullModel, useModels, usePresets, useProviders } from "../../lib/catalog";
import {
  activeInputs,
  addInput,
  composer,
  effectiveParams,
  inputKey,
  requestParams,
  resetParams,
  selectModel,
  setCompare,
  setCount,
  setNegativePrompt,
  setParam,
  setPrompt,
  slotInputs,
  useComposer,
} from "../../lib/composer";
import { intentOf, variantFor } from "../../lib/families";
import { missingModels, useModelDownload } from "../../lib/downloads";
import { bytes, estimateCost, kindPlural, money, plural } from "../../lib/format";
import { MAX_COMPARE, trackCompare } from "../../lib/compare";
import { addJobs } from "../../lib/jobs";
import { notify, notifyError } from "../../lib/notices";
import { navigate } from "../../lib/router";
import type { Asset, GenerationRequest, MediaKind, ModelSpec, ParamValues } from "../../lib/types";
import { Choice, Control } from "../../ui/control";
import { Icon, Spinner } from "../../ui/icon";
import { InputSlots, slotLabel } from "./inputs";
import { ComparePicker, ModelPicker, providerName } from "./model-picker";
import { ParamControl, ParamField } from "./params";
import { LoraPicker } from "./loras";
import { BatchDialog } from "./batch";
import { PromptHistoryControl, PromptsControl, SavePromptDialog, usePromptSuggestions } from "./prompts";
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
  const listed = list.find((item) => item.id === draft.modelByMode[mode]) ?? defaultModel(list);
  // Discovered gateway models are listed without params; their full spec loads on selection.
  const full = useFullModel(listed);
  const model = full.spec;
  const download = useModelDownload(model?.id);
  const lastFull = useRef<Partial<Record<MediaKind, ModelSpec>>>({});
  const preset = presets?.find((item) => item.id === draft.presetByMode[mode]) ?? null;
  const explicit = (model && draft.paramsByModel[model.id]) || NO_PARAMS;
  const values = useMemo(() => (model ? effectiveParams(model, explicit, preset) : {}), [model, explicit, preset]);
  const inputs = draft.inputsByMode[mode];
  const attached = activeInputs(model, inputs);
  const intent = intentOf(mode, inputs);
  // A model listed without an image slot can still edit through another member of its family.
  const editVersion = useMemo(
    () => (model && listed && mode === "image" && !model.inputs.some((slot) => slot.kind === "image") ? variantFor(list, listed, "image_edit") : null),
    [model, listed, list, mode],
  );
  const uploading = attached.some((input) => input.upload);
  const compare = (draft.compareByMode[mode] ?? []).filter((id) => id !== model?.id);
  const characterId = draft.characterByMode[mode] ?? null;
  const compareModels = compare.map((id) => list.find((item) => item.id === id)).filter((item): item is ModelSpec => Boolean(item));
  // With compare models the total is the sum of every model's estimate, when all can be computed.
  const estimate = useMemo(() => {
    if (!model) return null;
    const own = estimateCost(model, values, draft.count);
    if (!compareModels.length) return own;
    let usd = own?.kind === "total" ? own.usd : null;
    for (const other of compareModels) {
      const part = estimateCost(other, values, draft.count);
      usd = usd !== null && part?.kind === "total" ? usd + part.usd : null;
    }
    return usd === null ? null : ({ kind: "total", usd } as const);
  }, [model, values, draft.count, compareModels]);
  const [batchOpen, setBatchOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);
  const [focused, setFocused] = useState(false);
  const [suggestionIndex, setSuggestionIndex] = useState(-1);
  const [suggestionsOff, setSuggestionsOff] = useState(false);
  const suggestions = usePromptSuggestions(draft.prompt);
  const showSuggestions = focused && !suggestionsOff && suggestions.length > 0;
  const [busy, setBusy] = useState(false);
  const [invalid, setInvalid] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const [enhancing, setEnhancing] = useState(false);
  const [enhanceError, setEnhanceError] = useState<string | null>(null);
  const [undo, setUndo] = useState<{ before: string; after: string } | null>(null);

  // "Improve prompt": the server rewrites a short idea into a detailed prompt (2 to 10 s).
  async function improvePrompt() {
    const before = draft.prompt;
    if (!before.trim() || enhancing) return;
    setEnhancing(true);
    setEnhanceError(null);
    try {
      const { prompt: after } = await api.enhancePrompt(before.trim(), mode);
      if (composer.get().prompt !== before) return; // The person kept typing; keep their text.
      setPrompt(after);
      setUndo({ before, after });
      promptRef.current?.focus();
    } catch (error) {
      setEnhanceError(errorMessage(error));
    } finally {
      setEnhancing(false);
    }
  }

  function restorePrompt() {
    if (!undo) return;
    setPrompt(undo.before);
    setUndo(null);
    promptRef.current?.focus();
  }

  // The inline Undo stays for a few seconds; Ctrl or Cmd+Z works while the text is unchanged.
  useEffect(() => {
    if (!undo) return;
    const timer = setTimeout(() => setUndo(null), 12000);
    return () => clearTimeout(timer);
  }, [undo]);

  // Adding or removing a reference switches to the member of the model's family that does that.
  const lastIntent = useRef(intent);
  useEffect(() => {
    if (lastIntent.current === intent) return;
    lastIntent.current = intent;
    const next = listed && intent ? variantFor(list, listed, intent) : null;
    if (next && next.id !== listed?.id) selectModel(mode, next, model);
  }, [intent, listed, list, mode, model]);

  // Remember the default model so the choice is stable across reloads.
  useEffect(() => {
    if (listed && draft.modelByMode[mode] !== listed.id) selectModel(mode, listed);
  }, [listed, mode, draft.modelByMode]);

  // A model whose spec just loaded inherits fitting choices from the previous model of this mode.
  useEffect(() => {
    if (!model) return;
    const previous = lastFull.current[mode];
    if (previous && previous.id !== model.id && !draft.paramsByModel[model.id]) selectModel(mode, model, previous);
    lastFull.current[mode] = model;
  }, [model, mode, draft.paramsByModel]);

  // Settings loaded from the library: put the cursor in the prompt.
  useEffect(() => {
    if (draft.revision) promptRef.current?.focus({ preventScroll: true });
  }, [draft.revision]);

  useEffect(() => setInvalid(null), [model?.id, mode]);

  // Images dropped on a model without an image slot go to its edit version once that loads.
  const pending = useRef<{ modelId: string; files: File[]; asset?: Asset } | null>(null);
  const switchToEdit = (files: File[], asset?: Asset) => {
    if (!editVersion || !model || editVersion.id === model.id) return false;
    pending.current = { modelId: editVersion.id, files, asset };
    selectModel(mode, editVersion, model);
    return true;
  };

  const attachFiles = (files: File[]) => {
    if (!model || !files.length) return;
    const { placements, rejected } = routeFiles(model, inputs, files);
    placements.forEach((placement) => void uploadInto(mode, placement.slot.role, placement.files));
    if (!rejected.length || (rejected.some((file) => fileKind(file) === "image") && switchToEdit(rejected))) return;
    notifyError(rejectionMessage(model, rejected));
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
    if (asset) return attachAsset(asset);
    attachFiles([...event.dataTransfer.files]);
  };

  function attachAsset(asset: Asset) {
    if (!model) return;
    const slot = model.inputs.find((candidate) => candidate.kind === asset.kind && slotInputs(candidate, inputs).length < candidate.max);
    if (slot) return addInput(mode, { key: inputKey(), role: slot.role, asset });
    if (asset.kind === "image" && switchToEdit([], asset)) return;
    notifyError(`${model.name} has no free slot for ${kindPlural[asset.kind]}.`);
  }

  useEffect(() => {
    if (!model || pending.current?.modelId !== model.id) return;
    const { files, asset } = pending.current;
    pending.current = null;
    if (asset) attachAsset(asset);
    attachFiles(files);
    // Runs once per switched model; the handlers read the current state.
  }, [model]);

  function generationRequest(): GenerationRequest {
    if (!model) throw new Error("Choose a model first.");
    return {
      model: model.id,
      prompt: model.prompt ? draft.prompt.trim() : "",
      negative_prompt: model.negative_prompt && draft.negativePrompt.trim() ? draft.negativePrompt.trim() : null,
      params: { ...requestParams(model, explicit), ...(model.meta?.supports_loras === true && draft.loraByModel?.[model.id] ? { loras: [draft.loraByModel[model.id]!] } : {}) },
      inputs: attached.flatMap((input) => (input.asset ? [{ asset_id: input.asset.id, role: input.role }] : [])),
      count: draft.count,
      ...(preset ? { preset: preset.id } : {}),
      ...(compare.length ? { compare } : {}),
      ...(characterId && model.inputs.some((slot) => slot.role === "reference" && slot.kind === "image") ? { character: characterId } : {}),
    } satisfies GenerationRequest;
  }

  async function submit() {
    if (!model || busy || download.active) return;
    if (missingFiles.length) {
      await download.start();
      return;
    }
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
    const body = generationRequest();
    try {
      const { batch_id: batchId, jobs } = await api.generate(body);
      if (compare.length) trackCompare(batchId, jobs);
      addJobs(jobs);
    } catch (error) {
      if (error instanceof ApiError && error.field === "billing") {
        notify(errorMessage(error), { tone: "error", action: { label: "Add funds", run: () => navigate("/settings/billing") } });
      } else {
        if (error instanceof ApiError && error.field) setInvalid(error.field);
        notifyError(errorMessage(error));
      }
    } finally {
      setBusy(false);
    }
  }

  const missingFiles = missingModels(model);
  const missingSize = missingFiles.reduce((sum, file) => sum + (file.size ?? 0), 0);
  const main = model?.params.filter((spec) => !spec.advanced) ?? [];
  const advanced = model?.params.filter((spec) => spec.advanced) ?? [];
  const changedAdvanced = advanced.some((spec) => spec.key in explicit) || Boolean(model?.negative_prompt && draft.negativePrompt.trim());
  const providerLabel = model ? providerName(model.provider, providers) : "";
  const price = estimate ? (estimate.kind === "total" ? money(estimate.usd) : estimate.text) : null;
  const generateLabel = !model
    ? full.loading
      ? "Loading the model's settings"
      : "Choose a model first"
    : download.active
      ? `Downloading ${model.name}'s model files, ${download.percent}%`
      : missingFiles.length
        ? `Download ${model.name}'s model files${missingSize ? `, ${bytes(missingSize)}` : ""}`
        : !model.configured
          ? `Set up ${providerLabel} to use ${model.name}`
          : `Generate ${plural(draft.count, model.output, kindPlural[model.output])}${estimate?.kind === "total" ? `, estimated ${money(estimate.usd)}${model.billed ? " from your balance" : ""}` : ""}`;

  return (
    <form
      aria-label="Composer"
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
      onKeyDown={(event) => {
        // Enter in a field inside a popover (model search, seed, steps) must not start a run.
        if (event.key === "Enter" && event.target instanceof HTMLInputElement) event.preventDefault();
      }}
    >
      {listed && <PresetsStrip mode={mode} selected={preset?.id ?? null} />}
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
        {model && <LoraPicker model={model} characterId={characterId} />}
        <div className="composer-main">
        {model && (model.inputs.length > 0 || model.meta?.supports_loras || characterId) && <InputSlots model={model} mode={mode} inputs={inputs} invalidRole={invalid?.startsWith("inputs.") ? invalid.slice(7) : null} />}
        {editVersion && editVersion.id !== model?.id && (
          <button type="button" className="text-button edit-an-image" onClick={() => selectModel(mode, editVersion, model)}>
            <Icon name="add_photo_alternate" />
            Edit an image
          </button>
        )}
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
              <div className="prompt-body">
              <textarea
                id="prompt"
                ref={promptRef}
                className="prompt"
                rows={2}
                maxLength={50000}
                value={draft.prompt}
                role="combobox"
                aria-autocomplete="list"
                aria-expanded={Boolean(showSuggestions)}
                aria-controls="prompt-suggestions"
                aria-activedescendant={showSuggestions && suggestionIndex >= 0 ? `prompt-suggestion-${suggestionIndex}` : undefined}
                onFocus={() => setFocused(true)}
                onBlur={() => { setFocused(false); setSuggestionIndex(-1); }}
                aria-invalid={invalid === "prompt" || undefined}
                aria-busy={enhancing || undefined}
                aria-describedby={enhanceError ? "prompt-help-error" : undefined}
                placeholder={mode === "video" ? "Describe the shot and the motion" : mode === "audio" ? "Describe the sound" : "Describe your image"}
                onChange={(event) => {
                  setPrompt(event.target.value);
                  setSuggestionsOff(false);
                  setSuggestionIndex(-1);
                  setEnhanceError(null);
                }}
                onPaste={onPromptPaste}
                onKeyDown={(event) => {
                  if (showSuggestions && !event.ctrlKey && !event.metaKey) {
                    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                      event.preventDefault();
                      setSuggestionIndex((index) => event.key === "ArrowDown" ? (index + 1) % suggestions.length : index < 0 ? suggestions.length - 1 : (index + suggestions.length - 1) % suggestions.length);
                      return;
                    }
                    if (event.key === "Escape") { event.preventDefault(); setSuggestionsOff(true); return; }
                    if (event.key === "Enter" && suggestionIndex >= 0) {
                      event.preventDefault(); setPrompt(suggestions[suggestionIndex].prompt); setSuggestionsOff(true); setSuggestionIndex(-1); return;
                    }
                  }
                  if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                    event.preventDefault();
                    void submit();
                  }
                  // The rewrite replaced the text programmatically, so the browser cannot undo it.
                  if (event.key.toLowerCase() === "z" && (event.metaKey || event.ctrlKey) && !event.shiftKey && undo && draft.prompt === undo.after) {
                    event.preventDefault();
                    restorePrompt();
                  }
                }}
              />
              <div id="prompt-suggestions" role="listbox" aria-label="Past prompt suggestions" className="prompt-suggestions" hidden={!showSuggestions}>
                {showSuggestions && suggestions.map((item, index) => <button key={item.prompt} id={`prompt-suggestion-${index}`} type="button" role="option" aria-selected={index === suggestionIndex} tabIndex={-1} className="choice" onPointerDown={(event) => event.preventDefault()} onClick={() => { setPrompt(item.prompt); setSuggestionsOff(true); promptRef.current?.focus(); }}><span className="choice-body prompt-excerpt">{item.prompt}</span></button>)}
              </div>
              {enhanceError && (
                <p className="prompt-error" id="prompt-help-error" role="alert">
                  {enhanceError}
                </p>
              )}
              </div>
              <div className="prompt-tools">
                <PromptHistoryControl />
                <PromptsControl />
                {undo && draft.prompt === undo.after && (
                  <button type="button" className="text-button prompt-undo" onClick={restorePrompt}>
                    Undo
                  </button>
                )}
                {mode !== "audio" && (
                  <button
                    type="button"
                    className="icon-button"
                    aria-label="Improve prompt"
                    title="Improve prompt"
                    disabled={enhancing || !draft.prompt.trim()}
                    onClick={() => void improvePrompt()}
                  >
                    {enhancing ? <Spinner /> : <Icon name="auto_awesome" />}
                  </button>
                )}
              </div>
            </>
          )}
        </div>
        </div>
        <div className="composer-toolbar">
          <div className="composer-controls" aria-label="Generation settings" role="group">
            <ModelPicker key={mode} models={list} model={listed} intent={intent} loading={models.loading} onSelect={(next) => selectModel(mode, next, model)} />
            {compareModels.map((other) => (
              <span key={other.id} className="compare-chip">
                <span>{other.name}</span>
                <button type="button" aria-label={`Remove ${other.name} from the comparison`} onClick={() => setCompare(mode, compare.filter((id) => id !== other.id))}>
                  <Icon name="close" />
                </button>
              </span>
            ))}
            {model && (
              <ComparePicker
                models={list}
                intent={intent}
                exclude={[model.id, ...compare]}
                disabled={compare.length >= MAX_COMPARE}
                onAdd={(other) => setCompare(mode, [...compare, other.id])}
              />
            )}
            {full.loading && (
              <span className="control-note" role="status">
                <Spinner />
                Loading settings
              </span>
            )}
            {full.error && (
              <button type="button" className="control-trigger" onClick={full.retry} title={full.error}>
                <Icon name="refresh" />
                <span className="control-value">Settings did not load. Retry</span>
              </button>
            )}
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
            {model && (
              <Control
                label="More settings"
                icon="more_horiz"
                caret={false}
                className={changedAdvanced ? "has-changes" : ""}
                invalid={Boolean(invalid?.startsWith("params.") && advanced.some((spec) => `params.${spec.key}` === invalid))}
                size="wide"
              >
                <div className="more-fields">
                  <Choice icon="grid_view" onPick={() => setBatchOpen(true)} disabled={uploading || !model.configured}>Batch</Choice>
                  <Choice icon="draft" onPick={() => setSaveOpen(true)} disabled={!draft.prompt.trim()}>Save prompt</Choice>
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
          <button type="submit" className="primary-button generate" disabled={busy || !model} aria-label={generateLabel} aria-keyshortcuts="Control+Enter Meta+Enter" title={model?.billed ? "Paid from your Pictesa balance" : undefined}>
            <span className="generate-text">
              {busy ? "Starting" : download.active ? `Downloading ${download.percent}%` : missingFiles.length ? "Download models" : model && !model.configured ? "Set up" : "Generate"}
            </span>
            {missingFiles.length > 0 && !download.active && missingSize > 0 && <span className="generate-price">{bytes(missingSize)}</span>}
            {price && model?.configured && (
              <span className="generate-price">
                {model.billed && <Icon name="account_balance_wallet" />}
                {price}
              </span>
            )}
            {busy || download.active ? <Spinner /> : <Icon name={missingFiles.length ? "download" : model && !model.configured ? "key" : "arrow_upward"} />}
          </button>
        </div>
      </div>
      {batchOpen && model && <BatchDialog model={model} request={generationRequest()} onClose={() => setBatchOpen(false)} />}
      {saveOpen && model && <SavePromptDialog request={{ ...generationRequest(), params: values }} onClose={() => setSaveOpen(false)} />}
      {models.error && (
        <p className="notice error composer-error" role="alert">
          {models.error}
        </p>
      )}
    </form>
  );
}
