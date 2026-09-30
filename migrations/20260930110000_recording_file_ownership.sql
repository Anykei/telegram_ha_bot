-- All owned paths are registered before writing; pending rows survive restart.
CREATE TABLE recording_files (
    path TEXT PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES camera_recording_sessions(id),
    segment_id INTEGER NOT NULL REFERENCES camera_recording_segments(id),
    size_bytes INTEGER NOT NULL DEFAULT 0,
    delete_pending INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    retry_at TEXT,
    last_error TEXT
);
CREATE INDEX recording_files_pending ON recording_files(delete_pending, retry_at);
CREATE TABLE recording_writers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL REFERENCES camera_recording_sessions(id),
    segment_id INTEGER NOT NULL REFERENCES camera_recording_segments(id),
    kind TEXT NOT NULL,
    UNIQUE(segment_id, kind)
);
INSERT INTO recording_files(path, session_id, segment_id, size_bytes)
SELECT file_path, session_id, id, COALESCE(size_bytes, 0)
FROM camera_recording_segments WHERE file_path IS NOT NULL;
