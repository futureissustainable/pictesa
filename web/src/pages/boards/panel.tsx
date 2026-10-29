import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, errorMessage } from "../../lib/api";
import { useAssetList } from "../../lib/assets";
import { connect, deleteItems, setEdgeRole, updateNode, useTextDraft } from "../../lib/boards";
import { useFullModel, useModels } from "../../lib/catalog";
import { isValidParam } from "../../lib/composer";
import { formatDate, kindPlural, roleLabel } from "../../lib/format";
import { notifyError } from "../../lib/notices";
import type { Asset, Board, BoardEdge, BoardMember, BoardNode, BoardRole, InputRole, MediaKind, ModelSpec, ParamValue, ParamValues } from "../../lib/types";
import { Thumb } from "../../ui/asset-card";
import { Dialog } from "../../ui/dialog";
import { AssetGrid } from "../../ui/grid";
import { Icon, Spinner } from "../../ui/icon";
import { ModelPicker } from "../create/model-picker";
import { ParamField } from "../create/params";
import { runActive, runLabel } from "./nodes";

/** Library files to add: a click puts one in the middle of the view, a drag puts it where it lands. */
export function LibraryPanel({ onAdd, titleOf }: { onAdd(asset: Asset): void; titleOf(asset: Asset): string }) {
  const [kind, setKind] = useState<MediaKind>("image");
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const list = useAssetList({ kind, q: query || undefined });
  useEffect(() => {
    const timer = setTimeout(() => setQuery(search.trim()), 250);
    return () => clearTimeout(timer);
  }, [search]);
  return (
    <section className="board-library" aria-label="Library">
      <div className="board-panel-head">
        <h2>Library</h2>
        <div className="segmented segmented-small" role="group" aria-label="File type">
          {(["image", "video"] as const).map((value) => (
            <button key={value} type="button" aria-pressed={kind === value} onClick={() => setKind(value)}>
              {value === "image" ? "Images" : "Videos"}
            </button>
          ))}
        </div>
      </div>
      <label className="search">
        <Icon name="search" />
        <span className="sr-only">Search {kindPlural[kind]}</span>
        <input type="search" value={search} placeholder="Search by prompt" onChange={(event) => setSearch(event.target.value)} />
      </label>
      <p className="hint">Click a file to add it, or drag it onto the board.</p>
      <div className="board-library-grid" ref={scrollRef}>
        {list.items.length ? (
          <AssetGrid
            items={list.items}
            size={104}
            scroller={scrollRef}
            label={`Your ${kindPlural[kind]}`}
            titleOf={titleOf}
            verb="Add to the board:"
            actions={false}
            hasMore={list.hasMore}
            loading={list.loading}
            onEndReached={list.loadMore}
            onOpen={(asset) => onAdd(asset)}
          />
        ) : (
          <p className="hint" role="status">
            {list.loading ? <Spinner /> : null}
            {list.loading ? "Loading" : list.error ?? (query ? "Nothing matches that search." : `No ${kindPlural[kind]} yet.`)}
          </p>
        )}
      </div>
    </section>
  );
}

function PanelHead({ title, onClose }: { title: string; onClose(): void }) {
  return (
    <div className="board-panel-head">
      <h2>{title}</h2>
      <button type="button" className="icon-button" aria-label="Close" onClick={onClose}>
        <Icon name="close" />
      </button>
    </div>
  );
}

export interface InspectorProps {
  board: Board;
  node: BoardNode;
  nodes: BoardNode[];
  edges: BoardEdge[];
  editable: boolean;
  titleOf(node: BoardNode): string;
  onClose(): void;
  onRun(id: string, downstream: boolean): void;
  onCancel(id: string): void;
  onUseInCreate(assetIds: string[]): void;
  onNewStep(from: string[]): void;
  onOpenAsset(asset: Asset): void;
}

export function Inspector(props: InspectorProps) {
  const { node, editable } = props;
  if (node.kind === "step") return <StepInspector {...props} />;
  if (node.kind === "note") return <NoteInspector {...props} />;
  const asset = node.asset_id ? props.board.assets[node.asset_id] : undefined;
  return (
    <section className="board-inspector" aria-label="Selected file">
      <PanelHead title={asset ? props.titleOf(node) : "Missing file"} onClose={props.onClose} />
      {asset ? (
        <>
          <button type="button" className="board-preview" onClick={() => props.onOpenAsset(asset)} aria-label="Open the file">
            <Thumb asset={asset} eager />
          </button>
          {asset.prompt && <p className="detail-prompt">{asset.prompt}</p>}
          <div className="detail-actions">
            {asset.kind === "image" && (
              <button type="button" className="text-button" onClick={() => props.onUseInCreate([asset.id])}>
                <Icon name="arrow_upward" />
                Use as reference in Create
              </button>
            )}
            {editable && (
              <button type="button" className="text-button" onClick={() => props.onNewStep([node.id])}>
                <Icon name="auto_awesome" />
                New step from this
              </button>
            )}
          </div>
        </>
      ) : (
        <p className="hint">The file was deleted. Remove this item, or keep it as a marker.</p>
      )}
      {editable && <RemoveButton ids={[node.id]} label="Remove from the board" onDone={props.onClose} />}
    </section>
  );
}

function RemoveButton({ ids, label, onDone }: { ids: string[]; label: string; onDone(): void }) {
  return (
    <button
      type="button"
      className="text-button destructive board-remove"
      onClick={() => {
        deleteItems(ids);
        onDone();
      }}
    >
      <Icon name="delete" />
      {label}
    </button>
  );
}

function NoteInspector({ node, editable, onClose }: InspectorProps) {
  const draft = useTextDraft(node.text ?? "", node.rev, (text, seen) => updateNode(node.id, { text }, seen));
  return (
    <section className="board-inspector" aria-label="Selected note">
      <PanelHead title="Note" onClose={onClose} />
      <label className="field">
        <span>Text</span>
        <textarea rows={8} maxLength={10000} value={draft.text} readOnly={!editable} onChange={(event) => draft.change(event.target.value)} onBlur={draft.flush} />
      </label>
      {editable && <RemoveButton ids={[node.id]} label="Delete note" onDone={onClose} />}
    </section>
  );
}

const outputs = ["image", "video", "audio"] as const;

/** Keeps the settings the new model has, with values it accepts. */
function carryParams(params: ParamValues | undefined, model: ModelSpec): ParamValues {
  const out: ParamValues = {};
  for (const spec of model.params) {
    const value = params?.[spec.key];
    if (value !== undefined && isValidParam(spec, value)) out[spec.key] = value;
  }
  return out;
}

function StepInspector({ board, node, nodes, edges, editable, titleOf, onClose, onRun, onCancel }: InspectorProps) {
  const lists = { image: useModels("image"), video: useModels("video"), audio: useModels("audio") };
  const current = outputs.flatMap((output) => lists[output].data ?? []).find((model) => model.id === node.model);
  // Steps generate media; LoRA trainers live on the Characters page.
  const [output, setOutput] = useState<keyof typeof lists>(current?.output === "lora" ? "image" : (current?.output ?? "image"));
  useEffect(() => {
    if (current && current.output !== "lora") setOutput(current.output);
  }, [current]);
  const models = lists[output];
  const full = useFullModel(current);
  const spec = full.spec;
  const prompt = useTextDraft(node.prompt ?? "", node.rev, (text, seen) => updateNode(node.id, { prompt: text }, seen));
  const negative = useTextDraft(node.negative_prompt ?? "", node.rev, (text, seen) => updateNode(node.id, { negative_prompt: text }, seen));
  const [more, setMore] = useState(false);
  const run = board.runs[node.id];
  const active = runActive(run);
  const byId = new Map(nodes.map((item) => [item.id, item]));
  const incoming = edges.filter((edge) => edge.to === node.id && byId.get(edge.from)?.kind !== "note");
  const sources = nodes.filter((item) => item.id !== node.id && item.kind !== "note" && !incoming.some((edge) => edge.from === item.id));
  const feeds = edges.some((edge) => edge.from === node.id && byId.get(edge.to)?.kind === "step");
  const params = spec?.params.filter((param) => more || !param.advanced) ?? [];
  const setParam = (key: string, value: ParamValue | undefined) => {
    const next: ParamValues = { ...node.params };
    if (value === undefined) delete next[key];
    else next[key] = value;
    updateNode(node.id, { params: next });
  };
  const sourceLabel = (edge: BoardEdge) => {
    const source = byId.get(edge.from);
    return source ? (source.kind === "step" ? `Results of ${titleOf(source).replace(/^Step: /, "")}` : titleOf(source)) : "Missing item";
  };
  const visibleRuns = outputs.filter((value) => value !== "audio" || (lists.audio.data?.length ?? 0) > 0);

  return (
    <section className="board-inspector" aria-label="Selected step">
      <PanelHead title="Step" onClose={onClose} />
      {editable && visibleRuns.length > 1 && (
        <div className="segmented segmented-small" role="group" aria-label="Output">
          {visibleRuns.map((value) => (
            <button key={value} type="button" aria-pressed={output === value} onClick={() => setOutput(value)}>
              {value === "image" ? "Image" : value === "video" ? "Video" : "Audio"}
            </button>
          ))}
        </div>
      )}
      <div className="edit-model">
        <span className="edit-label">Model</span>
        {editable ? (
          <ModelPicker
            models={models.data ?? []}
            model={current}
            loading={models.loading}
            onSelect={(model) => updateNode(node.id, { model: model.id, params: carryParams(node.params, model) })}
          />
        ) : (
          <p>{current?.name ?? node.model}</p>
        )}
        {full.error && (
          <p className="prompt-error" role="alert">
            {full.error}
          </p>
        )}
      </div>
      {(spec?.prompt ?? true) && (
        <label className="field">
          <span>Prompt</span>
          <textarea rows={4} maxLength={20000} value={prompt.text} readOnly={!editable} placeholder="Describe the result" onChange={(event) => prompt.change(event.target.value)} onBlur={prompt.flush} />
        </label>
      )}
      {spec?.negative_prompt && (
        <label className="field">
          <span>Leave out</span>
          <input type="text" maxLength={20000} value={negative.text} readOnly={!editable} onChange={(event) => negative.change(event.target.value)} onBlur={negative.flush} />
        </label>
      )}
      <fieldset className="board-fields" disabled={!editable}>
        {params.map((param) => (
          <ParamField key={param.key} spec={param} value={node.params?.[param.key] ?? param.default ?? null} onChange={(value) => setParam(param.key, value)} />
        ))}
        <label className="field">
          <span>Results per run</span>
          <select value={node.count ?? 1} onChange={(event) => updateNode(node.id, { count: Number(event.target.value) })}>
            {[1, 2, 3, 4, 5, 6, 7, 8].map((count) => (
              <option key={count} value={count}>
                {count}
              </option>
            ))}
          </select>
        </label>
        {spec?.params.some((param) => param.advanced) && (
          <button type="button" className="text-button" onClick={() => setMore(!more)}>
            <Icon name="tune" />
            {more ? "Fewer settings" : "More settings"}
          </button>
        )}
      </fieldset>

      <div className="board-inputs">
        <h3>Inputs</h3>
        {spec && <p className="hint">{spec.inputs.length ? `${spec.name} takes ${spec.inputs.map((slot) => `${(slot.label || roleLabel[slot.role]).toLowerCase()} (up to ${slot.max})`).join(", ")}.` : `${spec.name} takes no files.`}</p>}
        {incoming.length ? (
          <ul>
            {incoming.map((edge) => {
              const source = byId.get(edge.from);
              const asset = source?.asset_id ? board.assets[source.asset_id] : undefined;
              return (
                <li key={edge.id} className="board-input">
                  <span className="board-input-thumb" aria-hidden="true">
                    {asset ? <Thumb asset={asset} /> : <Icon name={source?.kind === "step" ? "auto_awesome" : "hide_image"} />}
                  </span>
                  <span className="board-input-name">{sourceLabel(edge)}</span>
                  <select
                    aria-label={`Use ${sourceLabel(edge)} as`}
                    value={edge.role ?? ""}
                    disabled={!editable}
                    onChange={(event) => setEdgeRole(edge.id, (event.target.value || null) as InputRole | null)}
                  >
                    <option value="">Any free slot</option>
                    {(spec?.inputs ?? []).map((slot) => (
                      <option key={slot.role} value={slot.role}>
                        {slot.label || roleLabel[slot.role]}
                      </option>
                    ))}
                    {edge.role && !spec?.inputs.some((slot) => slot.role === edge.role) && <option value={edge.role}>{roleLabel[edge.role]} (not taken)</option>}
                  </select>
                  {editable && (
                    <button type="button" className="icon-button" aria-label={`Disconnect ${sourceLabel(edge)}`} onClick={() => deleteItems([], [edge.id])}>
                      <Icon name="close" />
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="hint">Connect a file or another step to feed this step.</p>
        )}
        {editable && sources.length > 0 && (
          <label className="field">
            <span className="sr-only">Connect an item</span>
            <select value="" onChange={(event) => event.target.value && connect(event.target.value, node.id)}>
              <option value="">Connect an item</option>
              {sources.map((source) => (
                <option key={source.id} value={source.id}>
                  {titleOf(source)}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>

      {run && (
        <p className={`board-run-line ${run.status}`} role="status">
          {active && <Spinner />}
          {runLabel[run.status]}
          {run.error ? `. ${run.error}` : ""}
          {!active && `, ${formatDate(run.updated_at)}`}
        </p>
      )}
      {editable &&
        (active ? (
          <button type="button" className="text-button" onClick={() => onCancel(node.id)}>
            <Icon name="close" />
            Cancel the run
          </button>
        ) : (
          <div className="board-run-actions">
            <button type="button" className="primary-button generate edit-run" onClick={() => onRun(node.id, false)}>
              <span>Run</span>
              <Icon name="play_arrow" />
            </button>
            {feeds && (
              <button type="button" className="text-button" onClick={() => onRun(node.id, true)}>
                <Icon name="fast_forward" />
                Run with the steps after it
              </button>
            )}
          </div>
        ))}
      {editable && <RemoveButton ids={[node.id]} label="Delete step" onDone={onClose} />}
    </section>
  );
}

/** Several items selected. */
export function SelectionPanel({ board, ids, nodes, editable, onClose, onUseInCreate, onNewStep }: { board: Board; ids: string[]; nodes: BoardNode[]; editable: boolean; onClose(): void; onUseInCreate(assetIds: string[]): void; onNewStep(from: string[]): void }) {
  const chosen = nodes.filter((node) => ids.includes(node.id));
  const images = chosen.flatMap((node) => (node.asset_id && board.assets[node.asset_id]?.kind === "image" ? [node.asset_id] : []));
  const feeders = chosen.filter((node) => node.kind !== "note").map((node) => node.id);
  return (
    <section className="board-inspector" aria-label="Selected items">
      <PanelHead title={`${chosen.length} selected`} onClose={onClose} />
      <div className="detail-actions">
        {images.length > 0 && (
          <button type="button" className="text-button" onClick={() => onUseInCreate(images)}>
            <Icon name="arrow_upward" />
            Use {images.length === 1 ? "the image" : `${images.length} images`} as references
          </button>
        )}
        {editable && feeders.length > 0 && (
          <button type="button" className="text-button" onClick={() => onNewStep(feeders)}>
            <Icon name="auto_awesome" />
            New step from these
          </button>
        )}
      </div>
      {editable && <RemoveButton ids={chosen.map((node) => node.id)} label={`Delete ${chosen.length} items`} onDone={onClose} />}
    </section>
  );
}

/** Hosted mode: the owner shares by email; members see who else is on the board and can leave. */
export function ShareDialog({ board, myId, onClose, onLeft }: { board: Board; myId: string; onClose(): void; onLeft(): void }) {
  const owner = board.role === "owner";
  const [members, setMembers] = useState<BoardMember[]>(board.members);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<BoardRole>("editor");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setMembers(board.members), [board.members]);

  const share = async (event: FormEvent | null, target = email, as = role) => {
    event?.preventDefault();
    if (!target.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      setMembers((await api.shareBoard(board.id, target.trim(), as)).members);
      if (target === email) setEmail("");
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  };
  const remove = async (member: BoardMember) => {
    try {
      await api.removeMember(board.id, member.id);
      if (member.id === myId) return onLeft();
      setMembers((list) => list.filter((item) => item.id !== member.id));
    } catch (failure) {
      notifyError(errorMessage(failure));
    }
  };
  const name = (person: { name: string | null; email: string | null }) => person.name || person.email || "Someone";

  return (
    <Dialog title={owner ? "Share board" : "People on this board"} onClose={onClose}>
      <div className="board-share">
        {owner && (
          <form className="board-share-form" onSubmit={(event) => void share(event)}>
            <label className="field">
              <span>Email</span>
              <input type="email" required value={email} autoComplete="off" placeholder="name@example.com" onChange={(event) => setEmail(event.target.value)} aria-invalid={Boolean(error) || undefined} />
            </label>
            <label className="field">
              <span>Role</span>
              <select value={role} onChange={(event) => setRole(event.target.value as BoardRole)}>
                <option value="editor">Editor</option>
                <option value="viewer">Viewer</option>
              </select>
            </label>
            <button type="submit" className="primary-button" disabled={busy || !email.trim()}>
              {busy ? <Spinner /> : <Icon name="person_add" />}
              Share
            </button>
          </form>
        )}
        {error && (
          <p className="notice error" role="alert">
            {error}
          </p>
        )}
        <p className="hint">Editors change the board and run its steps, paid from their own account. Viewers only look.</p>
        <ul className="board-members">
          <li className="connection-row">
            <span>
              {name(board.owner)}
              <small>Owner</small>
            </span>
          </li>
          {members.map((member) => (
            <li key={member.id} className="connection-row">
              <span>
                {name(member)}
                <small>{member.email && member.name ? `${member.email}, ` : ""}added {formatDate(member.added_at)}</small>
              </span>
              {owner ? (
                <span className="board-member-actions">
                  <select aria-label={`Role of ${name(member)}`} value={member.role} onChange={(event) => void share(null, member.email ?? "", event.target.value as BoardRole)}>
                    <option value="editor">Editor</option>
                    <option value="viewer">Viewer</option>
                  </select>
                  <button type="button" className="icon-button" aria-label={`Remove ${name(member)}`} onClick={() => void remove(member)}>
                    <Icon name="close" />
                  </button>
                </span>
              ) : member.id === myId ? (
                <button type="button" className="text-button" onClick={() => void remove(member)}>
                  <Icon name="logout" />
                  Leave
                </button>
              ) : (
                <small>{member.role === "editor" ? "Editor" : "Viewer"}</small>
              )}
            </li>
          ))}
        </ul>
      </div>
    </Dialog>
  );
}
