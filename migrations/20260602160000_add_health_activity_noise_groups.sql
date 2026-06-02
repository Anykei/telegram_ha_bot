CREATE TABLE IF NOT EXISTS camera_health (
    camera_id INTEGER PRIMARY KEY,
    last_snapshot_ok_at TEXT,
    last_clip_ok_at TEXT,
    last_recording_ok_at TEXT,
    last_check_at TEXT,
    last_error TEXT,
    last_file_size INTEGER,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(camera_id) REFERENCES cameras(id)
);

CREATE TABLE IF NOT EXISTS activity_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER,
    kind TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT,
    action TEXT NOT NULL,
    status TEXT NOT NULL,
    message TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_activity_log_created
ON activity_log(created_at DESC);

CREATE INDEX IF NOT EXISTS idx_activity_log_kind
ON activity_log(kind, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_activity_log_status
ON activity_log(status, created_at DESC);

CREATE TABLE IF NOT EXISTS camera_recording_rule_groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS camera_recording_rule_group_items (
    group_id INTEGER NOT NULL,
    rule_id INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(group_id, rule_id),
    FOREIGN KEY(group_id) REFERENCES camera_recording_rule_groups(id),
    FOREIGN KEY(rule_id) REFERENCES camera_recording_rules(id)
);

ALTER TABLE camera_recording_rules
ADD COLUMN noise_enabled INTEGER NOT NULL DEFAULT 1;

ALTER TABLE camera_recording_rules
ADD COLUMN noise_summary_sent_at TEXT;

CREATE INDEX IF NOT EXISTS idx_camera_recording_rule_group_items_rule
ON camera_recording_rule_group_items(rule_id);
