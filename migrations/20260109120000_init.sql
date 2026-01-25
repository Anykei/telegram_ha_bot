CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    is_admin BOOLEAN DEFAULT 0,
    notify_enabled BOOLEAN DEFAULT 1,
    last_menu_id INTEGER DEFAULT -1,
    current_context TEXT DEFAULT ""
);

CREATE TABLE IF NOT EXISTS subscriptions (
    user_id INTEGER,
    entity_id TEXT,
    PRIMARY KEY (user_id, entity_id)
);

CREATE TABLE IF NOT EXISTS aliases (
    entity_id TEXT PRIMARY KEY,
    human_name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS state_aliases (
    entity_id TEXT,
    original_state TEXT,
    human_state TEXT,
    PRIMARY KEY (entity_id, original_state)
);

CREATE TABLE IF NOT EXISTS hidden_entities (
    entity_id TEXT PRIMARY KEY
);

CREATE TABLE device_event_log (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      entity_id TEXT NOT NULL,
      state TEXT NOT NULL,
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_global_log_time ON device_event_log(created_at);
CREATE INDEX idx_global_log_entity ON device_event_log(entity_id);

CREATE TABLE IF NOT EXISTS pinned_headers (
      user_id INTEGER,
      entity_id TEXT,
      PRIMARY KEY (user_id, entity_id)
);