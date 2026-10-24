import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useProviders } from "../../lib/catalog";
import { priceHint, taskName } from "../../lib/format";
import { Link } from "../../lib/router";
import type { ModelSpec, ProviderKind, ProviderState } from "../../lib/types";
import { Control } from "../../ui/control";
import { Icon } from "../../ui/icon";

type Where = "all" | "local" | "cloud";

export interface PickerFilters {
  q: string;
  provider: string;
  task: string;
  where: Where;
  configuredOnly: boolean;
}

const LIMIT = 150;

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

/** The model trigger on the control line and its searchable, filterable list. */
export function ModelPicker({ models, model, onSelect, loading }: { models: ModelSpec[]; model?: ModelSpec; onSelect(model: ModelSpec): void; loading: boolean }) {
  const [filters, setFilters] = useState<PickerFilters | null>(null);
  const current = filters ?? { q: "", provider: "", task: "", where: "all", configuredOnly: models.some((item) => item.configured) };
  return (
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
}: {
  models: ModelSpec[];
  selected?: ModelSpec;
  filters: PickerFilters;
  setFilters(filters: PickerFilters): void;
  onSelect(model: ModelSpec): void;
}) {
  const providers = useProviders().data;
  const listRef = useRef<HTMLDivElement>(null);
  const set = (change: Partial<PickerFilters>) => setFilters({ ...filters, ...change });

  const providerIds = useMemo(() => [...new Set(models.map((model) => model.provider))].sort(), [models]);
  const tasks = useMemo(() => [...new Set(models.flatMap((model) => model.tasks))], [models]);
  const hasLocal = useMemo(() => models.some((model) => kindOf(model, providers) === "local"), [models, providers]);
  const found = useMemo(
    () => models.filter((model) => matches(model, filters, providers)).sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name)),
    [models, filters, providers],
  );
  const shown = found.slice(0, LIMIT);

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
      <div className="model-list" ref={listRef} role="list" aria-label="Models">
        {shown.map((model) => {
          const price = priceHint(model.pricing);
          const maker = model.vendor && model.vendor !== providerName(model.provider, providers) ? `${model.vendor} on ${providerName(model.provider, providers)}` : providerName(model.provider, providers);
          const isSelected = model.id === selected?.id;
          return (
            <div role="listitem" key={model.id}>
              <button
                type="button"
                className={`model-row ${model.configured ? "" : "unconfigured"}`}
                aria-pressed={isSelected}
                title={model.description ?? undefined}
                onClick={() => onSelect(model)}
              >
                <span className="model-main">
                  <span className="model-name">{model.name}</span>
                  <span className="model-sub">
                    {maker}
                    {!model.configured && <span className="model-setup"> Needs setup</span>}
                  </span>
                </span>
                <span className="model-side">
                  {model.open_weights && <span className="badge">Open weights</span>}
                  {price && <span className="model-price">{price}</span>}
                </span>
                <Icon name="check" className={`model-check ${isSelected ? "" : "invisible"}`} />
              </button>
            </div>
          );
        })}
        {!found.length && (
          <div className="model-empty">
            <p>No models match.</p>
            <button type="button" className="text-button" onClick={() => setFilters({ q: "", provider: "", task: "", where: "all", configuredOnly: false })}>
              Clear filters
            </button>
          </div>
        )}
      </div>
      <div className="model-footer">
        <span>
          {found.length > LIMIT ? `${LIMIT} of ${found.length} shown. Search to narrow.` : `${found.length} of ${models.length} models`}
        </span>
        <Link to="/settings" className="text-button">
          <Icon name="key" />
          Providers
        </Link>
      </div>
    </div>
  );
}
