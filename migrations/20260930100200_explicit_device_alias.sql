ALTER TABLE devices ADD COLUMN alias_is_manual INTEGER NOT NULL DEFAULT 0;
UPDATE devices SET alias_is_manual = 1
WHERE alias IS NOT NULL AND TRIM(alias) != '' AND alias != entity_id
AND (ha_name IS NULL OR alias != ha_name);
