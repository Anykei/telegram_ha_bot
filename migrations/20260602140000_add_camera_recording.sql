CREATE TABLE IF NOT EXISTS camera_recording_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    camera_id INTEGER NOT NULL,
    condition_logic TEXT NOT NULL DEFAULT 'any',
    tail_seconds INTEGER NOT NULL DEFAULT 60,
    max_segment_seconds INTEGER NOT NULL DEFAULT 300,
    cooldown_s INTEGER NOT NULL DEFAULT 0,
    retention_days INTEGER NOT NULL DEFAULT 30,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_completed_at TEXT,
    deleted_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(camera_id) REFERENCES cameras(id),
    CHECK(condition_logic IN ('all', 'any'))
);

CREATE INDEX IF NOT EXISTS idx_camera_recording_rules_camera
ON camera_recording_rules(camera_id);

CREATE INDEX IF NOT EXISTS idx_camera_recording_rules_enabled
ON camera_recording_rules(enabled, deleted_at);

CREATE TABLE IF NOT EXISTS camera_recording_rule_conditions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rule_id INTEGER NOT NULL,
    entity_id TEXT NOT NULL,
    operator TEXT NOT NULL DEFAULT 'changed_to',
    from_state TEXT,
    to_state TEXT,
    value TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(rule_id) REFERENCES camera_recording_rules(id),
    CHECK(operator IN (
        'changed_to',
        'changed_from_to',
        'is',
        'is_not',
        'contains',
        'above',
        'below'
    ))
);

CREATE INDEX IF NOT EXISTS idx_camera_recording_rule_conditions_event
ON camera_recording_rule_conditions(entity_id, to_state);

CREATE INDEX IF NOT EXISTS idx_camera_recording_rule_conditions_rule
ON camera_recording_rule_conditions(rule_id);

CREATE TABLE IF NOT EXISTS camera_recording_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_group_id TEXT NOT NULL,
    rule_id INTEGER NOT NULL,
    camera_id INTEGER NOT NULL,
    extended_by_rule_ids TEXT,
    trigger_summary TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT,
    first_event_at TEXT NOT NULL,
    last_event_at TEXT NOT NULL,
    stop_after_at TEXT NOT NULL,
    started_at TEXT,
    completed_at TEXT,
    expires_at TEXT NOT NULL,
    notification_sent_at TEXT,
    deleted_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(rule_id) REFERENCES camera_recording_rules(id),
    FOREIGN KEY(camera_id) REFERENCES cameras(id)
);

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_active
ON camera_recording_sessions(camera_id, status, deleted_at);

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_camera_created
ON camera_recording_sessions(camera_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_expires
ON camera_recording_sessions(expires_at, status, deleted_at);

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_event_group
ON camera_recording_sessions(event_group_id);

CREATE TABLE IF NOT EXISTS camera_recording_segments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL,
    camera_id INTEGER NOT NULL,
    segment_index INTEGER NOT NULL,
    file_path TEXT,
    duration_s INTEGER NOT NULL,
    size_bytes INTEGER,
    status TEXT NOT NULL,
    error TEXT,
    started_at TEXT,
    completed_at TEXT,
    expires_at TEXT NOT NULL,
    deleted_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(session_id) REFERENCES camera_recording_sessions(id),
    FOREIGN KEY(camera_id) REFERENCES cameras(id),
    UNIQUE(session_id, segment_index)
);

CREATE INDEX IF NOT EXISTS idx_camera_recording_segments_session
ON camera_recording_segments(session_id, segment_index);

CREATE INDEX IF NOT EXISTS idx_camera_recording_segments_camera_created
ON camera_recording_segments(camera_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_camera_recording_segments_expires
ON camera_recording_segments(expires_at, status, deleted_at);
