ALTER TABLE camera_health
ADD COLUMN consecutive_failures INTEGER NOT NULL DEFAULT 0;

ALTER TABLE camera_health
ADD COLUMN consecutive_successes INTEGER NOT NULL DEFAULT 0;

ALTER TABLE camera_health
ADD COLUMN health_state TEXT NOT NULL DEFAULT 'unknown';

ALTER TABLE camera_health
ADD COLUMN last_state_changed_at TEXT;

ALTER TABLE camera_health
ADD COLUMN degraded_at TEXT;

ALTER TABLE camera_health
ADD COLUMN recovered_at TEXT;

ALTER TABLE camera_health
ADD COLUMN degradation_alert_sent_at TEXT;

ALTER TABLE camera_health
ADD COLUMN recovery_alert_sent_at TEXT;

ALTER TABLE camera_health
ADD COLUMN last_failure_kind TEXT;

CREATE INDEX IF NOT EXISTS idx_camera_health_state
ON camera_health(health_state, updated_at);

CREATE TABLE IF NOT EXISTS camera_health_alert_delivery (
    camera_id INTEGER NOT NULL,
    alert_kind TEXT NOT NULL,
    recipient_id INTEGER NOT NULL,
    state_changed_at TEXT NOT NULL,
    sent_at TEXT,
    last_error TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(camera_id, alert_kind, recipient_id, state_changed_at),
    FOREIGN KEY(camera_id) REFERENCES cameras(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_camera_health_alert_delivery_pending
ON camera_health_alert_delivery(alert_kind, camera_id, state_changed_at, sent_at);

ALTER TABLE camera_recording_sessions
ADD COLUMN pinned_at TEXT;

ALTER TABLE camera_recording_sessions
ADD COLUMN pinned_by INTEGER;

ALTER TABLE camera_recording_sessions
ADD COLUMN pin_note TEXT;

ALTER TABLE camera_recording_sessions
ADD COLUMN pre_roll_from TEXT;

ALTER TABLE camera_recording_sessions
ADD COLUMN pre_roll_seconds INTEGER NOT NULL DEFAULT 0;

ALTER TABLE camera_recording_sessions
ADD COLUMN pre_roll_partial INTEGER NOT NULL DEFAULT 0;

ALTER TABLE camera_recording_sessions
ADD COLUMN pre_roll_warning TEXT;

CREATE INDEX IF NOT EXISTS idx_camera_recording_sessions_pinned
ON camera_recording_sessions(pinned_at);

ALTER TABLE camera_recording_rules
ADD COLUMN pre_roll_enabled INTEGER NOT NULL DEFAULT 0;

ALTER TABLE camera_recording_rules
ADD COLUMN pre_roll_seconds INTEGER NOT NULL DEFAULT 15;

CREATE TABLE IF NOT EXISTS user_notification_schedule (
    user_id INTEGER PRIMARY KEY,
    quiet_enabled INTEGER NOT NULL DEFAULT 0,
    quiet_from TEXT,
    quiet_to TEXT,
    timezone TEXT,
    critical_only INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
);
