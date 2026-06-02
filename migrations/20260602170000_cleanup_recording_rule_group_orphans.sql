DELETE FROM camera_recording_rule_group_items
WHERE NOT EXISTS (
    SELECT 1
    FROM camera_recording_rule_groups g
    WHERE g.id = camera_recording_rule_group_items.group_id
)
OR NOT EXISTS (
    SELECT 1
    FROM camera_recording_rules r
    WHERE r.id = camera_recording_rule_group_items.rule_id
);
