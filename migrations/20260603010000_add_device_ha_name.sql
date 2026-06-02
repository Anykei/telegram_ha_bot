ALTER TABLE devices ADD COLUMN ha_name TEXT;

UPDATE devices
SET ha_name = alias
WHERE ha_name IS NULL;
