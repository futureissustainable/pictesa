import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { api, errorMessage, uploadFiles } from "../../lib/api";
import { fileKind } from "../../lib/attach";
import { addNodes, deleteItems, flushDrafts, newId, renameBoard, updateNode, useOpenBoard, usePresence, whenSaved } from "../../lib/boards";
import { defaultModel, fallbackModelName, loadModels, peekModels, useModels } from "../../lib/catalog";
import { composer } from "../../lib/composer";
import { emitLocal } from "../../lib/events";
import { assetTitle } from "../../lib/names";
import { notify, notifyError } from "../../lib/notices";
import { PHONE, useMediaQuery } from "../../lib/prefs";
import { sendToCreate } from "../../lib/reuse";
import { Link, navigate } from "../../lib/router";
import { useSession } from "../../lib/session";
import type { Asset, BoardNode, Person } from "../../lib/types";
import { openViewer, useSequence } from "../../lib/viewer";
import { Choice, Control } from "../../ui/control";
import { Icon, Spinner } from "../../ui/icon";
import { Canvas, fitView, nodeHeight, toBoard, typing, zoomAround, type Point, type Viewport } from "./canvas";
import { assetHeight, type NodeActions } from "./nodes";
import { Inspector, LibraryPanel, SelectionPanel, ShareDialog } from "./panel";

const noop = () => undefined;
interface Box {
  w: number;
  h: number;
}

/** The model a new step starts with: the one picked in Create when it is set up, else the default. */
async function stepModel(): Promise<string | null> {
  await loadModels("image");
  const models = peekModels("image") ?? [];
  const picked = models.find((model) => model.id === composer.get().modelByMode.image && model.configured);
  return (picked ?? defaultModel(models))?.id ?? null;
}

export default function BoardPage({ id }: { id: string }) {
  const { state, view } = useOpenBoard(id);
  const session = useSession();
  const me = session.status === "ready" ? session.me : null;
  const hosted = me?.mode === "hosted";
  const myId = me?.user?.id ?? "local";
  const phone = useMediaQuery(PHONE);
  const board = state.board;
  const editable = Boolean(board && board.role !== "viewer");
  usePresence(id, hosted && Boolean(board));

  const canvasRef = useRef<HTMLDivElement | null>(null);
  const measured = useMemo(() => new Map<string, number>(), []);
  const [viewport, setViewport] = useState<Viewport>({ x: 48, y: 48, z: 1 });
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [selectedEdge, setSelectedEdge] = useState<string | null>(null);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [library, setLibrary] = useState(!phone);
  const [sharing, setSharing] = useState(false);
  const fitted = useRef(false);

  const image = useModels("image").data;
  const video = useModels("video").data;
  const audio = useModels("audio").data;
  const models = useMemo(() => new Map([...(image ?? []), ...(video ?? []), ...(audio ?? [])].map((model) => [model.id, model])), [image, video, audio]);
  const nameOf = useCallback((model: string | null | undefined) => (model ? (models.get(model)?.name ?? fallbackModelName(model)) : ""), [models]);
  const modelOf = useCallback((model?: string) => ({ name: nameOf(model), prompt: !model || models.get(model)?.prompt !== false }), [models, nameOf]);
  const assetName = useCallback((asset: Asset) => assetTitle(asset, nameOf), [nameOf]);
  const titleOf = useCallback(
    (node: BoardNode) => {
      if (node.kind === "note") return `Note: ${(node.text ?? "").trim().slice(0, 60) || "empty"}`;
      if (node.kind === "step") return `Step: ${node.model ? nameOf(node.model) : "no model"}`;
      const asset = node.asset_id ? board?.assets[node.asset_id] : undefined;
      return asset ? assetName(asset) : "Missing file";
    },
    [board?.assets, nameOf, assetName],
  );

  // The viewer steps through the board's files in board order.
  const files = useMemo(() => (view && board ? view.nodes.flatMap((node) => (node.asset_id && board.assets[node.asset_id] ? [board.assets[node.asset_id]!] : [])) : []), [view, board]);
  useSequence(files, false, noop);

  // Fit the board once it has loaded and its nodes have been measured. A phone starts readable.
  useEffect(() => {
    if (fitted.current || !view || !board || !canvasRef.current) return;
    fitted.current = true;
    const frame = requestAnimationFrame(() => {
      const rect = canvasRef.current?.getBoundingClientRect();
      if (rect) setViewport(fitView(board, view.nodes, measured, rect.width, rect.height, rect.width < 600 ? 0.5 : undefined));
    });
    return () => cancelAnimationFrame(frame);
  }, [view, board, measured]);

  // On a phone the panel is a sheet over the lower part: bring a newly selected item above it.
  const focusId = selected.size === 1 ? [...selected][0]! : null;
  useEffect(() => {
    const node = focusId ? view?.nodes.find((item) => item.id === focusId) : undefined;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!phone || !node || !rect) return;
    setViewport((current) => {
      const top = node.y * current.z + current.y;
      const left = node.x * current.z + current.x;
      const width = Math.min(node.w * current.z, rect.width - 32);
      const y = top < 8 || top > rect.height * 0.3 ? current.y + 16 - top : current.y;
      const x = left < 0 || left + width > rect.width ? current.x + (rect.width - width) / 2 - left : current.x;
      return x === current.x && y === current.y ? current : { ...current, x, y };
    });
    // Only when the selection changes: dragging the item must not move the view.
  }, [phone, focusId]);

  // Selection follows the items that still exist.
  useEffect(() => {
    if (!view) return;
    const ids = new Set(view.nodes.map((node) => node.id));
    setSelected((current) => ([...current].every((item) => ids.has(item)) ? current : new Set([...current].filter((item) => ids.has(item)))));
    setSelectedEdge((current) => (current && !view.edges.some((edge) => edge.id === current) ? null : current));
  }, [view]);

  const center = (): Point => {
    const rect = canvasRef.current?.getBoundingClientRect();
    return rect ? toBoard(viewport, rect.width / 2, rect.height / 2) : { x: 0, y: 0 };
  };
  const fit = () => {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (rect && view && board) setViewport(fitView(board, view.nodes, measured, rect.width, rect.height));
  };
  const zoom = (factor: number) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    if (rect) setViewport((current) => zoomAround(current, factor, rect.width / 2, rect.height / 2));
  };
  const select = useCallback((ids: string[], mode: "replace" | "toggle") => {
    setSelectedEdge(null);
    setSelected((current) => {
      if (mode === "replace") return new Set(ids);
      const next = new Set(current);
      for (const item of ids) {
        if (next.has(item)) next.delete(item);
        else next.add(item);
      }
      return next;
    });
  }, []);
  const selectEdge = useCallback((edge: string | null) => {
    setSelectedEdge(edge);
    if (edge) setSelected(new Set());
  }, []);

  /** The spot itself when it is free, else the nearest free place around it, so a new item never covers another. */
  const freeSpot = (spot: Point, size: Box, taken: (Point & Box)[] = []): Point => {
    const boxes = [
      // A file just added can be measured before its picture arrives, so its expected height counts too.
      ...(view?.nodes ?? []).map((node) => {
        const asset = node.asset_id ? board?.assets[node.asset_id] : undefined;
        return { x: node.x, y: node.y, w: node.w, h: Math.max(nodeHeight(node, measured, asset), node.kind === "asset" ? assetHeight(node.w, asset) : 0) };
      }),
      ...taken,
    ];
    const gap = 24;
    const free = (point: Point) => !boxes.some((box) => point.x < box.x + box.w + gap && point.x + size.w + gap > box.x && point.y < box.y + box.h + gap && point.y + size.h + gap > box.y);
    const rect = canvasRef.current?.getBoundingClientRect();
    const from = rect ? toBoard(viewport, 0, 0) : null;
    const to = rect ? toBoard(viewport, rect.width, rect.height) : null;
    const seen = (point: Point) => !from || !to || (point.x >= from.x && point.y >= from.y && point.x + size.w <= to.x && point.y + size.h <= to.y);
    // Places on a 48 px grid around the spot, nearest first, and to the right before the left.
    const candidates: Point[] = [];
    for (let dx = -24; dx <= 24; dx++) for (let dy = -24; dy <= 24; dy++) candidates.push({ x: spot.x + dx * 48, y: spot.y + dy * 48 });
    candidates.sort((a, b) => Math.hypot(a.x - spot.x, a.y - spot.y) - Math.hypot(b.x - spot.x, b.y - spot.y) || b.x - a.x);
    // A free place in sight first, then any free place.
    const found = candidates.find((point) => free(point) && seen(point)) ?? candidates.find(free) ?? spot;
    return { x: Math.round(found.x), y: Math.round(found.y) };
  };
  // Files do not take the selection, so the library stays open for the next one.
  const addAssets = (assets: Asset[], at: Point) => {
    const taken: (Point & Box)[] = [];
    addNodes(
      assets.map((asset, index) => {
        const size = { w: 240, h: assetHeight(240, asset) };
        const spot = freeSpot({ x: at.x - 120 + index * 264, y: at.y - 80 }, size, taken);
        taken.push({ ...spot, ...size });
        return { id: newId(), kind: "asset" as const, ...spot, asset_id: asset.id };
      }),
    );
  };
  const addFiles = async (files: File[], at: Point) => {
    const usable = files.filter((file) => fileKind(file));
    if (!usable.length) return notifyError("Drop images, videos or audio files.");
    notify(usable.length === 1 ? `Uploading ${usable[0]!.name}.` : `Uploading ${usable.length} files.`);
    try {
      const { assets, errors } = await uploadFiles(usable);
      assets.forEach((asset) => emitLocal({ asset }));
      if (assets.length) addAssets(assets, at);
      if (errors.length) notifyError(`${errors[0]!.name}: ${errors[0]!.error}`);
    } catch (error) {
      notifyError(errorMessage(error));
    }
  };
  const addNote = (text = "", at = center()) => {
    const node = { id: newId(), kind: "note" as const, ...freeSpot({ x: at.x - 120, y: at.y - 60 }, { w: 240, h: 140 }), text };
    addNodes([node]);
    setSelected(new Set([node.id]));
    requestAnimationFrame(() => document.querySelector<HTMLTextAreaElement>(`[data-node="${node.id}"] textarea`)?.focus());
  };
  const newStep = async (from: string[], at?: Point) => {
    const model = await stepModel();
    if (!model) return notifyError("No image model is set up yet. Add a provider in Settings.");
    const sources = view?.nodes.filter((node) => from.includes(node.id)) ?? [];
    const place = at ?? (sources.length ? { x: Math.max(...sources.map((node) => node.x + node.w)) + 80 + 150, y: Math.min(...sources.map((node) => node.y)) + 60 } : center());
    const step = { id: newId(), kind: "step" as const, ...freeSpot({ x: place.x - 150, y: place.y - 60 }, { w: 300, h: 190 }), model, prompt: "" };
    addNodes([step], from.map((source) => ({ from: source, to: step.id })));
    setSelected(new Set([step.id]));
  };
  const useInCreate = async (assetIds: string[]) => {
    try {
      const { assets } = await api.copyBoardAssets(id, assetIds);
      await sendToCreate(assets.filter((asset) => asset.kind === "image"), "image", "reference");
    } catch (error) {
      notifyError(errorMessage(error));
    }
  };
  const removeSelection = () => {
    if (selected.size) deleteItems([...selected]);
    else if (selectedEdge) deleteItems([], [selectedEdge]);
  };

  // Results land beside their step. After a run started here, the view follows them when they land out of sight.
  const following = useRef<{ step: string; until: number } | null>(null);
  const seenNodes = useRef<Set<string> | null>(null);
  useEffect(() => {
    if (!view || !board) return;
    const before = seenNodes.current;
    seenNodes.current = new Set(view.nodes.map((node) => node.id));
    const follow = following.current;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!before || !follow || Date.now() > follow.until || !rect) return;
    const added = view.nodes.filter((node) => node.kind === "asset" && !before.has(node.id));
    if (!added.length) return;
    const from = toBoard(viewport, 0, 0);
    const to = toBoard(viewport, rect.width, rect.height);
    const fits = (node: BoardNode) => node.x >= from.x && node.x + node.w <= to.x && node.y >= from.y && node.y + assetHeight(node.w, node.asset_id ? board.assets[node.asset_id] : undefined) <= to.y;
    if (added.every(fits)) return;
    const step = view.nodes.find((node) => node.id === follow.step);
    setViewport(fitView(board, step ? [step, ...added] : added, measured, rect.width, rect.height));
    // Runs only when the items change; the viewport is read as it is now.
  }, [view]);

  const run = async (nodeId: string, downstream: boolean) => {
    flushDrafts();
    await whenSaved();
    following.current = { step: nodeId, until: Date.now() + 10 * 60_000 };
    try {
      await api.runStep(id, nodeId, downstream);
    } catch (error) {
      notifyError(errorMessage(error));
    }
  };
  const actions = useMemo<NodeActions>(
    () => ({
      run: (nodeId, downstream) => void run(nodeId, downstream),
      cancel: (nodeId) => void api.cancelStep(id, nodeId).catch((error: unknown) => notifyError(errorMessage(error))),
      text: (nodeId, text, seen) => updateNode(nodeId, { text }, seen),
      open: (asset) => openViewer(asset),
    }),
    // `run` uses only the board id.
    [id],
  );

  // Paste: files become asset nodes, text becomes a note, in the middle of the view.
  const pasteRef = useRef<(event: ClipboardEvent) => void>(noop);
  pasteRef.current = (event: ClipboardEvent) => {
    if (!editable || typing(event.target) || document.querySelector("dialog[open]")) return;
    const files = [...(event.clipboardData?.files ?? [])];
    const text = event.clipboardData?.getData("text/plain")?.trim();
    if (files.length) {
      event.preventDefault();
      void addFiles(files, center());
    } else if (text) {
      event.preventDefault();
      addNote(text.slice(0, 10000));
    }
  };
  useEffect(() => {
    const paste = (event: ClipboardEvent) => pasteRef.current(event);
    window.addEventListener("paste", paste);
    return () => window.removeEventListener("paste", paste);
  }, []);

  if (!board || !view || state.status === "gone") {
    const gone = state.status === "gone" || state.status === "error";
    return (
      <div className="workspace">
        <div className={gone ? "empty" : "page-loading"} role={gone ? "alert" : "status"}>
          {gone ? <Icon name="search_off" /> : <Spinner />}
          {gone ? <h1>{state.status === "gone" && !hosted ? "This board was deleted." : state.error}</h1> : "Loading the board"}
          {gone && (
            <Link className="primary-button" to="/boards">
              All boards
              <Icon name="arrow_forward" />
            </Link>
          )}
        </div>
      </div>
    );
  }

  const one = selected.size === 1 ? view.nodes.find((node) => selected.has(node.id)) : undefined;
  const panel = one ? (
    <Inspector
      board={board}
      node={one}
      nodes={view.nodes}
      edges={view.edges}
      editable={editable}
      titleOf={titleOf}
      onClose={() => setSelected(new Set())}
      onRun={actions.run}
      onCancel={actions.cancel}
      onUseInCreate={(ids) => void useInCreate(ids)}
      onNewStep={(from) => void newStep(from)}
      onOpenAsset={openViewer}
    />
  ) : selected.size > 1 ? (
    <SelectionPanel board={board} ids={[...selected]} nodes={view.nodes} editable={editable} onClose={() => setSelected(new Set())} onUseInCreate={(ids) => void useInCreate(ids)} onNewStep={(from) => void newStep(from)} />
  ) : library && editable ? (
    <LibraryPanel onAdd={(asset) => addAssets([asset], center())} titleOf={assetName} />
  ) : null;
  const others = state.viewers.filter((person) => person.id !== myId);

  return (
    <div className="workspace workspace-board">
      <header className="board-head">
        <Link to="/boards" className="icon-button" aria-label="All boards" title="All boards">
          <Icon name="arrow_back" />
        </Link>
        <BoardName name={view.name} editable={editable} />
        {board.role === "viewer" && <span className="badge">View only</span>}
        <div className="board-head-right">
          {hosted && others.length > 0 && <Viewers people={others} />}
          {hosted && (
            <button type="button" className="text-button" onClick={() => setSharing(true)}>
              <Icon name={board.role === "owner" ? "person_add" : "group"} />
              <span className="board-head-label">{board.role === "owner" ? "Share" : "People"}</span>
            </button>
          )}
          <BoardMenu boardId={id} myId={myId} owner={board.role === "owner"} hosted={hosted} />
        </div>
      </header>
      <div className={`board-layout ${panel ? "with-panel" : ""}`}>
        <div className="board-stage">
          <Canvas
            board={board}
            view={view}
            editable={editable}
            viewport={viewport}
            setViewport={setViewport}
            canvasRef={canvasRef}
            measured={measured}
            selected={selected}
            selectedEdge={selectedEdge}
            onSelect={select}
            onSelectEdge={selectEdge}
            connecting={connecting}
            onConnecting={setConnecting}
            onConnectToEmpty={(from, at) => void newStep([from], at)}
            onDropAssets={addAssets}
            onDropFiles={(files, at) => void addFiles(files, at)}
            onDelete={removeSelection}
            onFit={fit}
            actions={actions}
            titleOf={titleOf}
            modelOf={modelOf}
          />
          <div className="board-toolbar" role="toolbar" aria-label="Board tools">
            {editable && (
              <>
                <button type="button" className="icon-button" aria-label="Add a note" title="Add a note" onClick={() => addNote()}>
                  <Icon name="sticky_note_2" />
                </button>
                <button type="button" className="icon-button" aria-label="Add a step" title="Add a step" onClick={() => void newStep([])}>
                  <Icon name="auto_awesome" />
                </button>
                <button type="button" className={`icon-button ${library && !one ? "selected" : ""}`} aria-label="Library" aria-pressed={library} title="Library" onClick={() => {
                  setSelected(new Set());
                  setLibrary(!library || selected.size > 0);
                }}>
                  <Icon name="photo_library" />
                </button>
                <span className="board-toolbar-gap" />
              </>
            )}
            <button type="button" className="icon-button" aria-label="Zoom out" onClick={() => zoom(0.8)}>
              <Icon name="zoom_out" />
            </button>
            <button type="button" className="zoom-value" aria-label="Fit the board to the view" onClick={fit}>
              {Math.round(viewport.z * 100)}%
            </button>
            <button type="button" className="icon-button" aria-label="Zoom in" onClick={() => zoom(1.25)}>
              <Icon name="zoom_in" />
            </button>
          </div>
        </div>
        {panel && <aside className="board-panel">{panel}</aside>}
      </div>
      {sharing && <ShareDialog board={board} myId={myId} onClose={() => setSharing(false)} onLeft={() => navigate("/boards")} />}
    </div>
  );
}

function BoardName({ name, editable }: { name: string; editable: boolean }) {
  const [editing, setEditing] = useState<string | null>(null);
  if (!editable) return <h1 className="board-title">{name}</h1>;
  if (editing === null) {
    return (
      <button type="button" className="board-title board-title-button" onClick={() => setEditing(name)} aria-label={`Rename ${name}`}>
        {name}
      </button>
    );
  }
  const save = (event?: FormEvent) => {
    event?.preventDefault();
    const next = editing.trim();
    if (next && next !== name) renameBoard(next);
    setEditing(null);
  };
  return (
    <form className="board-rename" onSubmit={save}>
      <input
        aria-label="Board name"
        value={editing}
        maxLength={100}
        autoFocus
        onChange={(event) => setEditing(event.target.value)}
        onBlur={() => save()}
        onKeyDown={(event) => {
          if (event.key === "Escape") setEditing(null);
        }}
      />
    </form>
  );
}

function Viewers({ people }: { people: Person[] }) {
  const label = (person: Person) => person.name || person.email || "Someone";
  return (
    <ul className="board-viewers" aria-label={`Also here: ${people.map(label).join(", ")}`}>
      {people.slice(0, 4).map((person) => (
        <li key={person.id} className="avatar" title={`${label(person)} is here`}>
          {label(person).slice(0, 1).toUpperCase()}
        </li>
      ))}
      {people.length > 4 && <li className="avatar">+{people.length - 4}</li>}
    </ul>
  );
}

function BoardMenu({ boardId, myId, owner, hosted }: { boardId: string; myId: string; owner: boolean; hosted: boolean }) {
  if (!owner && !hosted) return null;
  return (
    <Control label="Board" icon="more_horiz" caret={false} showTitle={false} triggerProps={{ title: "More" }}>
      {owner ? (
        <Choice
          icon="delete"
          onPick={() => {
            if (!confirm("Delete this board for everyone? Its files stay in the libraries.")) return;
            api.deleteBoard(boardId).then(
              () => navigate("/boards"),
              (error: unknown) => notifyError(errorMessage(error)),
            );
          }}
        >
          Delete board
        </Choice>
      ) : (
        <Choice
          icon="logout"
          onPick={() => {
            api.removeMember(boardId, myId).then(
              () => navigate("/boards"),
              (error: unknown) => notifyError(errorMessage(error)),
            );
          }}
        >
          Leave board
        </Choice>
      )}
    </Control>
  );
}
