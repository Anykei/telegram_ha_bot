-- Manual recordings have no automation rule. Rebuild atomically while keeping
-- foreign keys enabled. Back up dependent rows before replacing the parent;
-- their tables, indexes, IDs and file ownership are preserved.
ALTER TABLE cameras ADD COLUMN archive_manual_clips INTEGER NOT NULL DEFAULT 0
    CHECK(archive_manual_clips IN (0, 1));

CREATE TABLE camera_recording_sessions_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_group_id TEXT NOT NULL,
    rule_id INTEGER,
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
    pinned_at TEXT,
    pinned_by INTEGER,
    pin_note TEXT,
    pre_roll_from TEXT,
    pre_roll_seconds INTEGER NOT NULL DEFAULT 0,
    pre_roll_partial INTEGER NOT NULL DEFAULT 0,
    pre_roll_warning TEXT,
    pre_roll_attempted_seconds INTEGER NOT NULL DEFAULT 0,
    pre_roll_actual_seconds INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(rule_id) REFERENCES camera_recording_rules(id),
    FOREIGN KEY(camera_id) REFERENCES cameras(id)
);

INSERT INTO camera_recording_sessions_new SELECT * FROM camera_recording_sessions;
CREATE TEMP TABLE manual_archive_sequence AS SELECT seq FROM sqlite_sequence WHERE name='camera_recording_sessions';
CREATE TEMP TABLE manual_archive_segments AS SELECT * FROM camera_recording_segments;
CREATE TEMP TABLE manual_archive_files AS SELECT * FROM recording_files;
CREATE TEMP TABLE manual_archive_writers AS SELECT * FROM recording_writers;
DELETE FROM recording_writers;
DELETE FROM recording_files;
DELETE FROM camera_recording_segments;
DROP TABLE camera_recording_sessions;
ALTER TABLE camera_recording_sessions_new RENAME TO camera_recording_sessions;
UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM manual_archive_sequence),0))
WHERE name='camera_recording_sessions';
DROP TABLE manual_archive_sequence;

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_active
ON camera_recording_sessions(camera_id, status, deleted_at);

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_camera_created
ON camera_recording_sessions(camera_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_expires
ON camera_recording_sessions(expires_at, status, deleted_at);

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_event_group
ON camera_recording_sessions(event_group_id);

CREATE INDEX idx_camera_recording_sessions_pinned ON camera_recording_sessions(pinned_at);
INSERT INTO camera_recording_segments SELECT * FROM manual_archive_segments;
INSERT INTO recording_files SELECT * FROM manual_archive_files;
INSERT INTO recording_writers SELECT * FROM manual_archive_writers;
DROP TABLE manual_archive_segments;
DROP TABLE manual_archive_files;
DROP TABLE manual_archive_writers;
