import { lazy, Suspense, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { updatePins, useModelPrefs, useProviders } from "../../lib/catalog";
import { cleanName, familyScore, groupFamilies, pickVariant, unitCost, type Family } from "../../lib/families";
import { taskName, unitMoney } from "../../lib/format";
import { readJson, writeJson } from "../../lib/prefs";
import { Link } from "../../lib/router";
import type { ArenaScore, ModelSpec, Pricing, ProviderKind, ProviderState, Task } from "../../lib/types";
import { Control } from "../../ui/control";
import { Icon } from "../../ui/icon";

const WorkflowImportDialog = lazy(() => import("../workflows/import-dialog"));

type Sort = "best" | "price" | "newest" | "maker" | "name";
type Where = "all" | "local" | "cloud";

interface Filters {
  q: string;
  sort: Sort;
  where: Where;
  provider: string;
  /** Also list models whose provider has no key yet. */
  everything: boolean;
}

const SORTS: { value: Sort; label: string }[] = [
  { value: "best", label: "Best rated" },
  { value: "price", label: "Lowest price" },
  { value: "newest", label: "Newest" },
  { value: "maker", label: "By maker" },
  { value: "name", label: "Name" },
];
const SORT_KEY = "pictesa.modelSort";
const LEGACY_SORTS: Record<string, Sort> = { recommended: "best", arena: "best", provider: "maker" };
function savedSort(): Sort {
  const value = readJson<string>(SORT_KEY, "best");
  const sort = LEGACY_SORTS[value] ?? value;
  return SORTS.some((item) => item.value === sort) ? (sort as Sort) : "best";
}
const startFilters = (everything: boolean): Filters => ({ q: "", sort: savedSort(), where: "all", provider: "", everything });

const LIMIT = 150;
const RECENT = 4;
/** The top three ranks get a filled cup in their medal colour. */
const MEDALS = ["gold", "silver", "bronze"];

function kindOf(model: ModelSpec, providers: ProviderState[] | null): ProviderKind {
  const kind = providers?.find((provider) => provider.id === model.provider)?.kind;
  return kind ?? (model.pricing?.unit === "local" ? "local" : "cloud");
}

export function providerName(id: string, providers: ProviderState[] | null): string {
  return providers?.find((provider) => provider.id === id)?.name ?? id;
}

/** The price column: "$0.04" per image, "$0.10/s", "Free" for local runs, nothing when it varies. */
function shortPrice(pricing: Pricing | null | undefined): string | null {
  if (!pricing || pricing.unit === "variable") return null;
  if (pricing.unit === "local") return "Free";
  const usd = unitMoney(pricing.usd);
  return { per_image: usd, per_megapixel: `${usd}/MP`, per_second: `${usd}/s`, per_video: `${usd}/video`, per_run: usd, per_step: `${usd}/step` }[pricing.unit];
}

const priceNote = (pricing: Pricing | null | undefined) => (pricing?.unit === "variable" ? pricing.note : undefined);

const arenaLabel = (score: ArenaScore) =>
  `LMArena ${taskName(score.arena).toLowerCase()}: #${score.rank} of ${score.of} as ${score.name}, ${score.votes.toLocaleString("en")} votes, ${score.published.split("-").reverse().join(".")}`;

function matches(family: Family, filters: Filters, providers: ProviderState[] | null): boolean {
  if (!filters.everything && !family.configured) return false;
  if (filters.provider && !family.variants.some((model) => model.provider === filters.provider)) return false;
  if (filters.where !== "all" && (filters.where === "local") !== family.variants.some((model) => kindOf(model, providers) === "local")) return false;
  const query = filters.q.trim().toLowerCase();
  if (!query) return true;
  const haystack = [family.name, family.maker, ...family.variants.flatMap((model) => [model.name, model.id, providerName(model.provider, providers), ...(model.tags ?? []), ...model.tasks.map(taskName)])]
    .join(" ")
    .toLowerCase();
  return query.split(/\s+/).every((word) => haystack.includes(word));
}

const released = (family: Family) => family.variants.map((model) => model.released ?? "").sort().at(-1) ?? "";
const featured = (family: Family) => (family.variants.some((model) => model.tags?.includes("featured")) ? 0 : 1);

function sorter(sort: Sort, intent: Task | null): (a: Family, b: Family) => number {
  // Differences of two missing values are NaN, which falls through to the next comparison.
  const rank = (family: Family) => familyScore(family, intent)?.rank ?? Infinity;
  const setUp = (a: Family, b: Family) => Number(b.configured) - Number(a.configured);
  const best = (a: Family, b: Family) => setUp(a, b) || rank(a) - rank(b) || featured(a) - featured(b) || a.name.localeCompare(b.name);
  switch (sort) {
    case "price":
      return (a, b) => setUp(a, b) || unitCost(a.variants[0]!.pricing) - unitCost(b.variants[0]!.pricing) || best(a, b);
    case "newest":
      return (a, b) => setUp(a, b) || released(b).localeCompare(released(a)) || best(a, b);
    case "maker":
      return (a, b) => (a.maker ?? "~").localeCompare(b.maker ?? "~") || best(a, b);
    case "name":
      return (a, b) => a.name.localeCompare(b.name);
    default:
      return best;
  }
}

interface PickerProps {
  models: ModelSpec[];
  /** What the composer is doing (editing when it has a reference image): picks the member and the arena. */
  intent?: Task | null;
}

/** The model trigger on the control line and its searchable list, one row per model. */
export function ModelPicker({ models, model, onSelect, loading, intent = null }: PickerProps & { model?: ModelSpec; onSelect(model: ModelSpec): void; loading: boolean }) {
  const [filters, setFilters] = useState<Filters | null>(null);
  const [importing, setImporting] = useState(false);
  const families = useMemo(() => groupFamilies(models), [models]);
  const current = filters ?? startFilters(!models.some((item) => item.configured));
  const name = model ? (families.find((family) => family.variants.some((item) => item.id === model.id))?.name ?? cleanName(model.name)) : null;
  return (
    <>
      <Control label="Model" size="large" className="model-trigger" value={name ?? (loading ? "Loading models" : "Choose a model")} panelClassName="model-panel" showTitle={false}>
        {(close) => (
          <ModelList
            families={families}
            selected={model}
            intent={intent}
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
export function ComparePicker({ models, exclude, disabled, onAdd, intent = null }: PickerProps & { exclude: string[]; disabled: boolean; onAdd(model: ModelSpec): void }) {
  const [filters, setFilters] = useState<Filters | null>(null);
  const current = filters ?? startFilters(false);
  // Comparing a model with itself on another provider says nothing: leave out whole families.
  const families = useMemo(() => {
    const taken = new Set(models.filter((model) => exclude.includes(model.id)).map((model) => model.family || model.id));
    return groupFamilies(models.filter((model) => !taken.has(model.family || model.id)));
  }, [models, exclude]);
  return (
    <Control label="Compare with another model" icon="add" value="Compare" caret={false} size="large" className="compare-trigger" panelClassName="model-panel" showTitle={false} disabled={disabled}>
      {(close) => (
        <ModelList
          families={families}
          intent={intent}
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
  families,
  selected,
  intent,
  filters,
  setFilters,
  onSelect,
  onImport,
}: {
  families: Family[];
  selected?: ModelSpec;
  intent: Task | null;
  filters: Filters;
  setFilters(filters: Filters): void;
  onSelect(model: ModelSpec): void;
  onImport?(): void;
}) {
  const providers = useProviders().data;
  const prefs = useModelPrefs();
  const listRef = useRef<HTMLDivElement>(null);
  const [showFilters, setShowFilters] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const set = (change: Partial<Filters>) => setFilters({ ...filters, ...change });

  const providerIds = useMemo(() => [...new Set(families.flatMap((family) => family.variants.map((model) => model.provider)))].sort(), [families]);
  const hasLocal = useMemo(() => families.some((family) => family.variants.some((model) => kindOf(model, providers) === "local")), [families, providers]);
  const view = useMemo(() => {
    const found = families.filter((family) => matches(family, filters, providers)).sort(sorter(filters.sort, intent));
    const byKey = new Map(found.map((family) => [family.key, family]));
    const familyOf = new Map(families.flatMap((family) => family.variants.map((model) => [model.id, family.key] as const)));
    const keys = (ids: string[]) => [...new Set(ids.flatMap((id) => familyOf.get(id) ?? []))].filter((key) => byKey.has(key));
    const pinned = keys(prefs.pinned);
    const recent = keys(prefs.recent)
      .filter((key) => !pinned.includes(key))
      .slice(0, RECENT);
    const lead = new Set([...pinned, ...recent]);
    const rest = found.filter((family) => !lead.has(family.key));
    const shown = rest.slice(0, LIMIT);
    const groups: { title?: string; families: Family[] }[] = [
      { title: "Pinned", families: pinned.map((key) => byKey.get(key)!) },
      { title: "Recent", families: recent.map((key) => byKey.get(key)!) },
    ];
    if (filters.sort === "maker") {
      for (const family of shown) {
        const title = family.maker ?? "Other makers";
        const last = groups.at(-1);
        if (last && last.title === title && groups.length > 2) last.families.push(family);
        else groups.push({ title, families: [family] });
      }
    } else {
      groups.push({ title: lead.size ? "All models" : undefined, families: shown });
    }
    const keyless = filters.everything ? 0 : families.filter((family) => !family.configured && matches(family, { ...filters, everything: true }, providers)).length;
    return { groups: groups.filter((group) => group.families.length), found: found.length, hidden: rest.length - shown.length, keyless };
  }, [families, filters, providers, prefs, intent]);
  const pinnedIds = new Set(prefs.pinned);
  const filtered = filters.where !== "all" || Boolean(filters.provider) || filters.everything;

  const moveFocus = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const rows = [...(listRef.current?.querySelectorAll<HTMLButtonElement>(".model-row, .model-variant") ?? [])];
    if (!rows.length) return;
    event.preventDefault();
    const index = rows.indexOf(document.activeElement as HTMLButtonElement);
    const next = index === -1 ? (event.key === "ArrowDown" ? 0 : rows.length - 1) : Math.max(0, Math.min(rows.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)));
    rows[next]?.focus();
  };

  return (
    <div className="model-picker" onKeyDown={moveFocus}>
      <div className="model-head">
        <label className="search model-search">
          <Icon name="search" />
          <span className="sr-only">Search models</span>
          <input
            type="search"
            value={filters.q}
            placeholder="Search"
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
        <label className="select-chip model-sort">
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
        <button type="button" className={`icon-button model-filter-toggle ${filtered ? "selected" : ""}`} aria-expanded={showFilters} aria-label={filtered ? "Filters, some on" : "Filters"} title="Filters" onClick={() => setShowFilters(!showFilters)}>
          <Icon name="tune" />
        </button>
      </div>
      {showFilters && (
        <div className="model-filters">
          {hasLocal && (
            <div className="segmented segmented-small" role="group" aria-label="Where models run">
              {(["all", "local", "cloud"] as const).map((where) => (
                <button key={where} type="button" aria-pressed={filters.where === where} onClick={() => set({ where })}>
                  {where === "all" ? "All" : where === "local" ? "Local" : "Cloud"}
                </button>
              ))}
            </div>
          )}
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
          <label className="switch-chip">
            <input type="checkbox" role="switch" checked={filters.everything} onChange={(event) => set({ everything: event.target.checked })} />
            Models that need a key
          </label>
        </div>
      )}
      <div className="model-list" ref={listRef}>
        {view.groups.map((group) => (
          <section key={group.title ?? "models"} className="model-section">
            {group.title && <h3 className="model-group">{group.title}</h3>}
            <div role="list" aria-label={group.title ?? "Models"}>
              {group.families.map((family) => (
                <FamilyRow
                  key={family.key}
                  family={family}
                  intent={intent}
                  selected={selected}
                  recent={prefs.recent}
                  providers={providers}
                  pinned={family.variants.some((model) => pinnedIds.has(model.id))}
                  expanded={expanded === family.key}
                  onExpand={() => setExpanded(expanded === family.key ? null : family.key)}
                  onSelect={onSelect}
                />
              ))}
            </div>
          </section>
        ))}
        {!view.found && (
          <div className="model-empty">
            <p>No models match.</p>
            <button type="button" className="text-button" onClick={() => setFilters({ ...startFilters(true), sort: filters.sort })}>
              Clear filters
            </button>
          </div>
        )}
      </div>
      <div className="model-footer">
        {view.keyless > 0 ? (
          <button type="button" className="text-button" onClick={() => set({ everything: true })}>
            Show {view.keyless} more that need a key
          </button>
        ) : view.hidden > 0 ? (
          <span>Search to see all {view.found}.</span>
        ) : null}
        <span className="model-footer-actions">
          {onImport && (
            <button type="button" className="text-button" onClick={onImport}>
              <Icon name="add" />
              ComfyUI workflow
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

function FamilyRow({
  family,
  intent,
  selected,
  recent,
  providers,
  pinned,
  expanded,
  onExpand,
  onSelect,
}: {
  family: Family;
  intent: Task | null;
  selected?: ModelSpec;
  recent: string[];
  providers: ProviderState[] | null;
  pinned: boolean;
  expanded: boolean;
  onExpand(): void;
  onSelect(model: ModelSpec): void;
}) {
  const isSelected = family.variants.some((model) => model.id === selected?.id);
  const pick = pickVariant(family, intent, isSelected ? selected : null, recent);
  const score = familyScore(family, intent);
  const price = shortPrice(pick.pricing);
  return (
    <div role="listitem" className={`model-item ${family.configured ? "" : "keyless"} ${isSelected ? "selected" : ""}`}>
      <div className="model-line">
        <button type="button" className="model-row" aria-pressed={isSelected} title={pick.description ?? undefined} onClick={() => onSelect(pick)}>
          <span className="model-name">{family.name}</span>
          {family.maker && <span className="model-maker">{family.maker}</span>}
          <span className="model-meta">
            {score && (
              <span className={`model-arena ${MEDALS[score.rank - 1] ?? ""}`} title={arenaLabel(score)}>
                <Icon name="trophy" filled={score.rank <= MEDALS.length} />
                <span className="sr-only">Arena rank</span>
                {score.rank}
              </span>
            )}
            {family.configured ? (
              <span className="model-price" title={priceNote(pick.pricing)}>
                {price}
              </span>
            ) : (
              <span className="model-price" title="Add an API key for this provider in Settings">
                <Icon name="key" />
                <span className="sr-only">Needs an API key</span>
              </span>
            )}
          </span>
        </button>
        {family.variants.length > 1 && (
          <button type="button" className="model-expand" aria-expanded={expanded} aria-label={`${family.variants.length} versions of ${family.name}`} title="Providers and versions" onClick={onExpand}>
            <Icon name="keyboard_arrow_down" />
          </button>
        )}
        <button
          type="button"
          className="model-pin"
          aria-pressed={pinned}
          aria-label={`${pinned ? "Unpin" : "Pin"} ${family.name}`}
          title={pinned ? "Unpin" : "Pin to the top"}
          onClick={() => {
            const ids = new Set(family.variants.map((model) => model.id));
            updatePins((list) => (pinned ? list.filter((id) => !ids.has(id)) : [...list, pick.id]));
          }}
        >
          <Icon name="keep" filled={pinned} />
        </button>
      </div>
      {expanded && (
        <div className="model-variants" role="group" aria-label={`Versions of ${family.name}`}>
          {family.variants.map((model) => (
            <button key={model.id} type="button" className="model-variant" aria-pressed={model.id === selected?.id} onClick={() => onSelect(model)}>
              <span className="model-variant-name">{providerName(model.provider, providers)}</span>
              <span className="model-variant-task">{model.tasks.map(taskName).join(", ")}</span>
              <span className="model-price">{model.configured ? (shortPrice(model.pricing) ?? priceNote(model.pricing)) : "Needs a key"}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
