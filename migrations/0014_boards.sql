-- Boards: a canvas of library files, notes and steps, stored as one JSON document per board.
-- `version` counts applied changes. Every node and edge in the document records the version of its
-- own last change, so a write based on an older state of the same item is refused. `active_runs`
-- counts steps that are waiting, starting or running, so the board watcher reads only those boards.
CREATE TABLE boards (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    document TEXT NOT NULL DEFAULT '{"nodes":[],"edges":[],"runs":{}}',
    version INTEGER NOT NULL DEFAULT 0,
    active_runs INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX boards_by_owner ON boards (owner_id, updated_at DESC);
CREATE INDEX boards_by_update ON boards (updated_at);
CREATE INDEX boards_with_runs ON boards (active_runs) WHERE active_runs > 0;

-- People a board is shared with (hosted mode): editors change it, viewers only look.
CREATE TABLE board_members (
    board_id TEXT NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('editor', 'viewer')),
    created_at INTEGER NOT NULL,
    PRIMARY KEY (board_id, user_id)
);
CREATE INDEX board_members_by_user ON board_members (user_id);
