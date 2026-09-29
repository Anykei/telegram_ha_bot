CREATE TABLE ui_confirmations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    action_json TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending', 'executing', 'executed', 'failed', 'cancelled')),
    result_text TEXT
);
CREATE INDEX ui_confirmations_user ON ui_confirmations(user_id, status);
