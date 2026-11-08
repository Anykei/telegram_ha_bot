ALTER TABLE pending_commands ADD COLUMN result_text TEXT;
ALTER TABLE pending_commands ADD COLUMN result_json TEXT;
ALTER TABLE ui_confirmations ADD COLUMN result_json TEXT;

CREATE TABLE command_choices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    context_json TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'selected', 'cancelled'))
);
CREATE INDEX command_choices_owner_status ON command_choices(user_id, status);

CREATE TABLE ui_action_buttons (
    user_id INTEGER NOT NULL,
    button_key TEXT NOT NULL,
    confirmation_id INTEGER NOT NULL REFERENCES ui_confirmations(id) ON DELETE CASCADE,
    requires_review INTEGER NOT NULL,
    PRIMARY KEY(user_id, button_key)
);
