CREATE TABLE IF NOT EXISTS user_profiles (
    user_id INTEGER PRIMARY KEY,
    role TEXT NOT NULL DEFAULT 'user',
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS user_room_access (
    user_id INTEGER NOT NULL,
    room_id INTEGER NOT NULL,
    can_view INTEGER NOT NULL DEFAULT 1,
    can_control INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, room_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (room_id) REFERENCES rooms(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS user_device_access (
    user_id INTEGER NOT NULL,
    entity_id TEXT NOT NULL,
    can_view INTEGER NOT NULL DEFAULT 1,
    can_control INTEGER NOT NULL DEFAULT 1,
    can_notify INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, entity_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
