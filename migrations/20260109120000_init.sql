CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY,
    is_admin BOOLEAN DEFAULT 0,
    notify_enabled BOOLEAN DEFAULT 1,
    last_menu_id INTEGER DEFAULT -1
);

CREATE TABLE IF NOT EXISTS subscriptions (
    user_id INTEGER,
    entity_id TEXT,
    PRIMARY KEY (user_id, entity_id)
);

CREATE TABLE IF NOT EXISTS custom_names (
    entity_id TEXT PRIMARY KEY,
    custom_name TEXT NOT NULL
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

CREATE TABLE IF NOT EXISTS active_alerts (
    user_id INTEGER,
    entity_id TEXT,
    count INTEGER DEFAULT 1,
    last_state TEXT,
    last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (user_id, entity_id)
);