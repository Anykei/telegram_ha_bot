-- Stores a read-only route and optional command feedback for its owner.
-- Device state and keyboards are rebuilt with current access checks on each page.
CREATE TABLE ui_text_pages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    context TEXT NOT NULL,
    UNIQUE(user_id, context)
);
