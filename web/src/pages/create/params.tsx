import { useEffect, useId, useState, type ReactNode } from "react";
import { isRatio, optionLabel, paramIcon, ratioOf, sameValue } from "../../lib/format";
import type { ParamSpec, ParamValue } from "../../lib/types";
import { Choice, Control } from "../../ui/control";
import { Icon, type IconName } from "../../ui/icon";

// Controls generated from a model's ParamSpec. Main params sit on the control line; advanced
// ones are fields inside the More panel. Nothing here knows about a particular model.

type Change = (value: ParamValue | undefined) => void;

const SEED_MAX = 4294967295;
export const randomSeed = () => Math.floor(Math.random() * SEED_MAX);

function isRatioParam(spec: ParamSpec): boolean {
  if (spec.type !== "enum") return false;
  const ratios = spec.options.filter((option) => isRatio(option.label)).length;
  return ratios >= Math.max(2, spec.options.length - 1);
}

/** A small rectangle in the option's aspect ratio. */
export function RatioShape({ text }: { text: string }) {
  const ratio = ratioOf(text);
  if (!ratio) return <Icon name="aspect_ratio" />;
  const width = ratio >= 1 ? 14 : Math.max(5, Math.round(14 * ratio));
  const height = ratio >= 1 ? Math.max(5, Math.round(14 / ratio)) : 14;
  return (
    <span className="ratio-box" aria-hidden="true">
      <span style={{ width, height }} />
    </span>
  );
}

function boolIcon(key: string, on: boolean): IconName | null {
  if (key === "generate_audio") return on ? "volume_up" : "volume_off";
  if (key === "loop") return "repeat";
  if (key === "enhance_prompt") return "auto_awesome";
  return null;
}

/** Inline control for one main parameter. */
export function ParamControl({ spec, value, onChange, invalid }: { spec: ParamSpec; value: ParamValue; onChange: Change; invalid?: boolean }) {
  const icon = paramIcon(spec.key);
  const help = spec.help ? <p className="control-help">{spec.help}</p> : null;

  if (spec.type === "bool") {
    const on = value === true;
    const symbol = boolIcon(spec.key, on);
    return (
      <button
        type="button"
        className={`control-trigger toggle ${on ? "on" : ""}`}
        aria-pressed={on}
        aria-label={spec.label}
        aria-invalid={invalid || undefined}
        title={spec.help ? `${spec.label}: ${spec.help}` : spec.label}
        onClick={() => onChange(!on)}
      >
        <Icon name={symbol ?? (on ? "toggle_on" : "toggle_off")} filled={on && Boolean(symbol)} />
        {!symbol && <span className="control-value">{spec.label}</span>}
      </button>
    );
  }

  if (spec.type === "enum") {
    const label = optionLabel(spec, value);
    if (isRatioParam(spec)) {
      return (
        <Control label={spec.label} icon="aspect_ratio" value={label} invalid={invalid}>
          <div className="ratio-options">
            {spec.options.map((option) => (
              <Choice key={String(option.value)} selected={sameValue(option.value, value)} onPick={() => onChange(option.value)} className="ratio-choice">
                <RatioShape text={option.label} />
                {option.label}
              </Choice>
            ))}
          </div>
          {help}
        </Control>
      );
    }
    return (
      <Control label={spec.label} icon={icon} value={icon ? label : `${spec.label} ${label}`} invalid={invalid}>
        {spec.options.map((option) => (
          <Choice key={String(option.value)} selected={sameValue(option.value, value)} onPick={() => onChange(option.value)}>
            {option.label}
          </Choice>
        ))}
        {help}
      </Control>
    );
  }

  const shown = value === null || value === undefined || value === "" ? (spec.type === "seed" ? "Random" : "Default") : String(value);
  return (
    <Control
      label={spec.label}
      icon={icon}
      value={icon ? (spec.type === "text" ? truncate(shown, 18) : shown) : `${spec.label} ${truncate(shown, 14)}`}
      invalid={invalid}
      showTitle={false}
    >
      <div className="control-field">
        <ParamField spec={spec} value={value} onChange={onChange} autoFocus />
      </div>
    </Control>
  );
}

const truncate = (text: string, length: number) => (text.length > length ? `${text.slice(0, length - 1)}…` : text);

/** A labeled field for any parameter type (used in the More panel and inside popovers). */
export function ParamField({ spec, value, onChange, autoFocus = false }: { spec: ParamSpec; value: ParamValue; onChange: Change; autoFocus?: boolean }) {
  const id = useId();
  const help = spec.help ? <small id={`${id}-help`}>{spec.help}</small> : null;
  const describedBy = spec.help ? `${id}-help` : undefined;

  switch (spec.type) {
    case "enum":
      return (
        <label className="field" htmlFor={id}>
          <span>{spec.label}</span>
          <select
            id={id}
            aria-describedby={describedBy}
            value={String(spec.options.findIndex((option) => sameValue(option.value, value)))}
            onChange={(event) => onChange(spec.options[Number(event.target.value)]?.value)}
          >
            {spec.options.findIndex((option) => sameValue(option.value, value)) === -1 && <option value="-1">Default</option>}
            {spec.options.map((option, index) => (
              <option key={String(option.value)} value={String(index)}>
                {option.label}
              </option>
            ))}
          </select>
          {help}
        </label>
      );
    case "bool":
      return (
        <label className="field field-switch">
          <span>
            {spec.label}
            {help}
          </span>
          <input type="checkbox" role="switch" checked={value === true} aria-describedby={describedBy} onChange={(event) => onChange(event.target.checked)} />
        </label>
      );
    case "int":
    case "float":
      return <NumberField spec={spec} value={value} onChange={onChange} autoFocus={autoFocus} id={id} help={help} describedBy={describedBy} />;
    case "text":
      return (
        <label className="field" htmlFor={id}>
          <span>{spec.label}</span>
          {spec.multiline ? (
            <textarea
              id={id}
              rows={3}
              maxLength={spec.max_len ?? undefined}
              value={typeof value === "string" ? value : ""}
              aria-describedby={describedBy}
              data-autofocus={autoFocus ? "" : undefined}
              onChange={(event) => onChange(event.target.value === "" ? undefined : event.target.value)}
            />
          ) : (
            <input
              id={id}
              type="text"
              maxLength={spec.max_len ?? undefined}
              value={typeof value === "string" ? value : ""}
              aria-describedby={describedBy}
              data-autofocus={autoFocus ? "" : undefined}
              onChange={(event) => onChange(event.target.value === "" ? undefined : event.target.value)}
            />
          )}
          {help}
        </label>
      );
    case "seed":
      return <SeedField spec={spec} value={value} onChange={onChange} autoFocus={autoFocus} id={id} />;
    default:
      return null;
  }
}

function NumberField({
  spec,
  value,
  onChange,
  autoFocus,
  id,
  help,
  describedBy,
}: {
  spec: Extract<ParamSpec, { type: "int" | "float" }>;
  value: ParamValue;
  onChange: Change;
  autoFocus: boolean;
  id: string;
  help: ReactNode;
  describedBy?: string;
}) {
  const step = spec.step ?? (spec.type === "int" ? 1 : 0.01);
  const numeric = typeof value === "number" ? value : null;
  const [text, setText] = useState(numeric === null ? "" : String(numeric));
  // Follow outside changes without fighting partial input such as "1." while typing.
  useEffect(() => setText((current) => (current.trim() !== "" && Number(current) === numeric ? current : numeric === null ? "" : String(numeric))), [numeric]);
  // Sliders only make sense for a sensible range; huge open ranges get a number box.
  const sliding = Number.isFinite(spec.min) && Number.isFinite(spec.max) && spec.max > spec.min && (spec.max - spec.min) / step <= 20000 && spec.max <= 100000;
  const commit = (raw: string) => {
    setText(raw);
    if (raw.trim() === "") return onChange(undefined);
    const parsed = spec.type === "int" ? Math.round(Number(raw)) : Number(raw);
    if (Number.isFinite(parsed) && parsed >= spec.min && parsed <= spec.max) onChange(parsed);
  };
  return (
    <div className="field">
      <label className="field-top" htmlFor={id}>
        <span>{spec.label}</span>
        <output>{numeric ?? "Default"}</output>
      </label>
      {sliding && (
        <input
          type="range"
          min={spec.min}
          max={spec.max}
          step={step}
          value={numeric ?? spec.min}
          aria-label={spec.label}
          aria-describedby={describedBy}
          onChange={(event) => commit(event.target.value)}
        />
      )}
      <input
        id={id}
        type="number"
        inputMode={spec.type === "int" ? "numeric" : "decimal"}
        min={spec.min}
        max={spec.max}
        step={step}
        value={text}
        placeholder="Default"
        aria-describedby={describedBy}
        data-autofocus={autoFocus ? "" : undefined}
        onChange={(event) => commit(event.target.value)}
      />
      {help}
    </div>
  );
}

function SeedField({ spec, value, onChange, autoFocus, id }: { spec: ParamSpec; value: ParamValue; onChange: Change; autoFocus: boolean; id: string }) {
  const seed = typeof value === "number" ? value : null;
  return (
    <div className="field">
      <label htmlFor={id}>{spec.label}</label>
      <div className="seed-row">
        <input
          id={id}
          type="number"
          inputMode="numeric"
          min={0}
          max={SEED_MAX}
          placeholder="Random"
          value={seed ?? ""}
          data-autofocus={autoFocus ? "" : undefined}
          onChange={(event) => {
            const raw = event.target.value.trim();
            const parsed = Math.round(Number(raw));
            onChange(raw === "" ? undefined : Number.isFinite(parsed) && parsed >= 0 ? parsed : undefined);
          }}
        />
        <button type="button" className="icon-button" aria-label="Pick a random seed" title="Pick a random seed" onClick={() => onChange(randomSeed())}>
          <Icon name="casino" />
        </button>
      </div>
      <small>{spec.help ?? "Empty gives a fresh result each time. A fixed seed repeats a result."}</small>
    </div>
  );
}
