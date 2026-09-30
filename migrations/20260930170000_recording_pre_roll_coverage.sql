-- Keep the merged request independent from coverage and completed attempts.
ALTER TABLE camera_recording_sessions ADD COLUMN pre_roll_attempted_seconds INTEGER NOT NULL DEFAULT 0;
ALTER TABLE camera_recording_sessions ADD COLUMN pre_roll_actual_seconds INTEGER NOT NULL DEFAULT 0;

UPDATE camera_recording_sessions
SET pre_roll_actual_seconds = CASE WHEN pre_roll_from IS NOT NULL THEN
        MAX(0, MIN(pre_roll_seconds,
            CAST(ROUND((julianday(first_event_at) - julianday(pre_roll_from)) * 86400, 3) AS INTEGER)))
        ELSE 0 END,
    pre_roll_attempted_seconds = CASE
        WHEN pre_roll_from IS NOT NULL OR pre_roll_warning IS NOT NULL
          OR status NOT IN ('queued', 'recording') THEN pre_roll_seconds
        ELSE 0 END;
