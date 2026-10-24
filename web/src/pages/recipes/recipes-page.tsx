import { useEffect, useMemo, useRef, useState, type DragEvent, type FormEvent } from "react";
import { ApiError, errorMessage, uploadFiles } from "../../lib/api";
import { acceptFor, ASSET_DRAG_TYPE, fileKind, readDraggedAsset } from "../../lib/attach";
import { toggleFavorite } from "../../lib/assets";
import { fallbackModelName, useModelNames, useModels, usePresets } from "../../lib/catalog";
import { emitLocal } from "../../lib/events";
import { estimateCost, kindIcon, kindLabel, kindPlural, money } from "../../lib/format";
import { notify, notifyError } from "../../lib/notices";
import { draftFor, fillTemplate, recipeDrafts, startRun, updateDraft, useRecipeResults, useRecipes, type RecipeDraft } from "../../lib/recipes";
import { Link, navigate, useLocation, withParams } from "../../lib/router";
import { useStore } from "../../lib/store";
import type { Asset, ModelSpec, Recipe, RecipeField, RecipeInput } from "../../lib/types";
import { openViewer, useSequence } from "../../lib/viewer";
import { Thumb } from "../../ui/asset-card";
import { Choice, Control } from "../../ui/control";
import { AssetGrid } from "../../ui/grid";
import { Icon, Spinner, type IconName } from "../../ui/icon";
import { SizeSlider, useImageSize } from "../../ui/size-slider";
import { JobsTray } from "../create/jobs-tray";
import { LibraryPicker } from "../create/library-picker";
import { ModeSwitch } from "../create/mode-switch";

const noMore = () => undefined;

const fieldIcons: Record<string, IconName> = { camera: "videocam", lens: "camera", light: "light_mode", duration: "timer", setting: "landscape", style: "palette", angle: "photo_size_select_large" };

/** Model specs for every output the recipes use, keyed by id. */
function useModelIndex(): Map<string, ModelSpec> {
  const image = useModels("image");
  const video = useModels("video");
  const audio = useModels("audio");
  return useMemo(() => new Map([...(image.data ?? []), ...(video.data ?? []), ...(audio.data ?? [])].map((model) => [model.id, model])), [image.data, video.data, audio.data]);
}

export default function RecipesPage() {
  const { data, error } = useRecipes();
  const url = useLocation();
  const size = useImageSize();
  const feedRef = useRef<HTMLElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const models = useModelIndex();
  const results = useRecipeResults();
  const nameOf = useModelNames(["image", "video"]);
  const recipes = data ?? [];
  const selected = recipes.find((recipe) => recipe.id === url.searchParams.get("recipe")) ?? recipes.find((recipe) => recipe.runnable) ?? recipes[0];
  useSequence(results.items, false, noMore);

  useEffect(() => {
    const dock = dockRef.current;
    if (!dock) return;
    const observer = new ResizeObserver(() => document.documentElement.style.setProperty("--dock-height", `${dock.offsetHeight}px`));
    observer.observe(dock);
    return () => {
      observer.disconnect();
      document.documentElement.style.removeProperty("--dock-height");
    };
  }, []);

  return (
    <div className="workspace workspace-creation">
      <section className="creation-feed" ref={feedRef} aria-labelledby="recipes-title">
        <div className="feed-heading">
          <h1 id="recipes-title" className="sr-only">
            Recipes
          </h1>
          <ModeSwitch active="recipes" />
          <SizeSlider />
        </div>
        {!data ? (
          <div className="feed-loading" role={error ? "alert" : "status"}>
            {error ? <Icon name="cloud_off" /> : <Spinner />}
            {error ?? "Loading recipes"}
          </div>
        ) : (
          <div className="recipe-grid" role="list" aria-label="Recipes">
            {recipes.map((recipe) => (
              <RecipeCard key={recipe.id} recipe={recipe} models={models} selected={recipe.id === selected?.id} />
            ))}
          </div>
        )}
        <h2 className="feed-subtitle">Results</h2>
        {results.items.length ? (
          <AssetGrid
            items={results.items}
            size={size}
            scroller={feedRef}
            label="Recipe results"
            titleOf={(asset) => nameOf(asset.model)}
            onOpen={(asset) => openViewer(asset)}
            onFavorite={(asset) => void toggleFavorite(asset)}
          />
        ) : (
          <p className="hint feed-hint" role="status">
            {results.loading ? <Spinner /> : null}
            {results.loading ? "Loading results" : results.error ? results.error : "Finished runs land here. Every step also stays in the library."}
          </p>
        )}
      </section>
      <div className="composer-dock" ref={dockRef}>
        <JobsTray />
        {selected && <RecipeComposer key={selected.id} recipe={selected} models={models} />}
      </div>
    </div>
  );
}

function stepModel(recipe: Recipe, index: number, draft: RecipeDraft, models: Map<string, ModelSpec>): ModelSpec | undefined {
  const step = recipe.steps[index]!;
  const id = draft.models[step.id] ?? recipe.step_models[index] ?? undefined;
  return id ? models.get(id) : undefined;
}

function RecipeCard({ recipe, models, selected }: { recipe: Recipe; models: Map<string, ModelSpec>; selected: boolean }) {
  const draft = useStore(recipeDrafts)[recipe.id] ?? draftFor(recipe);
  const missing = recipe.steps.filter((_, index) => !recipe.step_models[index]).map((step) => step.label);
  return (
    <div role="listitem">
      <button
        type="button"
        className={`recipe-card ${recipe.runnable ? "" : "blocked"}`}
        aria-pressed={selected}
        onClick={() => navigate(withParams({ recipe: recipe.id }), { replace: true })}
      >
        <span className="recipe-top">
          <Icon name={kindIcon[recipe.output]} />
          <span className="recipe-name">{recipe.name}</span>
          <span className="recipe-output">{kindLabel[recipe.output]}</span>
        </span>
        {recipe.description && <span className="recipe-description">{recipe.description}</span>}
        <span className="recipe-steps">
          {recipe.steps.map((step, index) => {
            const model = stepModel(recipe, index, draft, models);
            return (
              <span key={step.id} className="recipe-step">
                {index > 0 && <Icon name="chevron_right" />}
                <span>
                  {step.label}
                  <small>{model?.name ?? (recipe.step_models[index] ? fallbackModelName(recipe.step_models[index]!) : "No model set up")}</small>
                </span>
              </span>
            );
          })}
        </span>
        {!recipe.runnable && <span className="recipe-blocked">Needs a set-up model for: {missing.length ? missing.join(", ") : "every step"}.</span>}
      </button>
    </div>
  );
}

/** The recipe form in the dock: fields, inputs and a model per step, generated from the recipe. */
function RecipeComposer({ recipe, models }: { recipe: Recipe; models: Map<string, ModelSpec> }) {
  const draft = useStore(recipeDrafts)[recipe.id] ?? draftFor(recipe);
  const videoPresets = usePresets("video").data ?? [];
  const imagePresets = usePresets("image").data ?? [];
  const presets = [...videoPresets, ...imagePresets];
  const [busy, setBusy] = useState(false);
  const [invalid, setInvalid] = useState<string | null>(null);
  const primary = recipe.fields.find((field) => field.type === "text");
  const others = recipe.fields.filter((field) => field !== primary);
  const uploading = Object.values(draft.inputs).some((list) => list.some((item) => !item.asset));

  const setValue = (key: string, value: string) => updateDraft(recipe, (current) => ({ ...current, values: { ...current.values, [key]: value } }));

  // Best-effort total: only when every step's model publishes a price this form can compute.
  const total = useMemo(() => {
    let sum = 0;
    for (let index = 0; index < recipe.steps.length; index++) {
      const step = recipe.steps[index]!;
      const model = stepModel(recipe, index, draft, models);
      if (!model) return null;
      if (model.pricing?.unit === "local") continue;
      const params = Object.fromEntries(Object.entries(step.params ?? {}).map(([key, value]) => [key, typeof value === "string" ? fillTemplate(value, draft.values) : value]));
      const estimate = estimateCost(model, params, 1);
      if (estimate?.kind !== "total") return null;
      sum += estimate.usd;
    }
    return sum;
  }, [recipe, draft, models]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy || !recipe.runnable) return;
    const missingField = recipe.fields.find((field) => field.required && !draft.values[field.key]?.trim());
    if (missingField) {
      setInvalid(missingField.key);
      notifyError(`Fill in ${missingField.label.toLowerCase()}.`);
      return;
    }
    const missingInput = recipe.inputs.find((input) => input.required && !(draft.inputs[input.key] ?? []).some((item) => item.asset));
    if (missingInput) {
      setInvalid(missingInput.key);
      notifyError(`Add ${missingInput.label.toLowerCase()}.`);
      return;
    }
    if (uploading) {
      notify("Wait for the upload to finish.");
      return;
    }
    setInvalid(null);
    setBusy(true);
    try {
      const overrides = Object.fromEntries(Object.entries(draft.models).filter(([stepId, model]) => recipe.step_models[recipe.steps.findIndex((step) => step.id === stepId)] !== model));
      await startRun(recipe, {
        values: Object.fromEntries(Object.entries(draft.values).filter(([key]) => recipe.fields.some((field) => field.key === key))),
        inputs: Object.fromEntries(recipe.inputs.map((input) => [input.key, (draft.inputs[input.key] ?? []).flatMap((item) => (item.asset ? [item.asset.id] : []))])),
        ...(Object.keys(overrides).length ? { models: overrides } : {}),
      });
    } catch (reason) {
      if (reason instanceof ApiError && reason.field === "billing") {
        notify(errorMessage(reason), { tone: "error", action: { label: "Add funds", run: () => navigate("/settings/billing") } });
      } else {
        if (reason instanceof ApiError && reason.field) setInvalid(reason.field.replace(/^(values|inputs)\./, ""));
        notifyError(errorMessage(reason));
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      aria-label={`${recipe.name} recipe`}
      noValidate
      onSubmit={(event) => void submit(event)}
      onKeyDown={(event) => {
        // Enter in a field inside a popover must not start a run.
        if (event.key === "Enter" && event.target instanceof HTMLInputElement) event.preventDefault();
      }}
    >
      <div className="composer recipe-composer">
        <div className="composer-main">
          {recipe.inputs.length > 0 && (
            <div className="slots" role="group" aria-label="Inputs">
              {recipe.inputs.map((input) => (
                <RecipeInputSlot key={input.key} recipe={recipe} input={input} draft={draft} invalid={invalid === input.key} />
              ))}
            </div>
          )}
          <div className="prompt-line">
            {primary ? (
              <>
                <label className="sr-only" htmlFor="recipe-prompt">
                  {primary.label}
                </label>
                <textarea
                  id="recipe-prompt"
                  className="prompt"
                  rows={2}
                  value={draft.values[primary.key] ?? ""}
                  placeholder={primary.help ?? primary.label}
                  aria-invalid={invalid === primary.key || undefined}
                  onChange={(event) => setValue(primary.key, event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                      event.preventDefault();
                      event.currentTarget.form?.requestSubmit();
                    }
                  }}
                />
              </>
            ) : (
              <p className="prompt-off">
                <Icon name="info" />
                {recipe.description ?? "No text needed. Add the inputs and run."}
              </p>
            )}
          </div>
        </div>
        <div className="composer-toolbar">
          <div className="composer-controls" role="group" aria-label="Recipe settings">
            <span className="control-trigger recipe-label" aria-hidden="true">
              <Icon name="conversion_path" />
              <span className="control-value">{recipe.name}</span>
            </span>
            {others.map((field) => (
              <FieldControl key={field.key} field={field} value={draft.values[field.key] ?? ""} presets={presets} invalid={invalid === field.key} onChange={(value) => setValue(field.key, value)} />
            ))}
            {recipe.steps.map((step, index) => {
              const current = stepModel(recipe, index, draft, models);
              const options = step.models.map((id) => models.get(id)).filter((model): model is ModelSpec => Boolean(model?.configured));
              return (
                <Control key={step.id} label={`Model for ${step.label.toLowerCase()}`} icon={kindIcon[step.output]} value={current?.name ?? "No model"} size="wide">
                  {options.length ? (
                    options.map((model) => (
                      <Choice key={model.id} selected={model.id === current?.id} onPick={() => updateDraft(recipe, (draftNow) => ({ ...draftNow, models: { ...draftNow.models, [step.id]: model.id } }))}>
                        {model.name}
                        <small>{model.vendor ?? model.provider}</small>
                      </Choice>
                    ))
                  ) : (
                    <p className="control-help">
                      No model for this step is set up. <Link to="/settings">Open providers</Link>
                    </p>
                  )}
                </Control>
              );
            })}
          </div>
          <button type="submit" className="primary-button generate" disabled={busy || !recipe.runnable} aria-label={`Run ${recipe.name}${total !== null && total > 0 ? `, estimated ${money(total)}` : ""}`}>
            <span className="generate-text">{busy ? "Starting" : "Run"}</span>
            {total !== null && total > 0 && <span className="generate-price">{money(total)}</span>}
            {busy ? <Spinner /> : <Icon name="arrow_upward" />}
          </button>
        </div>
      </div>
    </form>
  );
}

function FieldControl({ field, value, presets, invalid, onChange }: { field: RecipeField; value: string; presets: { id: string; name: string; category: string; description?: string | null }[]; invalid: boolean; onChange(value: string): void }) {
  const icon = fieldIcons[field.key] ?? (field.type === "preset" && field.category ? fieldIcons[field.category] : undefined);
  if (field.type === "text") {
    return (
      <Control label={field.label} icon={icon} value={value ? (value.length > 16 ? `${value.slice(0, 15)}…` : value) : field.label} invalid={invalid} showTitle={false}>
        <div className="control-field">
          <label className="field">
            <span>{field.label}</span>
            {field.multiline ? <textarea rows={3} value={value} data-autofocus="" onChange={(event) => onChange(event.target.value)} /> : <input type="text" value={value} data-autofocus="" onChange={(event) => onChange(event.target.value)} />}
            {field.help && <small>{field.help}</small>}
          </label>
        </div>
      </Control>
    );
  }
  const options =
    field.type === "preset"
      ? presets.filter((preset) => !field.category || preset.category === field.category).map((preset) => ({ value: preset.id, label: preset.name, note: preset.description ?? undefined }))
      : (field.options ?? []).map((option) => ({ value: option, label: field.key === "duration" && /^\d+$/.test(option) ? `${option}s` : option, note: undefined as string | undefined }));
  const current = options.find((option) => option.value === value);
  const shown = current?.label ?? (value ? value : "None");
  return (
    <Control label={field.label} icon={icon} value={icon ? shown : `${field.label} ${shown}`} invalid={invalid} size={field.type === "preset" ? "wide" : "default"}>
      <div className="control-list">
        {!field.required && field.type === "preset" && (
          <Choice selected={!value} onPick={() => onChange("")}>
            None
          </Choice>
        )}
        {options.map((option) => (
          <Choice key={option.value} selected={option.value === value} onPick={() => onChange(option.value)}>
            {option.label}
            {option.note && <small>{option.note}</small>}
          </Choice>
        ))}
      </div>
      {field.help && <p className="control-help">{field.help}</p>}
    </Control>
  );
}

function RecipeInputSlot({ recipe, input, draft, invalid }: { recipe: Recipe; input: RecipeInput; draft: RecipeDraft; invalid: boolean }) {
  const items = draft.inputs[input.key] ?? [];
  const room = input.max - items.length;
  const fileRef = useRef<HTMLInputElement>(null);
  const [over, setOver] = useState(false);
  const [picking, setPicking] = useState(false);

  const setItems = (change: (list: RecipeDraft["inputs"][string]) => RecipeDraft["inputs"][string]) =>
    updateDraft(recipe, (current) => ({ ...current, inputs: { ...current.inputs, [input.key]: change(current.inputs[input.key] ?? []) } }));

  const addAssets = (assets: Asset[]) => {
    const fitting = assets.filter((asset) => asset.kind === input.kind);
    if (fitting.length < assets.length) notifyError(`${input.label} takes ${kindPlural[input.kind]}.`);
    setItems((list) => [...list, ...fitting.filter((asset) => !list.some((item) => item.asset?.id === asset.id)).map((asset) => ({ key: asset.id, asset }))].slice(0, input.max));
  };

  const upload = (files: File[]) => {
    const fitting = files.filter((file) => fileKind(file) === input.kind).slice(0, Math.max(0, room));
    if (fitting.length < files.length) notifyError(`${input.label} takes up to ${input.max} ${kindPlural[input.kind]}.`);
    for (const file of fitting) {
      const key = `${file.name}-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
      const preview = input.kind === "image" ? URL.createObjectURL(file) : null;
      setItems((list) => [...list, { key, name: file.name, progress: null, preview }]);
      uploadFiles([file], (progress) => setItems((list) => list.map((item) => (item.key === key ? { ...item, progress } : item))))
        .then(([asset]) => {
          if (!asset) throw new Error(`${file.name} did not upload.`);
          emitLocal({ asset });
          setItems((list) => list.map((item) => (item.key === key ? { key, asset } : item)));
        })
        .catch((reason: unknown) => {
          setItems((list) => list.filter((item) => item.key !== key));
          notifyError(`${file.name}: ${errorMessage(reason)}`);
        })
        .finally(() => {
          if (preview) setTimeout(() => URL.revokeObjectURL(preview), 5000);
        });
    }
  };

  const drop = (event: DragEvent) => {
    event.preventDefault();
    setOver(false);
    const asset = readDraggedAsset(event.dataTransfer);
    if (asset) return addAssets([asset]);
    upload([...event.dataTransfer.files]);
  };

  return (
    <div
      className={`slot-group ${over ? "drop-over" : ""} ${invalid ? "invalid" : ""}`}
      role="group"
      aria-label={`${input.label}${input.required ? ", required" : ""}`}
      onDragOver={(event) => {
        if (!event.dataTransfer.types.includes("Files") && !event.dataTransfer.types.includes(ASSET_DRAG_TYPE)) return;
        event.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={drop}
      onPaste={(event) => {
        const files = [...event.clipboardData.files];
        if (files.length) {
          event.preventDefault();
          upload(files);
        }
      }}
    >
      {items.map((item) => (
        <div key={item.key} className="slot-tile" title={item.asset?.prompt ?? item.name ?? input.label}>
          {item.asset ? <Thumb asset={item.asset} /> : item.preview ? <img src={item.preview} alt="" /> : <span className="thumb-placeholder"><Icon name={kindIcon[input.kind]} /></span>}
          {!item.asset && (
            <span className="slot-progress" role="status" aria-label={`Uploading ${item.name ?? "file"}`}>
              {typeof item.progress === "number" ? `${Math.round(item.progress * 100)}%` : <Spinner />}
            </span>
          )}
          <span className="slot-caption">{input.label}</span>
          <button type="button" className="slot-remove" aria-label={`Remove from ${input.label.toLowerCase()}`} onClick={() => setItems((list) => list.filter((entry) => entry.key !== item.key))}>
            <Icon name="close" />
          </button>
        </div>
      ))}
      {room > 0 && (
        <Control
          label={`Add to ${input.label.toLowerCase()}`}
          className={`slot-add ${input.required && !items.length ? "required" : ""}`}
          showTitle={false}
          placement="above"
          trigger={
            <>
              <Icon name={input.kind === "image" ? "add_photo_alternate" : kindIcon[input.kind]} />
              <span className="slot-label">{input.label}</span>
              {input.max > 1 && items.length > 0 && (
                <small className="slot-count">
                  {items.length}/{input.max}
                </small>
              )}
            </>
          }
        >
          <Choice icon="upload" onPick={() => fileRef.current?.click()}>
            Upload {room > 1 ? kindPlural[input.kind] : `a ${input.kind}`}
          </Choice>
          <Choice icon="photo_library" onPick={() => setPicking(true)}>
            Choose from library
          </Choice>
        </Control>
      )}
      <input
        ref={fileRef}
        type="file"
        className="sr-only"
        tabIndex={-1}
        aria-hidden="true"
        accept={acceptFor(input.kind)}
        multiple={room > 1}
        onChange={(event) => {
          const files = [...(event.target.files ?? [])];
          event.target.value = "";
          upload(files);
        }}
      />
      {picking && (
        <LibraryPicker
          kind={input.kind}
          room={Math.max(1, room)}
          title={`Choose for ${input.label.toLowerCase()}`}
          onClose={() => setPicking(false)}
          onPick={addAssets}
        />
      )}
    </div>
  );
}
