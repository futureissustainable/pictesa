import { lazy, Suspense, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { togglePin, useModelPrefs, useProviders } from "../../lib/catalog";
import { priceHint, taskName } from "../../lib/format";
import { readJson, writeJson } from "../../lib/prefs";
import { Link } from "../../lib/router";
import type { ArenaScore, ModelSpec, Pricing, ProviderKind, ProviderState } from "../../lib/types";
import { Control } from "../../ui/control";
import { Icon } from "../../ui/icon";

const WorkflowImportDialog = lazy(() => import("../workflows/import-dialog"));

type Where = "all" | "local" | "cloud";
type Sort = "recommended" | "arena" | "price" | "newest" | "provider";

export interface PickerFilters {
  q: string;
  provider: string;
  task: string;
  where: Where;
  configuredOnly: boolean;
  sort: Sort;
}

const SORTS: { value: Sort; label: string }[] = [
  { value: "recommended", label: "Recommended" },
  { value: "arena", label: "Best rated" },
  { value: "price", label: "Lowest price" },
  { value: "newest", label: "Newest" },
  { value: "provider", label: "By provider" },
];
const SORT_KEY = "pictesa.modelSort";
const savedSort = (): Sort => {
  const value = readJson<string>(SORT_KEY, "recommended");
  return SORTS.some((sort) => sort.value === value) ? (value as Sort) : "recommended";
};

const LIMIT = 150;
const RECENT = 5;

function kindOf(model: ModelSpec, providers: ProviderState[] | null): ProviderKind {
  const kind = providers?.find((provider) => provider.id === model.provider)?.kind;
  return kind ?? (model.pricing?.unit === "local" ? "local" : "cloud");
}

export function providerName(id: string, providers: ProviderState[] | null): string {
  return providers?.find((provider) => provider.id === id)?.name ?? id;
}

function matches(model: ModelSpec, filters: PickerFilters, providers: ProviderState[] | null): boolean {
  if (filters.configuredOnly && !model.configured) return false;
  if (filters.provider && model.provider !== filters.provider) return false;
  if (filters.task && !model.tasks.includes(filters.task as ModelSpec["tasks"][number])) return false;
  if (filters.where !== "all") {
    const local = kindOf(model, providers) === "local";
    if ((filters.where === "local") !== local) return false;
  }
  const query = filters.q.trim().toLowerCase();
  if (!query) return true;
  const haystack = [model.name, model.vendor, model.provider, model.id, ...(model.tags ?? []), ...model.tasks.map(taskName)].join(" ").toLowerCase();
  return query.split(/\s+/).every((word) => haystack.includes(word));
}

function rank(model: ModelSpec): number {
  return (model.configured ? 0 : 2) + (model.tags?.includes("featured") ? 0 : 1);
}

const recommended = (a: ModelSpec, b: ModelSpec) => rank(a) - rank(b) || a.name.localeCompare(b.name);

/** The model's standing in the arena of the chosen task; without a task, its first arena. */
function arenaScore(model: ModelSpec, task: string): ArenaScore | undefined {
  return task ? model.arena?.find((score) => score.arena === task) : model.arena?.[0];
}

/** A comparable cost per output: a 5 s clip for per-second video prices, one image per megapixel. */
function unitCost(pricing: Pricing | null | undefined): number {
  if (!pricing || pricing.unit === "variable") return Infinity;
  if (pricing.unit === "local") return 0;
  return pricing.unit === "per_second" ? pricing.usd * 5 : pricing.usd;
}

function sorter(filters: PickerFilters, providers: ProviderState[] | null): (a: ModelSpec, b: ModelSpec) => number {
  // Differences of two missing values are NaN, which falls through to the next comparison.
  switch (filters.sort) {
    case "arena":
      return (a, b) => (arenaScore(b, filters.task)?.rating ?? -Infinity) - (arenaScore(a, filters.task)?.rating ?? -Infinity) || recommended(a, b);
    case "price":
      return (a, b) => unitCost(a.pricing) - unitCost(b.pricing) || recommended(a, b);
    case "newest":
      return (a, b) => (b.released ?? "").localeCompare(a.released ?? "") || recommended(a, b);
    case "provider":
      return (a, b) => providerName(a.provider, providers).localeCompare(providerName(b.provider, providers)) || recommended(a, b);
    default:
      return recommended;
  }
}

/** The top three ranks get a filled cup in their medal colour. */
const MEDALS = ["gold", "silver", "bronze"];

const arenaLabel = (score: ArenaScore) =>
  `LMArena ${taskName(score.arena).toLowerCase()}: #${score.rank} of ${score.of} as ${score.name}, ${score.votes.toLocaleString("en")} votes, ${score.published.split("-").reverse().join(".")}`;

/** The model trigger on the control line and its searchable, filterable list. */
export function ModelPicker({ models, model, onSelect, loading }: { models: ModelSpec[]; model?: ModelSpec; onSelect(model: ModelSpec): void; loading: boolean }) {
  const [filters, setFilters] = useState<PickerFilters | null>(null);
  const [importing, setImporting] = useState(false);
  const current = filters ?? { q: "", provider: "", task: "", where: "all", configuredOnly: models.some((item) => item.configured), sort: savedSort() };
  return (
    <>
    <Control
      label="Model"
      size="large"
      className="model-trigger"
      value={model?.name ?? (loading ? "Loading models" : "Choose a model")}
      panelClassName="model-panel"
      showTitle={false}
    >
      {(close) => (
        <ModelList
          models={models}
          selected={model}
          filters={current}
          setFilters={setFilters}
          onSelect={(next) => {
            onSelect(next);
            // Keep the filters, start the next search fresh.
            setFilters({ ...current, q: "" });
            close();
          }}
          onImport={() => {
            close();
            setImporting(true);
          }}
        />
      )}
    </Control>
    {importing && (
      <Suspense fallback={null}>
        <WorkflowImportDialog onClose={() => setImporting(false)} />
      </Suspense>
    )}
    </>
  );
}

/** "+ Compare": adds another model to run the same prompt side by side. */
export function ComparePicker({ models, exclude, disabled, onAdd }: { models: ModelSpec[]; exclude: string[]; disabled: boolean; onAdd(model: ModelSpec): void }) {
  const [filters, setFilters] = useState<PickerFilters | null>(null);
  const current = filters ?? { q: "", provider: "", task: "", where: "all", configuredOnly: true, sort: savedSort() };
  return (
    <Control
      label="Compare with another model"
      icon="add"
      value="Compare"
      caret={false}
      size="large"
      className="compare-trigger"
      panelClassName="model-panel"
      showTitle={false}
      disabled={disabled}
    >
      {(close) => (
        <ModelList
          models={models.filter((model) => !exclude.includes(model.id))}
          filters={current}
          setFilters={setFilters}
          onSelect={(model) => {
            onAdd(model);
            close();
          }}
        />
      )}
    </Control>
  );
}

function ModelList({
  models,
  selected,
  filters,
  setFilters,
  onSelect,
  onImport,
}: {
  models: ModelSpec[];
  selected?: ModelSpec;
  filters: PickerFilters;
  setFilters(filters: PickerFilters): void;
  onSelect(model: ModelSpec): void;
  onImport?(): void;
}) {
  const providers = useProviders().data;
  const prefs = useModelPrefs();
  const listRef = useRef<HTMLDivElement>(null);
  const set = (change: Partial<PickerFilters>) => setFilters({ ...filters, ...change });

  const providerIds = useMemo(() => [...new Set(models.map((model) => model.provider))].sort(), [models]);
  const tasks = useMemo(() => [...new Set(models.flatMap((model) => model.tasks))], [models]);
  const hasLocal = useMemo(() => models.some((model) => kindOf(model, providers) === "local"), [models, providers]);
  const found = useMemo(() => models.filter((model) => matches(model, filters, providers)).sort(sorter(filters, providers)), [models, filters, providers]);
  // Pinned and recent models lead the list; each model shows once.
  const sections = useMemo(() => {
    const byId = new Map(found.map((model) => [model.id, model]));
    const pick = (ids: string[]) => ids.flatMap((id) => byId.get(id) ?? []);
    const pinned = pick(prefs.pinned);
    const recent = pick(prefs.recent.filter((id) => !prefs.pinned.includes(id))).slice(0, RECENT);
    const lead = new Set([...pinned, ...recent].map((model) => model.id));
    const rest = found.filter((model) => !lead.has(model.id)).slice(0, LIMIT);
    const groups: { title?: string; models: ModelSpec[] }[] = [
      { title: "Pinned", models: pinned },
      { title: "Recent", models: recent },
    ];
    if (filters.sort === "provider") {
      for (const model of rest) {
        const title = providerName(model.provider, providers);
        const group = groups.at(-1)?.title === title ? groups.at(-1) : undefined;
        if (group) group.models.push(model);
        else groups.push({ title, models: [model] });
      }
    } else {
      groups.push({ title: lead.size ? "All models" : undefined, models: rest });
    }
    return groups.filter((group) => group.models.length);
  }, [found, prefs, filters.sort, providers]);
  const pinned = new Set(prefs.pinned);
  const hidden = found.length - sections.reduce((sum, group) => sum + group.models.length, 0);

  const moveFocus = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const rows = [...(listRef.current?.querySelectorAll<HTMLButtonElement>(".model-row") ?? [])];
    if (!rows.length) return;
    event.preventDefault();
    const index = rows.indexOf(document.activeElement as HTMLButtonElement);
    const next = index === -1 ? (event.key === "ArrowDown" ? 0 : rows.length - 1) : Math.max(0, Math.min(rows.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)));
    rows[next]?.focus();
  };

  return (
    <div className="model-picker" onKeyDown={moveFocus}>
      <label className="search model-search">
        <Icon name="search" />
        <span className="sr-only">Search models</span>
        <input
          type="search"
          value={filters.q}
          placeholder="Search models, makers, tasks"
          data-autofocus=""
          onChange={(event) => set({ q: event.target.value })}
          onKeyDown={(event) => {
            if (event.key === "Escape" && filters.q) {
              event.preventDefault();
              event.stopPropagation();
              set({ q: "" });
            }
          }}
        />
      </label>
      <div className="model-filters">
        <label className="select-chip">
          <span className="sr-only">Sort models</span>
          <select
            value={filters.sort}
            onChange={(event) => {
              const sort = event.target.value as Sort;
              writeJson(SORT_KEY, sort);
              set({ sort });
            }}
          >
            {SORTS.map((sort) => (
              <option key={sort.value} value={sort.value}>
                {sort.label}
              </option>
            ))}
          </select>
        </label>
        <label className="select-chip">
          <span className="sr-only">Provider</span>
          <select value={filters.provider} onChange={(event) => set({ provider: event.target.value })}>
            <option value="">All providers</option>
            {providerIds.map((id) => (
              <option key={id} value={id}>
                {providerName(id, providers)}
              </option>
            ))}
          </select>
        </label>
        {tasks.length > 1 && (
          <label className="select-chip">
            <span className="sr-only">Task</span>
            <select value={filters.task} onChange={(event) => set({ task: event.target.value })}>
              <option value="">All tasks</option>
              {tasks.map((task) => (
                <option key={task} value={task}>
                  {taskName(task)}
                </option>
              ))}
            </select>
          </label>
        )}
        {hasLocal && (
          <div className="segmented segmented-small" role="group" aria-label="Where models run">
            {(["all", "local", "cloud"] as const).map((where) => (
              <button key={where} type="button" aria-pressed={filters.where === where} onClick={() => set({ where })}>
                {where === "all" ? "All" : where === "local" ? "Local" : "Cloud"}
              </button>
            ))}
          </div>
        )}
        <label className="switch-chip">
          <input type="checkbox" role="switch" checked={filters.configuredOnly} onChange={(event) => set({ configuredOnly: event.target.checked })} />
          Configured only
        </label>
      </div>
      <div className="model-list" ref={listRef}>
        {sections.map((group) => (
          <section key={group.title ?? "models"} className="model-section">
            {group.title && <h3 className="model-group">{group.title}</h3>}
            <div role="list" aria-label={group.title ?? "Models"}>
              {group.models.map((model) => (
                <ModelRow
                  key={model.id}
                  model={model}
                  providers={providers}
                  task={filters.task}
                  selected={model.id === selected?.id}
                  pinned={pinned.has(model.id)}
                  onSelect={onSelect}
                />
              ))}
            </div>
          </section>
        ))}
        {!found.length && (
          <div className="model-empty">
            <p>No models match.</p>
            <button type="button" className="text-button" onClick={() => setFilters({ q: "", provider: "", task: "", where: "all", configuredOnly: false, sort: filters.sort })}>
              Clear filters
            </button>
          </div>
        )}
      </div>
      <div className="model-footer">
        <span>
          {hidden > 0 ? `${found.length - hidden} of ${found.length} shown. Search to narrow.` : `${found.length} of ${models.length} models`}
        </span>
        <span className="model-footer-actions">
          {onImport && (
            <button type="button" className="text-button" onClick={onImport}>
              <Icon name="add" />
              Add ComfyUI workflow
            </button>
          )}
          <Link to="/settings" className="text-button">
            <Icon name="key" />
            Providers
          </Link>
        </span>
      </div>
    </div>
  );
}

function ModelRow({
  model,
  providers,
  task,
  selected,
  pinned,
  onSelect,
}: {
  model: ModelSpec;
  providers: ProviderState[] | null;
  task: string;
  selected: boolean;
  pinned: boolean;
  onSelect(model: ModelSpec): void;
}) {
  const price = priceHint(model.pricing);
  const provider = providerName(model.provider, providers);
  const maker = model.vendor && model.vendor !== provider ? `${model.vendor} on ${provider}` : provider;
  const score = arenaScore(model, task);
  return (
    <div role="listitem" className="model-item">
      <button type="button" className={`model-row ${model.configured ? "" : "unconfigured"}`} aria-pressed={selected} title={model.description ?? undefined} onClick={() => onSelect(model)}>
        <span className="model-main">
          <span className="model-title">
            <span className="model-name">{model.name}</span>
            {score && (
              <span className={`model-arena ${MEDALS[score.rank - 1] ?? ""}`} title={arenaLabel(score)}>
                <Icon name="trophy" filled={score.rank <= MEDALS.length} />
                <span className="sr-only">Arena rank</span>
                {score.rank}
              </span>
            )}
          </span>
          <span className="model-sub">
            {maker}
            {!model.configured && <span className="model-setup"> Needs setup</span>}
          </span>
        </span>
        <span className="model-side">
          {model.open_weights && <span className="badge">Open weights</span>}
          {price && (
            <span className="model-price" title={model.billed ? "Paid from your Pictesa balance" : undefined}>
              {model.billed && <Icon name="account_balance_wallet" />}
              {price}
            </span>
          )}
        </span>
        <Icon name="check" className={`model-check ${selected ? "" : "invisible"}`} />
      </button>
      <button type="button" className="model-pin" aria-pressed={pinned} aria-label={`${pinned ? "Unpin" : "Pin"} ${model.name}`} title={pinned ? "Unpin" : "Pin to the top"} onClick={() => togglePin(model.id)}>
        <Icon name="keep" filled={pinned} />
      </button>
    </div>
  );
}
