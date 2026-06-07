ALTER TABLE camera_recording_rules
ADD COLUMN active_time_enabled INTEGER NOT NULL DEFAULT 0;

ALTER TABLE camera_recording_rules
ADD COLUMN active_from_minute INTEGER;

ALTER TABLE camera_recording_rules
ADD COLUMN active_to_minute INTEGER;

ALTER TABLE camera_recording_rules
ADD COLUMN active_days_mask INTEGER NOT NULL DEFAULT 127;
