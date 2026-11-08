-- Revision changes only for execution-relevant edits, never for last_run_at.
DROP TRIGGER action_schedule_revision;
CREATE TRIGGER action_schedule_revision
AFTER UPDATE OF target_type, target_id, command, time_minute, days_mask, enabled ON action_schedules
WHEN NEW.target_type != OLD.target_type OR NEW.target_id != OLD.target_id
  OR NEW.command != OLD.command OR NEW.time_minute != OLD.time_minute
  OR NEW.days_mask != OLD.days_mask OR NEW.enabled != OLD.enabled
BEGIN
    UPDATE action_schedules SET revision = OLD.revision + 1 WHERE id = NEW.id;
END;

-- No cascading FK: deleting a schedule must retain its execution history.
CREATE TABLE action_schedule_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    schedule_id INTEGER NOT NULL,
    local_run_key TEXT NOT NULL,
    planned_at INTEGER NOT NULL,
    deadline_at INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    target_type TEXT NOT NULL,
    target_id INTEGER NOT NULL,
    command TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN
        ('pending', 'executing', 'sent', 'partial', 'rejected', 'unknown', 'skipped')),
    started_at INTEGER,
    finished_at INTEGER,
    result_json TEXT NOT NULL DEFAULT '[]',
    reason TEXT,
    UNIQUE(schedule_id, local_run_key),
    CHECK (deadline_at = planned_at + 60)
);
CREATE INDEX action_schedule_runs_pending ON action_schedule_runs(status, deadline_at, id);
