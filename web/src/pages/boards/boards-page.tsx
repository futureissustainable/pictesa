import { useEffect, useState, type FormEvent } from "react";
import { api, errorMessage } from "../../lib/api";
import { onReconnect, subscribeEvents } from "../../lib/events";
import { formatDate, plural } from "../../lib/format";
import { Link, navigate } from "../../lib/router";
import { useSession } from "../../lib/session";
import type { BoardSummary } from "../../lib/types";
import { Thumb } from "../../ui/asset-card";
import { Dialog } from "../../ui/dialog";
import { Icon, Spinner } from "../../ui/icon";

/** The boards this person owns or that were shared with them, newest change first. */
function useBoards() {
  const [boards, setBoards] = useState<BoardSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () =>
      api.boards().then(
        ({ boards: list }) => alive && (setBoards(list), setError(null)),
        (failure: unknown) => alive && setError(errorMessage(failure)),
      );
    void load();
    // Created, renamed, shared or deleted anywhere: reload the short list.
    const stop = subscribeEvents({ boards: () => void load() });
    const stopReconnect = onReconnect(() => void load());
    return () => {
      alive = false;
      stop();
      stopReconnect();
    };
  }, []);
  return { boards, error };
}

export default function BoardsPage() {
  const { boards, error } = useBoards();
  const session = useSession();
  const hosted = session.status === "ready" && session.me.mode === "hosted";
  const [creating, setCreating] = useState(false);

  return (
    <div className="workspace">
      <div className="intro">
        <div>
          <h1>Boards.</h1>
          <p>Collect references, notes and steps on one canvas. Run a step and its results land next to it.</p>
        </div>
        {/* An empty list has its own New board button. */}
        {boards && boards.length > 0 && (
          <button type="button" className="primary-button" onClick={() => setCreating(true)}>
            <Icon name="add" />
            New board
          </button>
        )}
      </div>
      {!boards ? (
        <div className={error ? "empty" : "page-loading"} role={error ? "alert" : "status"}>
          {error ? <Icon name="cloud_off" /> : <Spinner />}
          {error ? <p>{error}</p> : "Loading boards"}
        </div>
      ) : boards.length ? (
        <ul className="board-grid" aria-label="Boards">
          {boards.map((board) => (
            <li key={board.id}>
              <Link to={`/boards/${encodeURIComponent(board.id)}`} className="board-card">
                <span className={`board-cover cover-${Math.min(4, Math.max(1, board.covers.length))}`} aria-hidden="true">
                  {board.covers.length ? board.covers.map((asset) => <Thumb key={asset.id} asset={asset} />) : <Icon name="schema" />}
                </span>
                <span className="character-name">{board.name}</span>
                <span className="character-meta">
                  {plural(board.node_count, "item", "items")}
                  {board.role !== "owner" ? `, shared by ${board.owner.name || board.owner.email || "someone"}` : hosted && board.member_count ? `, shared with ${board.member_count}` : ""}
                </span>
                <span className="character-meta">Changed {formatDate(board.updated_at)}</span>
              </Link>
            </li>
          ))}
        </ul>
      ) : (
        <div className="empty">
          <Icon name="schema" />
          <h2>No boards yet.</h2>
          <p>Start one for a project, a shoot or a moodboard. Add files, write notes and chain steps.</p>
          <button type="button" className="primary-button" onClick={() => setCreating(true)}>
            New board
            <Icon name="arrow_forward" />
          </button>
        </div>
      )}
      {creating && <NewBoard onClose={() => setCreating(false)} />}
    </div>
  );
}

function NewBoard({ onClose }: { onClose(): void }) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const create = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    try {
      const board = await api.createBoard(name.trim() || "Untitled board");
      onClose();
      navigate(`/boards/${encodeURIComponent(board.id)}`);
    } catch (failure) {
      setError(errorMessage(failure));
      setBusy(false);
    }
  };
  return (
    <Dialog title="New board" onClose={onClose}>
      <form className="library-form" onSubmit={(event) => void create(event)}>
        <label className="field">
          <span>Name</span>
          <input value={name} maxLength={100} autoFocus placeholder="Untitled board" onChange={(event) => setName(event.target.value)} aria-invalid={Boolean(error) || undefined} />
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
          <button type="submit" className="primary-button" disabled={busy}>
            {busy ? <Spinner /> : <Icon name="add" />}
            Create
          </button>
        </div>
      </form>
    </Dialog>
  );
}
