CREATE TABLE IF NOT EXISTS action_groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    access_scope TEXT NOT NULL DEFAULT 'admin_only',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK(access_scope IN ('admin_only', 'all_users'))
);

CREATE TABLE IF NOT EXISTS action_group_items (
    group_id INTEGER NOT NULL,
    device_id INTEGER NOT NULL,
    position INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(group_id, device_id),
    FOREIGN KEY(group_id) REFERENCES action_groups(id) ON DELETE CASCADE,
    FOREIGN KEY(device_id) REFERENCES devices(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_action_group_items_device
ON action_group_items(device_id);

CREATE INDEX IF NOT EXISTS idx_action_groups_access
ON action_groups(access_scope, enabled);

CREATE TABLE IF NOT EXISTS ha_native_targets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id TEXT NOT NULL UNIQUE,
    domain TEXT NOT NULL,
    ha_name TEXT NOT NULL,
    display_name TEXT,
    access_scope TEXT NOT NULL DEFAULT 'admin_only',
    enabled INTEGER NOT NULL DEFAULT 1,
    archived INTEGER NOT NULL DEFAULT 0,
    last_seen_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK(domain IN ('script', 'scene')),
    CHECK(access_scope IN ('admin_only', 'all_users'))
);

CREATE INDEX IF NOT EXISTS idx_ha_native_targets_access
ON ha_native_targets(access_scope, enabled, archived);

CREATE INDEX IF NOT EXISTS idx_ha_native_targets_entity
ON ha_native_targets(entity_id);

CREATE TABLE IF NOT EXISTS action_schedules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_type TEXT NOT NULL,
    target_id INTEGER NOT NULL,
    command TEXT NOT NULL,
    time_minute INTEGER NOT NULL,
    days_mask INTEGER NOT NULL DEFAULT 127,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_run_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK(target_type IN ('bot_group', 'ha_native')),
    CHECK(command IN ('turn_on', 'turn_off', 'execute')),
    CHECK(time_minute BETWEEN 0 AND 1439),
    CHECK(days_mask BETWEEN 1 AND 127)
);

CREATE INDEX IF NOT EXISTS idx_action_schedules_due
ON action_schedules(enabled, time_minute, days_mask);

CREATE INDEX IF NOT EXISTS idx_action_schedules_target
ON action_schedules(target_type, target_id);
