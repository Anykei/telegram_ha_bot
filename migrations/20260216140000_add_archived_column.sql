-- Add archived column to devices table to mark devices that no longer exist in Home Assistant
ALTER TABLE devices ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
