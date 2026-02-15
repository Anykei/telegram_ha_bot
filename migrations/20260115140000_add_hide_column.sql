-- Add hide column to hidden_entities table to control visibility
ALTER TABLE hidden_entities ADD COLUMN hide INTEGER NOT NULL DEFAULT 1;
