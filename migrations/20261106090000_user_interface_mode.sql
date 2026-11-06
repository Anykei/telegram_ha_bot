ALTER TABLE users ADD COLUMN interface_mode TEXT NOT NULL DEFAULT 'classic'
    CHECK (interface_mode IN ('classic', 'html'));
