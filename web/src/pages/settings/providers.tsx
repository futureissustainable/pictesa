import { useEffect, useId, useState, type FormEvent } from "react";
import { api, errorMessage } from "../../lib/api";
import { invalidateModels, loadProviders, replaceProvider, useProviders } from "../../lib/catalog";
import { plural, providerKindIcon } from "../../lib/format";
import { notify, notifyError } from "../../lib/notices";
import type { CredentialField, FieldValue, ProviderKind, ProviderState } from "../../lib/types";
import { Icon, Spinner } from "../../ui/icon";

const groups: { kind: ProviderKind; title: string; text: string }[] = [
  { kind: "local", title: "Local", text: "Runs on your own hardware. No key, no per-run cost." },
  { kind: "gateway", title: "Gateways", text: "One key opens many models." },
  { kind: "cloud", title: "Cloud", text: "The model makers' own APIs." },
];

export function Providers() {
  const { data, error, loading, status } = useProviders();
  const [refreshing, setRefreshing] = useState(false);

  // Links such as /settings#provider-fal land on that card.
  useEffect(() => {
    if (!data || !location.hash) return;
    const card = document.getElementById(decodeURIComponent(location.hash.slice(1)));
    card?.scrollIntoView({ block: "center" });
    card?.querySelector<HTMLInputElement>("input")?.focus({ preventScroll: true });
  }, [data]);

  const refresh = async () => {
    setRefreshing(true);
    try {
      const { models } = await api.refreshModels();
      invalidateModels();
      await loadProviders(true);
      notify(`Found ${plural(models, "model", "models")}.`);
    } catch (reason) {
      notifyError(errorMessage(reason));
    } finally {
      setRefreshing(false);
    }
  };

  if (!data) {
    if (status === 403) {
      return (
        <div className="empty">
          <Icon name="key" />
          <h2>Providers are managed by the admin.</h2>
          <p>Your account uses the models this server has set up.</p>
        </div>
      );
    }
    return error ? (
      <div className="empty" role="alert">
        <Icon name="cloud_off" />
        <h2>Providers did not load.</h2>
        <p>{error}</p>
        <button type="button" className="primary-button" onClick={() => void loadProviders(true)}>
          Try again
          <Icon name="refresh" />
        </button>
      </div>
    ) : (
      <div className="page-loading" role="status">
        <Spinner />
        {loading ? "Loading providers" : ""}
      </div>
    );
  }

  return (
    <div className="providers">
      <div className="section-head">
        <p>Connect the services you want to use. Secrets are stored by your Pictesa server and never shown again.</p>
        <button type="button" className="text-button" onClick={() => void refresh()} disabled={refreshing}>
          {refreshing ? <Spinner /> : <Icon name="refresh" />}
          Refresh models
        </button>
      </div>
      {groups.map((group) => {
        const list = data.filter((provider) => provider.kind === group.kind).sort((a, b) => Number(b.configured) - Number(a.configured) || a.name.localeCompare(b.name));
        if (!list.length) return null;
        return (
          <section key={group.kind} className="provider-group" aria-labelledby={`group-${group.kind}`}>
            <div className="group-head">
              <h2 id={`group-${group.kind}`}>{group.title}</h2>
              <p>{group.text}</p>
            </div>
            <div className="provider-grid">
              {list.map((provider) => (
                <ProviderCard key={provider.id} provider={provider} />
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

type Status = { kind: "idle" } | { kind: "busy"; text: string } | { kind: "ok"; text: string } | { kind: "error"; text: string };

function ProviderCard({ provider }: { provider: ProviderState }) {
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [confirmForget, setConfirmForget] = useState(false);
  const dirty = Object.keys(edits).length > 0;
  const busy = status.kind === "busy";
  const stored = Object.values(provider.values ?? {}).some((value) => value.source === "stored");

  useEffect(() => {
    if (!confirmForget) return;
    const timer = setTimeout(() => setConfirmForget(false), 4000);
    return () => clearTimeout(timer);
  }, [confirmForget]);

  const check = async () => {
    setStatus({ kind: "busy", text: "Checking" });
    try {
      const result = await api.checkProvider(provider.id);
      setStatus(result.ok ? { kind: "ok", text: "Connected." } : { kind: "error", text: result.error || "The check failed." });
    } catch (reason) {
      setStatus({ kind: "error", text: errorMessage(reason) });
    }
  };

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!dirty || busy) return;
    setStatus({ kind: "busy", text: "Saving" });
    try {
      replaceProvider(await api.saveProvider(provider.id, edits));
      setEdits({});
      invalidateModels();
      await check();
    } catch (reason) {
      setStatus({ kind: "error", text: errorMessage(reason) });
    }
  };

  const forget = async () => {
    if (!confirmForget) return setConfirmForget(true);
    setConfirmForget(false);
    setStatus({ kind: "busy", text: "Forgetting" });
    try {
      await api.forgetProvider(provider.id);
      await loadProviders(true);
      invalidateModels();
      setStatus({ kind: "ok", text: "Stored values removed." });
    } catch (reason) {
      setStatus({ kind: "error", text: errorMessage(reason) });
    }
  };

  return (
    <article id={`provider-${provider.id}`} className={`provider-card ${provider.configured ? "ready" : ""}`} aria-labelledby={`provider-${provider.id}-name`}>
      <header className="provider-head">
        <div className="provider-name">
          <Icon name={providerKindIcon[provider.kind]} />
          <h3 id={`provider-${provider.id}-name`}>{provider.name}</h3>
        </div>
        <span className={`status ${provider.configured ? "" : "off"}`}>{provider.configured ? "Ready" : "Not set up"}</span>
      </header>
      <p className="provider-meta">{plural(provider.model_count, "model", "models")}</p>
      <form onSubmit={(event) => void save(event)} noValidate>
        {provider.fields.map((field) => (
          <CredentialInput
            key={field.key}
            field={field}
            state={provider.values?.[field.key]}
            value={edits[field.key]}
            onChange={(value) =>
              setEdits((current) => {
                const next = { ...current };
                if (value === undefined) delete next[field.key];
                else next[field.key] = value;
                return next;
              })
            }
          />
        ))}
        <div className="provider-actions">
          {dirty && (
            <button type="submit" className="primary-button" disabled={busy}>
              Save
            </button>
          )}
          <button type="button" className="text-button" disabled={busy} onClick={() => void check()}>
            <Icon name="check_circle" />
            Check
          </button>
          {stored && (
            <button type="button" className="text-button destructive" disabled={busy} onClick={() => void forget()}>
              <Icon name="delete" />
              {confirmForget ? "Confirm forget" : "Forget"}
            </button>
          )}
          {provider.signup_url && !provider.configured && (
            <a className="text-button" href={provider.signup_url} target="_blank" rel="noreferrer">
              <Icon name="open_in_new" />
              Get a key
            </a>
          )}
          {provider.docs_url && (
            <a className="text-button" href={provider.docs_url} target="_blank" rel="noreferrer">
              <Icon name="open_in_new" />
              Docs
            </a>
          )}
        </div>
        {status.kind !== "idle" && (
          <p className={`provider-status ${status.kind}`} role={status.kind === "error" ? "alert" : "status"}>
            {status.kind === "busy" ? <Spinner /> : <Icon name={status.kind === "ok" ? "check_circle" : "error"} />}
            {status.text}
          </p>
        )}
      </form>
    </article>
  );
}

function CredentialInput({ field, state, value, onChange }: { field: CredentialField; state?: FieldValue; value?: string; onChange(value: string | undefined): void }) {
  const id = useId();
  const [reveal, setReveal] = useState(false);
  const env = field.env?.filter(Boolean) ?? [];
  // Non-secret values (a local server URL) are shown as they are; secrets never come back.
  const storedValue = field.secret ? "" : (state?.value ?? "");
  const placeholder = field.secret
    ? state?.set
      ? state.source === "env"
        ? `Set by ${env[0] ?? "the environment"}`
        : "Saved. Type to replace."
      : (field.placeholder ?? "Paste your key")
    : (field.placeholder ?? field.default ?? "");
  const hint =
    state?.source === "env"
      ? `Read from ${env.join(" or ") || "the environment"}.`
      : state?.source === "stored"
        ? field.secret
          ? "Stored on the server."
          : null
        : state?.source === "default"
          ? "Default."
          : env.length
            ? `Or set ${env.join(" or ")}.`
            : null;
  return (
    <div className="field">
      <label className="field-top" htmlFor={id}>
        <span>{field.label}</span>
        {field.optional && <small>Optional</small>}
      </label>
      <div className="input-row">
        <input
          id={id}
          type={field.secret && !reveal ? "password" : "text"}
          autoComplete="off"
          spellCheck={false}
          autoCapitalize="off"
          value={value ?? storedValue}
          placeholder={placeholder}
          aria-describedby={hint ? `${id}-hint` : undefined}
          onChange={(event) => onChange(event.target.value === storedValue ? undefined : event.target.value)}
        />
        {field.secret && (
          <button type="button" className="icon-button" aria-label={reveal ? "Hide what you typed" : "Show what you typed"} aria-pressed={reveal} onClick={() => setReveal(!reveal)}>
            <Icon name={reveal ? "visibility_off" : "visibility"} />
          </button>
        )}
      </div>
      {hint && <small id={`${id}-hint`}>{hint}</small>}
    </div>
  );
}
