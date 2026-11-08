CREATE TABLE user_notification_access (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL,
    can_notify INTEGER NOT NULL DEFAULT 1 CHECK(can_notify IN (0,1)),
    PRIMARY KEY(user_id, entity_id)
);
INSERT INTO user_notification_access (user_id, entity_id, can_notify)
SELECT user_id, entity_id, COALESCE(can_notify, 1) FROM user_device_access;
CREATE TABLE legacy_device_acl_review (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL,
    PRIMARY KEY(user_id, entity_id)
);
INSERT INTO legacy_device_acl_review SELECT user_id, entity_id FROM user_device_access;

CREATE VIEW legacy_device_acl_report AS
SELECT review.user_id, review.entity_id, d.id AS device_id, d.room_id,
       uda.can_view AS explicit_view, uda.can_control AS explicit_control,
       COALESCE(ura.can_view, CASE WHEN COALESCE(up.role, 'user') IN ('user', 'admin') THEN 1 ELSE 0 END) AS inherited_view,
       COALESCE(ura.can_control, CASE WHEN COALESCE(up.role, 'user') IN ('user', 'admin') THEN 1 ELSE 0 END) AS inherited_control,
       CASE WHEN d.archived = 0 AND r.hide = 0 THEN COALESCE(uda.can_view, ura.can_view, CASE WHEN COALESCE(up.role, 'user') IN ('user', 'admin') THEN 1 ELSE 0 END) ELSE 0 END AS effective_view,
       CASE WHEN d.archived = 0 AND r.hide = 0 AND COALESCE(uda.can_view, ura.can_view, CASE WHEN COALESCE(up.role, 'user') IN ('user', 'admin') THEN 1 ELSE 0 END) != 0 THEN COALESCE(uda.can_control, ura.can_control, CASE WHEN COALESCE(up.role, 'user') IN ('user', 'admin') THEN 1 ELSE 0 END) ELSE 0 END AS effective_control,
       COALESCE(una.can_notify, 1) AS can_notify
FROM legacy_device_acl_review review
JOIN user_device_access uda ON uda.user_id = review.user_id AND uda.entity_id = review.entity_id
LEFT JOIN devices d ON d.entity_id = review.entity_id
LEFT JOIN rooms r ON r.id = d.room_id
LEFT JOIN user_room_access ura ON ura.user_id = review.user_id AND ura.room_id = d.room_id
LEFT JOIN user_profiles up ON up.user_id = review.user_id
LEFT JOIN user_notification_access una ON una.user_id = review.user_id AND una.entity_id = review.entity_id;
