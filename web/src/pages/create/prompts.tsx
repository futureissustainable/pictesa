import { useEffect, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { composer, setPrompt } from "../../lib/composer";
import { formatDateTime } from "../../lib/format";
import { notify } from "../../lib/notices";
import type { GenerationRequest, PromptContent, PromptHistory, SavedPrompt } from "../../lib/types";
import { Choice, Control } from "../../ui/control";
import { Dialog } from "../../ui/dialog";
import { Icon, Spinner } from "../../ui/icon";

export function usePromptSuggestions(prompt: string) {
  const [items, setItems] = useState<PromptHistory[]>([]);
  useEffect(() => {
    let current = true;
    setItems([]);
    if (prompt.trim().length < 3) return;
    const timer = setTimeout(() => {
      void api
        .promptHistory(prompt.trim(), 4)
        .then(({ history }) => {
          if (current) setItems(history.filter((item) => item.prompt !== prompt).slice(0, 3));
        })
        .catch(() => undefined);
    }, 250);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [prompt]);
  return items;
}

export function PromptHistoryControl() {
  return (
    <Control label="Prompt history" icon="schedule" caret={false} size="wide">
      {(close) => <HistoryList close={close} />}
    </Control>
  );
}

function HistoryList({ close }: { close: () => void }) {
  const [q, setQ] = useState("");
  const [items, setItems] = useState<PromptHistory[] | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let current = true;
    void api
      .promptHistory(q)
      .then(({ history }) => {
        if (current) {
          setItems(history);
          setError("");
        }
      })
      .catch((e) => {
        if (current) setError(errorMessage(e));
      });
    return () => {
      current = false;
    };
  }, [q]);
  return (
    <div className="prompt-list">
      <label className="field">
        <span className="sr-only">Search prompt history</span>
        <input data-autofocus type="search" placeholder="Search history" value={q} onChange={(e) => setQ(e.target.value)} />
      </label>
      {error && (
        <p role="alert" className="prompt-error">
          {error}
        </p>
      )}
      {!items && !error && <Spinner />}
      {items?.map((item) => (
        <Choice
          key={item.prompt}
          onPick={() => {
            setPrompt(item.prompt);
            close();
          }}
        >
          <span className="prompt-excerpt">{item.prompt}</span>
          <small>
            {item.model_name} / {formatDateTime(item.last_used_at)}
          </small>
        </Choice>
      ))}
      {items?.length === 0 && <p className="control-help">No past prompts found.</p>}
    </div>
  );
}

export function PromptsControl() {
  return (
    <Control label="Prompts" value="Prompts" icon="draft" caret={false} size="wide">
      {(close) => <SavedList close={close} />}
    </Control>
  );
}

async function insertPrompt(content: PromptContent) {
  const model = content.model ? await api.model(content.model) : null;
  composer.set((state) => {
    const mode = model?.output ?? state.mode;
    const modelId = model?.id ?? state.modelByMode[mode];
    return {
      ...state,
      mode,
      prompt: content.prompt,
      negativePrompt: content.negative_prompt ?? "",
      modelByMode: model ? { ...state.modelByMode, [mode]: model.id } : state.modelByMode,
      paramsByModel: content.params && modelId ? { ...state.paramsByModel, [modelId]: content.params } : state.paramsByModel,
      presetByMode: content.params ? { ...state.presetByMode, [mode]: null } : state.presetByMode,
      revision: state.revision + 1,
    };
  });
}

function SavedList({ close }: { close: () => void }) {
  const [q, setQ] = useState("");
  const [items, setItems] = useState<SavedPrompt[] | null>(null);
  const [selected, setSelected] = useState<SavedPrompt | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [rename, setRename] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [remove, setRemove] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let current = true;
    void api
      .savedPrompts(q)
      .then(({ prompts }) => {
        if (current) {
          setItems(prompts);
          setError("");
        }
      })
      .catch((e) => {
        if (current) setError(errorMessage(e));
      });
    return () => {
      current = false;
    };
  }, [q]);
  async function run(action: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }
  async function use(saved: SavedPrompt) {
    await run(async () => {
      await insertPrompt(await api.renderPrompt(saved.id, values));
      close();
    });
  }
  return (
    <div className="prompt-list">
      <label className="field">
        <span className="sr-only">Search saved prompts</span>
        <input
          data-autofocus
          type="search"
          placeholder="Search prompts"
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            setSelected(null);
          }}
        />
      </label>
      {error && (
        <p role="alert" className="prompt-error">
          {error}
        </p>
      )}
      {!items && !error && <Spinner />}
      {items?.length === 0 && <p className="control-help">Save a prompt from More settings. Use {"{placeholders}"} to make a template.</p>}
      {items?.map((item) => (
        <div className="saved-prompt" key={item.id}>
          {rename === item.id ? (
            <div className="prompt-row">
              <label className="field">
                <span className="sr-only">Prompt name</span>
                <input autoFocus maxLength={100} value={name} onChange={(e) => setName(e.target.value)} />
              </label>
              <button
                type="button"
                className="text-button"
                disabled={busy || !name.trim()}
                onClick={() =>
                  void run(async () => {
                    const updated = await api.renamePrompt(item.id, name);
                    setItems((list) => list?.map((p) => (p.id === updated.id ? updated : p)) ?? null);
                    setRename(null);
                  })
                }
              >
                Save name
              </button>
              <button type="button" className="text-button" onClick={() => setRename(null)}>
                Cancel
              </button>
            </div>
          ) : (
            <div className="prompt-row">
              <button
                type="button"
                className="choice"
                disabled={busy}
                aria-label={`Insert ${item.name}`}
                onClick={() => {
                  if (item.placeholders.length) {
                    setSelected(item);
                    setValues({});
                  } else void use(item);
                }}
              >
                <span className="choice-body">
                  <strong>{item.name}</strong>
                  <span className="prompt-excerpt">{item.prompt}</span>
                </span>
              </button>
              <button
                type="button"
                className="icon-button"
                aria-label={`Rename ${item.name}`}
                onClick={() => {
                  setRename(item.id);
                  setName(item.name);
                }}
              >
                <Icon name="edit" />
              </button>
              <button type="button" className="icon-button" aria-label={`Delete ${item.name}`} onClick={() => setRemove(item.id)}>
                <Icon name="delete" />
              </button>
            </div>
          )}
          {remove === item.id && (
            <div className="prompt-row">
              <span>Delete this prompt?</span>
              <button
                type="button"
                className="text-button destructive"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    await api.deletePrompt(item.id);
                    setItems((list) => list?.filter((p) => p.id !== item.id) ?? null);
                    setRemove(null);
                  })
                }
              >
                Delete prompt
              </button>
              <button type="button" className="text-button" onClick={() => setRemove(null)}>
                Cancel
              </button>
            </div>
          )}
          {selected?.id === item.id && (
            <div className="more-fields">
              {item.placeholders.map((key, index) => (
                <label key={key} className="field">
                  <span>{key}</span>
                  <input
                    autoFocus={index === 0}
                    maxLength={20000}
                    value={values[key] ?? ""}
                    onChange={(e) => setValues((v) => ({ ...v, [key]: e.target.value }))}
                  />
                </label>
              ))}
              <div className="prompt-row">
                <button
                  type="button"
                  className="primary-button"
                  disabled={busy || item.placeholders.some((key) => !values[key]?.trim())}
                  onClick={() => void use(item)}
                >
                  Insert prompt
                </button>
                <button type="button" className="text-button" onClick={() => setSelected(null)}>
                  Cancel
                </button>
              </div>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

export function SavePromptDialog({ request, onClose }: { request: GenerationRequest; onClose: () => void }) {
  const [name, setName] = useState("");
  const [settings, setSettings] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <Dialog title="Save prompt" onClose={onClose}>
      <div className="more-fields">
        <label className="field">
          <span>Name</span>
          <input autoFocus value={name} maxLength={100} onChange={(e) => setName(e.target.value)} placeholder="Cabin at dusk" />
        </label>
        <p className="control-help">{"Use {subject} or {place} in your prompt to make a template."}</p>
        <label className="prompt-row">
          <input type="checkbox" checked={settings} onChange={(e) => setSettings(e.target.checked)} />
          Include model and settings
        </label>
        {error && (
          <p role="alert" className="prompt-error">
            {error}
          </p>
        )}
        <button
          type="button"
          className="primary-button"
          disabled={busy || !name.trim() || !request.prompt.trim()}
          onClick={async () => {
            setBusy(true);
            try {
              await api.savePrompt({
                name,
                prompt: request.prompt,
                negative_prompt: request.negative_prompt,
                ...(settings ? { model: request.model, params: request.params } : {}),
              });
              notify("Prompt saved.");
              onClose();
            } catch (e) {
              setError(errorMessage(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy ? <Spinner /> : "Save prompt"}
        </button>
      </div>
    </Dialog>
  );
}
