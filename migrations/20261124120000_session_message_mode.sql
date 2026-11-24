ALTER TABLE users ADD COLUMN ui_message_mode TEXT
    CHECK (ui_message_mode IN ('photo', 'text', 'html'));
