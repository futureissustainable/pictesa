import { useRef, useState, type DragEvent } from "react";
import { api, errorMessage } from "../../lib/api";
import { invalidateModels, loadModels, peekModels } from "../../lib/catalog";
import { composer, selectModel, setMode } from "../../lib/composer";
import { kindIcon, kindLabel, roleLabel, taskName } from "../../lib/format";
import { notify } from "../../lib/notices";
import { navigate } from "../../lib/router";
import type { InputRole, MediaKind, WorkflowImport } from "../../lib/types";
import { Dialog } from "../../ui/dialog";
import { Icon, Spinner } from "../../ui/icon";

/** The largest workflow the server takes. */
const MAX_BYTES = 2 * 1024 * 1024;

/** "my_wan-22.json" -> "My wan 22" */
function nameFromFile(file: string): string {
  const base = file.replace(/\.[^.]+$/, "").replace(/[-_]+/g, " ").replace(/\s+/g, " ").trim();
  return base ? base.charAt(0).toUpperCase() + base.slice(1) : "";
}

/**
 * Turns a ComfyUI workflow (API format) into a Pictesa model. Accepts a dropped or picked
 * .json file or pasted JSON, then shows what Pictesa found in the graph.
 */
export default function WorkflowImportDialog({ onClose }: { onClose(): void }) {
  const [name, setName] = useState("");
  const [text, setText] = useState("");
  const [fileName, setFileName] = useState<string | null>(null);
  const [over, setOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<WorkflowImport | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const readFile = async (file: File) => {
    setError(null);
    const json = /\.json$/i.test(file.name) || file.type === "application/json";
    if (!json) return setError(`${file.name} is not a JSON file. Export the workflow with Workflow > Export (API).`);
    if (file.size > MAX_BYTES) return setError("That file is larger than 2 MB. Export the workflow with Workflow > Export (API).");
    setText(await file.text());
    setFileName(file.name);
    if (!name.trim()) setName(nameFromFile(file.name));
  };

  const onDrop = (event: DragEvent) => {
    event.preventDefault();
    setOver(false);
    const file = event.dataTransfer.files[0];
    if (file) void readFile(file);
  };

  const submit = async () => {
    if (busy) return;
    if (new Blob([text]).size > MAX_BYTES) return setError("That workflow is larger than 2 MB. Export the workflow with Workflow > Export (API).");
    let workflow: unknown;
    try {
      workflow = JSON.parse(text);
    } catch {
      setError("That is not valid JSON. Paste the file exported with Workflow > Export (API).");
      return;
    }
    if (!name.trim()) return setError("Give the model a name.");
    setBusy(true);
    setError(null);
    try {
      const result = await api.importWorkflow(name.trim(), workflow);
      invalidateModels();
      setDone(result);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  const useIt = async () => {
    if (!done) return;
    const output = (done.manifest.output ?? "image") as MediaKind;
    await loadModels(output, true);
    const model = (peekModels(output) ?? []).find((item) => item.id === done.model);
    if (!model) {
      notify("The model is imported, but the catalog has not listed it yet. Refresh models in Settings.", { tone: "error" });
      return;
    }
    const current = (peekModels(output) ?? []).find((item) => item.id === composer.get().modelByMode[output]);
    selectModel(output, model, current);
    setMode(output);
    onClose();
    navigate("/");
  };

  if (done) {
    const manifest = done.manifest;
    const output = (manifest.output ?? "image") as MediaKind;
    const inputs = manifest.inputs ?? [];
    const params = manifest.params ?? [];
    const files = manifest.requires?.models ?? [];
    return (
      <Dialog title={`${manifest.name ?? name} is ready.`} onClose={onClose} size="wide" className="workflow-dialog">
        <p className="workflow-lead">Pictesa read the workflow and made it a model. This is what it found.</p>
        <dl className="workflow-found">
          <div>
            <dt>Makes</dt>
            <dd>
              <Icon name={kindIcon[output]} />
              {kindLabel[output]}
              {manifest.tasks?.length ? <span className="hint-inline">{manifest.tasks.map(taskName).join(", ")}</span> : null}
            </dd>
          </div>
          <div>
            <dt>Prompt</dt>
            <dd>
              <Icon name={manifest.prompt?.length ? "check_circle" : "block"} />
              {manifest.prompt?.length ? "Yes" : "No prompt"}
              {manifest.negative_prompt?.length ? <span className="hint-inline">with a negative prompt</span> : null}
            </dd>
          </div>
          <div>
            <dt>Inputs</dt>
            <dd>
              {inputs.length
                ? inputs.map((input) => `${input.label || roleLabel[input.role as InputRole] || input.role}${input.min ? "" : " (optional)"}`).join(", ")
                : "None"}
            </dd>
          </div>
          <div>
            <dt>Settings</dt>
            <dd>{params.length ? params.map((param) => param.label || param.key).join(", ") : "None"}</dd>
          </div>
          {files.length > 0 && (
            <div>
              <dt>Needs on ComfyUI</dt>
              <dd className="workflow-files">
                {files.map((file) => (
                  <code key={`${file.directory}/${file.name}`}>
                    {file.directory ? `${file.directory}/` : ""}
                    {file.name}
                  </code>
                ))}
              </dd>
            </div>
          )}
        </dl>
        {done.notes.length > 0 && (
          <ul className="workflow-notes">
            {done.notes.map((note) => (
              <li key={note}>
                <Icon name="info" />
                {note}
              </li>
            ))}
          </ul>
        )}
        <div className="dialog-actions">
          <button type="button" className="text-button" onClick={onClose}>
            Close
          </button>
          <button type="button" className="primary-button" onClick={() => void useIt()}>
            Use it
            <Icon name="arrow_forward" />
          </button>
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog title="Add a ComfyUI workflow" onClose={onClose} size="wide" className="workflow-dialog">
      <p className="workflow-lead">Any ComfyUI workflow becomes a model here. In ComfyUI, use Workflow &gt; Export (API) and bring that file.</p>
      <div
        className={`workflow-drop ${over ? "drop-over" : ""}`}
        onDragOver={(event) => {
          if (!event.dataTransfer.types.includes("Files")) return;
          event.preventDefault();
          setOver(true);
        }}
        onDragLeave={() => setOver(false)}
        onDrop={onDrop}
      >
        <Icon name={fileName ? "check_circle" : "upload"} />
        <p>{fileName ? fileName : "Drop the workflow .json here"}</p>
        <button type="button" className="text-button" onClick={() => fileRef.current?.click()}>
          {fileName ? "Choose another file" : "Choose a file"}
        </button>
        <input
          ref={fileRef}
          type="file"
          accept=".json,application/json"
          className="sr-only"
          tabIndex={-1}
          aria-hidden="true"
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            if (file) void readFile(file);
          }}
        />
      </div>
      <label className="field workflow-paste">
        <span>Or paste the JSON</span>
        <textarea
          rows={4}
          spellCheck={false}
          value={text}
          placeholder='{ "1": { "class_type": "KSampler", "inputs": { ... } } }'
          onChange={(event) => {
            setText(event.target.value);
            setFileName(null);
            setError(null);
          }}
        />
      </label>
      <label className="field">
        <span>Model name</span>
        <input type="text" value={name} maxLength={80} placeholder="Wan 2.2 my settings" onChange={(event) => setName(event.target.value)} />
      </label>
      {error && (
        <p className="notice error" role="alert">
          {error}
        </p>
      )}
      <div className="dialog-actions">
        <button type="button" className="text-button" onClick={onClose}>
          Cancel
        </button>
        <button type="button" className="primary-button" disabled={busy || !text.trim() || !name.trim()} onClick={() => void submit()}>
          {busy ? <Spinner /> : <Icon name="add" />}
          Import
        </button>
      </div>
    </Dialog>
  );
}
