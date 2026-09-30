CREATE TABLE user_favorites (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner INTEGER NOT NULL,
    action TEXT NOT NULL,
    target_key TEXT NOT NULL,
    position INTEGER NOT NULL,
    UNIQUE(owner, target_key)
);
CREATE INDEX user_favorites_owner_position ON user_favorites(owner, position, id);
CREATE TABLE user_display_metadata (
    user_id INTEGER PRIMARY KEY,
    display_name TEXT,
    username TEXT,
    admin_alias TEXT
);
