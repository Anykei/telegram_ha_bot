ALTER TABLE action_schedules ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
CREATE TRIGGER action_schedule_revision AFTER UPDATE OF command,time_minute,days_mask,enabled ON action_schedules
BEGIN
    UPDATE action_schedules SET revision = OLD.revision + 1 WHERE id = NEW.id;
END;
