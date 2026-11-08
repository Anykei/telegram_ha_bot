ALTER TABLE ui_confirmations ADD COLUMN reuse_key TEXT;
ALTER TABLE ui_confirmations ADD COLUMN replacement_id INTEGER;
CREATE INDEX ui_confirmations_pending_reuse ON ui_confirmations(user_id, reuse_key, status, expires_at);

-- Legacy button identities are replay tombstones. Deleting an expired proposal
-- (or removing/re-adding its owner) must not allow that old button to bind a new
-- executable operation. Generated callback IDs are AUTOINCREMENT and not reused.
ALTER TABLE ui_action_buttons RENAME TO ui_action_buttons_old;
CREATE TABLE ui_action_buttons (
    user_id INTEGER NOT NULL,
    button_key TEXT NOT NULL,
    confirmation_id INTEGER NOT NULL,
    requires_review INTEGER NOT NULL,
    PRIMARY KEY(user_id, button_key)
);
INSERT INTO ui_action_buttons SELECT * FROM ui_action_buttons_old;
DROP TABLE ui_action_buttons_old;
