ALTER TABLE user_profiles ADD COLUMN can_use_voice INTEGER NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS pending_commands (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    source TEXT NOT NULL,
    command_text TEXT NOT NULL,
    intent_json TEXT NOT NULL,
    reason TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    expires_at TEXT NOT NULL,
    confirmed_at TEXT,
    cancelled_at TEXT,
    executed_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
    CHECK(source IN ('text', 'voice')),
    CHECK(status IN ('pending', 'confirmed', 'cancelled', 'expired', 'executed', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_pending_commands_user_status
ON pending_commands(user_id, status, expires_at);
