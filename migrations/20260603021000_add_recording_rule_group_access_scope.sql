ALTER TABLE camera_recording_rule_groups
ADD COLUMN access_scope TEXT NOT NULL DEFAULT 'admin_only';
