CREATE TABLE IF NOT EXISTS devices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id INTEGER NOT NULL,          -- Ссылка на нашу таблицу rooms.id
    entity_id TEXT NOT NULL UNIQUE,     -- HA ID (напр. 'light.kitchen_main')
    alias TEXT,                        -- Красивое имя
    device_class TEXT,                 -- Тип (light, switch, sensor) для иконок
    device_domain TEXT,
    FOREIGN KEY (room_id) REFERENCES rooms(id) ON DELETE CASCADE
);