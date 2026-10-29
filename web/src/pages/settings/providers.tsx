import { lazy, Suspense, useEffect, useId, useState, type FormEvent } from "react";
import { api, errorMessage, plainError } from "../../lib/api";
import { invalidateModels, loadProviders, replaceProvider, useProviders } from "../../lib/catalog";
import { plural, providerKindIcon } from "../../lib/format";
import { notify, notifyError } from "../../lib/notices";
import type { CredentialField, FieldValue, ProviderKind, ProviderState } from "../../lib/types";
import { Icon, Spinner } from "../../ui/icon";

const WorkflowImportDialog = lazy(() => import("../workflows/import-dialog"));

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

/** Address fields take a full http or https URL. */
const isUrlField = (field: CredentialField) => /url$/i.test(field.key);
function urlProblem(value: string): string | null {
  if (!value.trim()) return null;
  try {
    const url = new URL(value.trim());
    if (url.protocol === "http:" || url.protocol === "https:") return null;
  } catch {
    // Falls through to the message.
  }
  return "Enter an http or https address, such as http://127.0.0.1:8188.";
}

function ProviderCard({ provider }: { provider: ProviderState }) {
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [confirmForget, setConfirmForget] = useState(false);
  const [importing, setImporting] = useState(false);
  /** The last check's answer: a local server can be set up and still not running. */
  const [reachable, setReachable] = useState<boolean | null>(null);
  const dirty = Object.keys(edits).length > 0;
  const invalid = provider.fields.filter((field) => isUrlField(field) && edits[field.key] !== undefined && urlProblem(edits[field.key]!));
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
      setReachable(result.ok);
      setStatus(result.ok ? { kind: "ok", text: "Connected." } : { kind: "error", text: result.error ? plainError(result.error) : "The check failed." });
      // A local server's model list is refreshed by a successful check.
      if (result.ok && provider.kind === "local") invalidateModels();
    } catch (reason) {
      setStatus({ kind: "error", text: errorMessage(reason) });
    }
  };

  // A set-up local server is checked quietly once, so the badge says whether it answers.
  useEffect(() => {
    if (provider.kind !== "local" || !provider.configured || !provider.fields.some(isUrlField)) return;
    let live = true;
    api
      .checkProvider(provider.id)
      .then((result) => live && setReachable(result.ok))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [provider.id, provider.kind, provider.configured]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!dirty || busy || invalid.length) return;
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
        <span className={`status ${provider.configured && reachable !== false ? "" : "off"}`}>{!provider.configured ? "Not set up" : reachable === false ? "Not reachable" : "Ready"}</span>
      </header>
      <p className="provider-meta">{plural(provider.model_count, "model", "models")}</p>
      <form onSubmit={(event) => void save(event)} noValidate>
        {provider.fields.map((field) => (
          <CredentialInput
            key={field.key}
            field={field}
            locked={provider.id === "ai-toolkit" && field.key === "path" ? "Pictesa runs this program, so set it with pictesa keys set ai-toolkit path or PICTESA_AI_TOOLKIT_PATH." : undefined}
            state={provider.values?.[field.key]}
            value={edits[field.key]}
            error={isUrlField(field) && edits[field.key] !== undefined ? urlProblem(edits[field.key]!) : null}
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
            <button type="submit" className="primary-button" disabled={busy || invalid.length > 0}>
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
          {provider.id === "comfyui" && (
            <button type="button" className="text-button" onClick={() => setImporting(true)}>
              <Icon name="add" />
              Import workflow
            </button>
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
      {importing && (
        <Suspense fallback={null}>
          <WorkflowImportDialog onClose={() => setImporting(false)} />
        </Suspense>
      )}
    </article>
  );
}

function CredentialInput({
  field,
  state,
  value,
  error,
  locked,
  onChange,
}: {
  field: CredentialField;
  /** Set outside the studio: shown read-only with this explanation. */
  locked?: string;
  state?: FieldValue;
  value?: string;
  error?: string | null;
  onChange(value: string | undefined): void;
}) {
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
  const hint = locked ??
    (state?.source === "env"
      ? `Read from ${env.join(" or ") || "the environment"}.`
      : state?.source === "stored"
        ? field.secret
          ? "Stored on the server."
          : null
        : state?.source === "default"
          ? "Default."
          : env.length
            ? `Or set ${env.join(" or ")}.`
            : null);
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
          readOnly={Boolean(locked)}
          aria-invalid={error ? true : undefined}
          aria-describedby={[error && `${id}-error`, hint && `${id}-hint`].filter(Boolean).join(" ") || undefined}
          onChange={(event) => onChange(event.target.value === storedValue ? undefined : event.target.value)}
        />
        {field.secret && (
          <button type="button" className="icon-button" aria-label={reveal ? "Hide what you typed" : "Show what you typed"} aria-pressed={reveal} onClick={() => setReveal(!reveal)}>
            <Icon name={reveal ? "visibility_off" : "visibility"} />
          </button>
        )}
      </div>
      {error && (
        <small id={`${id}-error`} className="field-error">
          {error}
        </small>
      )}
      {hint && <small id={`${id}-hint`}>{hint}</small>}
    </div>
  );
}
