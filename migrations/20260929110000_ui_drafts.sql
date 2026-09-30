CREATE TABLE ui_drafts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    revision INTEGER NOT NULL DEFAULT 0,
    payload TEXT NOT NULL,
    step TEXT NOT NULL DEFAULT '',
    return_context TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'editing' CHECK(status IN ('editing','review','saved','cancelled','expired')),
    updated_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    committed_target_id INTEGER
);
CREATE INDEX idx_ui_drafts_owner_kind ON ui_drafts(owner_id, kind, status, updated_at);
