import { useEffect, useMemo, useRef, useState, type DragEvent, type FormEvent } from "react";
import { ApiError, errorMessage, uploadFiles } from "../../lib/api";
import { acceptFor, ASSET_DRAG_TYPE, clearUploadErrors, fileKind, readDraggedAsset, refusal, reportUploadError, uploadErrors } from "../../lib/attach";
import { toggleFavorite } from "../../lib/assets";
import { useCharacters } from "../../lib/characters";
import { fallbackModelName, useModelNames, useModels, usePresets } from "../../lib/catalog";
import { emitLocal } from "../../lib/events";
import { estimateCost, kindIcon, kindPlural, money, plural } from "../../lib/format";
import { notify, notifyError } from "../../lib/notices";
import { draftFor, fieldNeeded, fillTemplate, isFilled, recipeDrafts, startRun, stepRuns, updateDraft, useRecipeResults, useRecipes, type DraftValue, type RecipeDraft } from "../../lib/recipes";
import { Link, navigate, useLocation, withParams } from "../../lib/router";
import { useStore } from "../../lib/store";
import type { Asset, MediaKind, ModelSpec, Preset, Recipe, RecipeField, RecipeInput } from "../../lib/types";
import { openViewer, useSequence } from "../../lib/viewer";
import { Thumb } from "../../ui/asset-card";
import { Choice, Control } from "../../ui/control";
import { AssetGrid } from "../../ui/grid";
import { Icon, Spinner, type IconName } from "../../ui/icon";
import { SizeSlider, useImageSize } from "../../ui/size-slider";
import { JobsTray } from "../create/jobs-tray";
import { UploadErrors } from "../create/inputs";
import { LibraryPicker } from "../create/library-picker";
import { ModeSwitch } from "../create/mode-switch";

// Apps: the studio's gallery of recipes. A card shows what an app makes and what it needs;
// opening one puts its form in the dock, where it asks for its inputs and runs.

const noMore = () => undefined;

const fieldIcons: Record<string, IconName> = { camera: "videocam", lens: "camera", light: "light_mode", duration: "timer", setting: "landscape", style: "palette", angle: "photo_size_select_large", angles: "photo_size_select_large", look: "contrast", aspect_ratio: "aspect_ratio", direction: "light_mode", mood: "light_mode" };

const pictures = import.meta.glob<string>("../../assets/apps/*.svg", { eager: true, query: "?url", import: "default" });
const builtInPicture = (name: string) => pictures[`../../assets/apps/${name}.svg`];

const groups: { kind: MediaKind; title: string }[] = [
  { kind: "image", title: "Image apps" },
  { kind: "video", title: "Video apps" },
  { kind: "audio", title: "Audio apps" },
];

/** "an image editing model, such as ..." becomes "an image editing model". */
const shortNeeds = (needs: string) => needs.split(/,?\s+such as\s/)[0] ?? needs;

/** Model specs for every output the recipes use, keyed by id. */
function useModelIndex(): Map<string, ModelSpec> {
  const image = useModels("image");
  const video = useModels("video");
  const audio = useModels("audio");
  return useMemo(() => new Map([...(image.data ?? []), ...(video.data ?? []), ...(audio.data ?? [])].map((model) => [model.id, model])), [image.data, video.data, audio.data]);
}

export default function AppsPage() {
  const { data, error } = useRecipes();
  const url = useLocation();
  const size = useImageSize();
  const feedRef = useRef<HTMLElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const models = useModelIndex();
  const results = useRecipeResults();
  const nameOf = useModelNames(["image", "video"]);
  const recipes = data ?? [];
  const chosen = url.searchParams.get("app") ?? url.searchParams.get("recipe");
  const selected = recipes.find((recipe) => recipe.id === chosen);
  useSequence(results.items, false, noMore);

  // Old links to /recipes keep working.
  useEffect(() => {
    if (url.pathname === "/recipes") navigate(`/apps${chosen ? `?app=${encodeURIComponent(chosen)}` : ""}`, { replace: true });
  }, [url.pathname, chosen]);

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
      <section className="creation-feed" ref={feedRef} aria-labelledby="apps-title">
        <div className="feed-heading">
          <h1 id="apps-title" className="sr-only">
            Apps
          </h1>
          <ModeSwitch active="apps" />
          <SizeSlider />
        </div>
        {!data ? (
          <div className="feed-loading" role={error ? "alert" : "status"}>
            {error ? <Icon name="cloud_off" /> : <Spinner />}
            {error ?? "Loading apps"}
          </div>
        ) : (
          groups.map(({ kind, title }) => {
            const list = recipes.filter((recipe) => recipe.output === kind);
            if (!list.length) return null;
            return (
              <section key={kind} className="app-group" aria-labelledby={`apps-${kind}`}>
                <h2 id={`apps-${kind}`} className="feed-subtitle">
                  {title}
                </h2>
                <ul className="app-grid">
                  {list.map((recipe) => (
                    <AppCard key={recipe.id} recipe={recipe} selected={recipe.id === selected?.id} />
                  ))}
                </ul>
              </section>
            );
          })
        )}
        <h2 className="feed-subtitle">Results</h2>
        {results.items.length ? (
          <AssetGrid
            items={results.items}
            size={size}
            scroller={feedRef}
            label="App results"
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

/** The card picture: a built-in illustration, a library image, or the illustration for its output. */
function AppPicture({ recipe }: { recipe: Recipe }) {
  const [broken, setBroken] = useState(false);
  const fallback = builtInPicture(recipe.output === "video" ? "video" : "image");
  const own = recipe.picture ? (builtInPicture(recipe.picture) ?? `/thumbs/${encodeURIComponent(recipe.picture)}`) : undefined;
  return <img className="app-picture" src={(!broken && own) || fallback} alt="" width={400} height={300} loading="lazy" decoding="async" onError={() => setBroken(true)} />;
}

/** What an app asks for: its input files, or its main text field when it takes no files. */
function needsOf(recipe: Recipe): { icon: IconName; label: string; optional: boolean }[] {
  if (recipe.inputs.length) return recipe.inputs.map((input) => ({ icon: kindIcon[input.kind], label: input.label, optional: !input.required }));
  const text = recipe.fields.find((field) => field.type === "text");
  return text ? [{ icon: "edit", label: text.label, optional: !text.required }] : [];
}

function AppCard({ recipe, selected }: { recipe: Recipe; selected: boolean }) {
  const missing = recipe.steps.find((_, index) => !recipe.step_models[index]);
  const asks = needsOf(recipe);
  const open = () => {
    if (recipe.open) navigate(recipe.open);
    else navigate(withParams({ app: selected ? null : recipe.id, recipe: null }), { replace: true });
  };
  return (
    <li>
      <button type="button" className={`app-card ${recipe.runnable ? "" : "blocked"}`} aria-pressed={recipe.open ? undefined : selected} onClick={open}>
        <span className="app-visual">
          <AppPicture recipe={recipe} />
        </span>
        <span className="app-body">
          <span className="app-name">{recipe.name}</span>
          {recipe.description && <span className="app-line">{recipe.description}</span>}
          {asks.length > 0 && (
            <span className="app-asks">
              <span className="sr-only">Needs: </span>
              {asks.map((item) => (
                <span key={item.label} className={`app-ask ${item.optional ? "optional" : ""}`}>
                  <Icon name={item.icon} />
                  {item.label}
                  {item.optional && <span className="sr-only">, optional</span>}
                </span>
              ))}
            </span>
          )}
          {missing && <span className="app-blocked">Needs {missing.needs ? shortNeeds(missing.needs) : "a model"}</span>}
        </span>
      </button>
    </li>
  );
}

function stepModel(recipe: Recipe, index: number, draft: RecipeDraft, models: Map<string, ModelSpec>): ModelSpec | undefined {
  const step = recipe.steps[index]!;
  const id = draft.models[step.id] ?? recipe.step_models[index] ?? undefined;
  return id ? models.get(id) : undefined;
}

/** The output of the step that uses a preset field, so the field offers presets for it. */
function presetOutput(recipe: Recipe, key: string): MediaKind | undefined {
  return recipe.steps.find((step) => step.preset?.replace(/\s/g, "") === `{{${key}}}`)?.output;
}

/** The app form in the dock: its inputs, settings and a model per step, generated from the recipe. */
function RecipeComposer({ recipe, models }: { recipe: Recipe; models: Map<string, ModelSpec> }) {
  const draft = useStore(recipeDrafts)[recipe.id] ?? draftFor(recipe);
  const videoPresets = usePresets("video").data ?? [];
  const imagePresets = usePresets("image").data ?? [];
  const presets = [...videoPresets, ...imagePresets];
  const [busy, setBusy] = useState(false);
  const [invalid, setInvalid] = useState<string | null>(null);
  const formRef = useRef<HTMLFormElement>(null);
  const primary = recipe.fields.find((field) => field.type === "text");
  const refused = useStore(uploadErrors).filter((item) => item.scope.startsWith(`recipe:${recipe.id}:`));
  const others = recipe.fields.filter((field) => field !== primary);
  const uploading = Object.values(draft.inputs).some((list) => list.some((item) => !item.asset));
  const missing = recipe.steps.filter((_, index) => !recipe.step_models[index]);

  // Opening an app moves focus to the first thing it asks for.
  useEffect(() => {
    formRef.current?.querySelector<HTMLElement>(".slot-add, textarea")?.focus({ preventScroll: true });
  }, []);

  const setValue = (key: string, value: DraftValue) => updateDraft(recipe, (current) => ({ ...current, values: { ...current.values, [key]: value } }));
  const close = () => navigate(withParams({ app: null, recipe: null }), { replace: true });

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
      sum += estimate.usd * stepRuns(step, draft.values);
    }
    return sum;
  }, [recipe, draft, models]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy || !recipe.runnable) return;
    const missingField = recipe.fields.find((field) => fieldNeeded(field, draft) && !isFilled(draft.values[field.key]));
    if (missingField) {
      setInvalid(missingField.key);
      notifyError(missingField.required_without && missingField.help ? missingField.help : `Fill in ${missingField.label.toLowerCase()}.`);
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
        ...(recipe.character && draft.character ? { character: draft.character } : {}),
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
      ref={formRef}
      aria-label={`${recipe.name} app`}
      noValidate
      onSubmit={(event) => void submit(event)}
      onKeyDown={(event) => {
        // Enter in a field inside a popover must not start a run.
        if (event.key === "Enter" && event.target instanceof HTMLInputElement) event.preventDefault();
      }}
    >
      <div className="composer recipe-composer">
        {missing.length > 0 && (
          <p className="app-setup" role="status">
            <Icon name="info" />
            <span>
              {missing.map((step) => `"${step.label}" needs ${step.needs ?? "a set-up model"}.`).join(" ")} <Link to="/settings">Open providers</Link>
            </span>
          </p>
        )}
        <div className="composer-main">
          {recipe.inputs.length > 0 && (
            <div className="slots-area">
              <div className="slots" role="group" aria-label="Inputs">
                {recipe.inputs.map((input) => (
                  <RecipeInputSlot key={input.key} recipe={recipe} input={input} draft={draft} invalid={invalid === input.key} />
                ))}
              </div>
              <UploadErrors errors={refused} />
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
                  value={typeof draft.values[primary.key] === "string" ? (draft.values[primary.key] as string) : ""}
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
          <div className="composer-controls" role="group" aria-label="App settings">
            <span className="control-trigger recipe-label">
              <Icon name="apps" />
              <span className="control-value">{recipe.name}</span>
              <button type="button" className="icon-button app-close" aria-label={`Close ${recipe.name}`} title="Close" onClick={close}>
                <Icon name="close" />
              </button>
            </span>
            {others.map((field) => {
              const output = field.type === "preset" ? presetOutput(recipe, field.key) : undefined;
              return <FieldControl key={field.key} field={field} value={draft.values[field.key]} presets={output ? presets.filter((preset) => preset.output === output) : presets} invalid={invalid === field.key} onChange={(value) => setValue(field.key, value)} />;
            })}
            {recipe.character && <CharacterControl recipe={recipe} value={draft.character ?? null} invalid={invalid === "character"} />}
            {recipe.steps.map((step, index) => {
              const current = stepModel(recipe, index, draft, models);
              const options = step.models.map((id) => models.get(id)).filter((model): model is ModelSpec => Boolean(model?.configured));
              const label = recipe.steps.length > 1 ? `Model for ${step.label.toLowerCase()}` : "Model";
              return (
                <Control key={step.id} label={label} icon={kindIcon[step.output]} value={current?.name ?? (recipe.step_models[index] ? fallbackModelName(recipe.step_models[index]!) : "No model")} invalid={invalid === `models.${step.id}`} size="wide">
                  {options.length ? (
                    options.map((model) => (
                      <Choice key={model.id} selected={model.id === current?.id} onPick={() => updateDraft(recipe, (draftNow) => ({ ...draftNow, models: { ...draftNow.models, [step.id]: model.id } }))}>
                        {model.name}
                        <small>{model.vendor ?? model.provider}</small>
                      </Choice>
                    ))
                  ) : (
                    <p className="control-help">
                      No model for this step is set up. It needs {step.needs ?? "one of its listed models"}. <Link to="/settings">Open providers</Link>
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

/** A character or brand kit that applies to every step of the app. */
function CharacterControl({ recipe, value, invalid }: { recipe: Recipe; value: string | null; invalid: boolean }) {
  const { data } = useCharacters();
  const selected = data?.find((character) => character.id === value) ?? null;
  const pick = (id: string | null) => updateDraft(recipe, (current) => ({ ...current, character: id }));
  return (
    <Control label="Character or brand kit" icon={selected?.kind === "brand" ? "palette" : "face"} value={selected?.name ?? "No brand kit"} invalid={invalid} size="wide">
      <div className="control-list">
        <Choice selected={!selected} onPick={() => pick(null)}>
          None
        </Choice>
        {(data ?? []).map((character) => (
          <Choice key={character.id} selected={character.id === value} onPick={() => pick(character.id)}>
            {character.name}
            <small>
              {character.kind === "brand" ? "Brand kit, " : ""}
              {plural(character.assets.length, "image", "images")}
            </small>
          </Choice>
        ))}
      </div>
      <p className="control-help">
        Its images join every step that takes images. A brand kit also adds its colors and words. <Link to="/characters">Manage characters</Link>
      </p>
    </Control>
  );
}

function FieldControl({ field, value, presets, invalid, onChange }: { field: RecipeField; value: DraftValue | undefined; presets: Preset[]; invalid: boolean; onChange(value: DraftValue): void }) {
  const icon = fieldIcons[field.key] ?? (field.type === "preset" && field.category ? fieldIcons[field.category] : undefined);
  const text = typeof value === "string" ? value : "";
  if (field.type === "text") {
    return (
      <Control label={field.label} icon={icon} value={text ? (text.length > 16 ? `${text.slice(0, 15)}…` : text) : field.label} invalid={invalid} showTitle={false}>
        <div className="control-field">
          <label className="field">
            <span>{field.label}</span>
            {field.multiline ? <textarea rows={3} value={text} data-autofocus="" onChange={(event) => onChange(event.target.value)} /> : <input type="text" value={text} data-autofocus="" onChange={(event) => onChange(event.target.value)} />}
            {field.help && <small>{field.help}</small>}
          </label>
        </div>
      </Control>
    );
  }
  if (field.type === "multi") {
    const picked = Array.isArray(value) ? value : [];
    const toggle = (option: string) => onChange(picked.includes(option) ? picked.filter((item) => item !== option) : (field.options ?? []).filter((item) => item === option || picked.includes(item)));
    return (
      <Control label={field.label} icon={icon} value={picked.length === 1 ? picked[0] : `${picked.length} ${field.label.toLowerCase()}`} invalid={invalid} size="wide">
        <div className="control-list">
          {(field.options ?? []).map((option) => (
            <Choice key={option} selected={picked.includes(option)} keepOpen onPick={() => toggle(option)}>
              {option}
            </Choice>
          ))}
        </div>
        {field.help && <p className="control-help">{field.help}</p>}
      </Control>
    );
  }
  const options =
    field.type === "preset"
      ? presets.filter((preset) => !field.category || preset.category === field.category).map((preset) => ({ value: preset.id, label: preset.name, note: preset.description ?? undefined }))
      : (field.options ?? []).map((option) => ({ value: option, label: field.key === "duration" && /^\d+$/.test(option) ? `${option}s` : option, note: undefined as string | undefined }));
  const current = options.find((option) => option.value === text);
  const shown = current?.label ?? (text ? text : "None");
  return (
    <Control label={field.label} icon={icon} value={icon ? shown : `${field.label} ${shown}`} invalid={invalid} size={field.type === "preset" ? "wide" : "default"}>
      <div className="control-list">
        {!field.required && field.type === "preset" && (
          <Choice selected={!text} onPick={() => onChange("")}>
            None
          </Choice>
        )}
        {options.map((option) => (
          <Choice key={option.value} selected={option.value === text} onPick={() => onChange(option.value)}>
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

  const scope = `recipe:${recipe.id}:${input.key}`;
  const upload = (files: File[]) => {
    clearUploadErrors(scope);
    const fitting = files.filter((file) => fileKind(file) === input.kind).slice(0, Math.max(0, room));
    if (fitting.length < files.length) notifyError(`${input.label} takes up to ${input.max} ${kindPlural[input.kind]}.`);
    for (const file of fitting) {
      const key = `${file.name}-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
      const preview = input.kind === "image" ? URL.createObjectURL(file) : null;
      setItems((list) => [...list, { key, name: file.name, progress: null, preview }]);
      uploadFiles([file], (progress) => setItems((list) => list.map((item) => (item.key === key ? { ...item, progress } : item))))
        .then(({ assets, errors }) => {
          const asset = assets[0];
          if (!asset) throw new Error(errors[0]?.error ?? `${file.name} did not upload.`);
          emitLocal({ asset });
          setItems((list) => list.map((item) => (item.key === key ? { key, asset } : item)));
        })
        .catch((reason: unknown) => {
          setItems((list) => list.filter((item) => item.key !== key));
          reportUploadError(scope, file.name, refusal(reason, file.name));
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
      aria-label={`${input.label}${input.required ? ", required" : ", optional"}`}
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
              {!input.required && !items.length && <small className="slot-count">Optional</small>}
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

