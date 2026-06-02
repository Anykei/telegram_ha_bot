use crate::options::VoiceCommandEngine;
use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessRule {
    pub can_view: bool,
    pub can_control: bool,
}

impl AccessRule {
    fn from_row(can_view: i64, can_control: i64) -> Self {
        Self {
            can_view: can_view != 0,
            can_control: can_control != 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccessSummary {
    pub rooms_full: i64,
    pub rooms_view: i64,
    pub rooms_hidden: i64,
    pub devices_full: i64,
    pub devices_view: i64,
    pub devices_hidden: i64,
    pub notifications_disabled: i64,
}

pub async fn get_user_role(user_id: u64, pool: &SqlitePool) -> Result<String> {
    let role = sqlx::query_scalar::<_, String>("SELECT role FROM user_profiles WHERE user_id = ?")
        .bind(user_id as i64)
        .fetch_optional(pool)
        .await?;

    Ok(role.unwrap_or_else(|| "user".to_string()))
}

pub async fn can_use_voice(user_id: u64, is_admin: bool, pool: &SqlitePool) -> Result<bool> {
    if is_admin {
        return Ok(true);
    }

    let value: Option<i64> =
        sqlx::query_scalar("SELECT can_use_voice FROM user_profiles WHERE user_id = ?")
            .bind(user_id as i64)
            .fetch_optional(pool)
            .await?;

    Ok(value.unwrap_or(1) != 0)
}

pub async fn toggle_user_voice_access(user_id: u64, pool: &SqlitePool) -> Result<bool> {
    sqlx::query(
        r#"
        INSERT INTO user_profiles (user_id, can_use_voice)
        VALUES (?, 0)
        ON CONFLICT(user_id) DO UPDATE SET
            can_use_voice = CASE WHEN COALESCE(can_use_voice, 1) = 0 THEN 1 ELSE 0 END
        "#,
    )
    .bind(user_id as i64)
    .execute(pool)
    .await?;

    let value: i64 = sqlx::query_scalar(
        "SELECT COALESCE(can_use_voice, 1) FROM user_profiles WHERE user_id = ?",
    )
    .bind(user_id as i64)
    .fetch_one(pool)
    .await?;

    Ok(value != 0)
}

pub async fn get_user_voice_command_engine(
    user_id: u64,
    default_engine: VoiceCommandEngine,
    pool: &SqlitePool,
) -> Result<VoiceCommandEngine> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT voice_command_engine FROM user_profiles WHERE user_id = ?")
            .bind(user_id as i64)
            .fetch_optional(pool)
            .await?;

    Ok(value
        .as_deref()
        .map(VoiceCommandEngine::from_db)
        .unwrap_or(default_engine))
}

pub async fn cycle_user_voice_command_engine(
    user_id: u64,
    default_engine: VoiceCommandEngine,
    pool: &SqlitePool,
) -> Result<VoiceCommandEngine> {
    let current = get_user_voice_command_engine(user_id, default_engine, pool).await?;
    let next = current.next_for_profile();

    sqlx::query(
        r#"
        INSERT INTO user_profiles (user_id, voice_command_engine)
        VALUES (?, ?)
        ON CONFLICT(user_id) DO UPDATE SET
            voice_command_engine = excluded.voice_command_engine
        "#,
    )
    .bind(user_id as i64)
    .bind(next.as_str())
    .execute(pool)
    .await?;

    Ok(next)
}

pub async fn set_user_role(user_id: u64, role: &str, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO user_profiles (user_id, role)
        VALUES (?, ?)
        ON CONFLICT(user_id) DO UPDATE SET role = excluded.role
        "#,
    )
    .bind(user_id as i64)
    .bind(role)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn cycle_user_role(user_id: u64, pool: &SqlitePool) -> Result<String> {
    let current = get_user_role(user_id, pool).await?;
    let next = match current.as_str() {
        "user" => "child",
        "child" => "guest",
        _ => "user",
    };

    set_user_role(user_id, next, pool).await?;

    if matches!(next, "child" | "guest") {
        deny_all_rooms(user_id, pool).await?;
    } else {
        clear_user_access_overrides(user_id, pool).await?;
    }

    Ok(next.to_string())
}

pub async fn deny_all_rooms(user_id: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO user_room_access (user_id, room_id, can_view, can_control)
        SELECT ?, id, 0, 0 FROM rooms
        WHERE true
        ON CONFLICT(user_id, room_id) DO UPDATE SET
            can_view = excluded.can_view,
            can_control = excluded.can_control
        "#,
    )
    .bind(user_id as i64)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn clear_user_access_overrides(user_id: u64, pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM user_device_access WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM user_room_access WHERE user_id = ?")
        .bind(user_id as i64)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn reset_user_access(user_id: u64, pool: &SqlitePool) -> Result<()> {
    set_user_role(user_id, "user", pool).await?;
    clear_user_access_overrides(user_id, pool).await
}

pub async fn get_access_summary(
    user_id: u64,
    is_admin: bool,
    pool: &SqlitePool,
) -> Result<AccessSummary> {
    if is_admin {
        let rooms_full = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM rooms WHERE hide = 0")
            .fetch_one(pool)
            .await?;
        let devices_full = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)
            FROM devices d
            JOIN rooms r ON r.id = d.room_id
            WHERE d.archived = 0 AND r.hide = 0
            "#,
        )
        .fetch_one(pool)
        .await?;

        return Ok(AccessSummary {
            rooms_full,
            devices_full,
            ..Default::default()
        });
    }

    let (rooms_full, rooms_view, rooms_hidden) = sqlx::query_as::<_, (i64, i64, i64)>(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN COALESCE(ura.can_view, 1) != 0
                AND COALESCE(ura.can_control, 1) != 0 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN COALESCE(ura.can_view, 1) != 0
                AND COALESCE(ura.can_control, 1) = 0 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN COALESCE(ura.can_view, 1) = 0 THEN 1 ELSE 0 END), 0)
        FROM rooms r
        LEFT JOIN user_room_access ura ON ura.room_id = r.id AND ura.user_id = ?
        WHERE r.hide = 0
        "#,
    )
    .bind(user_id as i64)
    .fetch_one(pool)
    .await?;

    let (devices_full, devices_view, devices_hidden) = sqlx::query_as::<_, (i64, i64, i64)>(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN COALESCE(uda.can_view, ura.can_view, 1) != 0
                AND COALESCE(uda.can_control, ura.can_control, 1) != 0 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN COALESCE(uda.can_view, ura.can_view, 1) != 0
                AND COALESCE(uda.can_control, ura.can_control, 1) = 0 THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN COALESCE(uda.can_view, ura.can_view, 1) = 0 THEN 1 ELSE 0 END), 0)
        FROM devices d
        JOIN rooms r ON r.id = d.room_id
        LEFT JOIN user_room_access ura ON ura.room_id = d.room_id AND ura.user_id = ?
        LEFT JOIN user_device_access uda ON uda.entity_id = d.entity_id AND uda.user_id = ?
        WHERE d.archived = 0 AND r.hide = 0
        "#,
    )
    .bind(user_id as i64)
    .bind(user_id as i64)
    .fetch_one(pool)
    .await?;

    let notifications_disabled = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM devices d
        JOIN rooms r ON r.id = d.room_id
        LEFT JOIN user_room_access ura ON ura.room_id = d.room_id AND ura.user_id = ?
        LEFT JOIN user_device_access uda ON uda.entity_id = d.entity_id AND uda.user_id = ?
        WHERE d.archived = 0
            AND r.hide = 0
            AND COALESCE(uda.can_view, ura.can_view, 1) != 0
            AND COALESCE(uda.can_notify, 1) = 0
        "#,
    )
    .bind(user_id as i64)
    .bind(user_id as i64)
    .fetch_one(pool)
    .await?;

    Ok(AccessSummary {
        rooms_full,
        rooms_view,
        rooms_hidden,
        devices_full,
        devices_view,
        devices_hidden,
        notifications_disabled,
    })
}

#[allow(dead_code)]
pub async fn set_room_access(
    user_id: u64,
    room_id: i64,
    can_view: bool,
    can_control: bool,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO user_room_access (user_id, room_id, can_view, can_control)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(user_id, room_id) DO UPDATE SET
            can_view = excluded.can_view,
            can_control = excluded.can_control
        "#,
    )
    .bind(user_id as i64)
    .bind(room_id)
    .bind(can_view as i64)
    .bind(can_control as i64)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_room_access(user_id: u64, room_id: i64, pool: &SqlitePool) -> Result<AccessRule> {
    let row = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT COALESCE(can_view, 1), COALESCE(can_control, 1)
        FROM user_room_access
        WHERE user_id = ? AND room_id = ?
        "#,
    )
    .bind(user_id as i64)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;

    Ok(row
        .map(|(can_view, can_control)| AccessRule::from_row(can_view, can_control))
        .unwrap_or(AccessRule {
            can_view: true,
            can_control: true,
        }))
}

pub async fn toggle_room_view_access(
    user_id: u64,
    room_id: i64,
    pool: &SqlitePool,
) -> Result<AccessRule> {
    let current = get_room_access(user_id, room_id, pool).await?;
    let next = match (current.can_view, current.can_control) {
        (true, true) => AccessRule {
            can_view: true,
            can_control: false,
        },
        (true, false) => AccessRule {
            can_view: false,
            can_control: false,
        },
        (false, _) => AccessRule {
            can_view: true,
            can_control: true,
        },
    };

    set_room_access(user_id, room_id, next.can_view, next.can_control, pool).await?;

    Ok(next)
}

#[allow(dead_code)]
pub async fn set_device_access(
    user_id: u64,
    entity_id: &str,
    can_view: bool,
    can_control: bool,
    can_notify: bool,
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO user_device_access (user_id, entity_id, can_view, can_control, can_notify)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(user_id, entity_id) DO UPDATE SET
            can_view = excluded.can_view,
            can_control = excluded.can_control,
            can_notify = excluded.can_notify
        "#,
    )
    .bind(user_id as i64)
    .bind(entity_id)
    .bind(can_view as i64)
    .bind(can_control as i64)
    .bind(can_notify as i64)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_device_access(
    user_id: u64,
    entity_id: &str,
    pool: &SqlitePool,
) -> Result<AccessRule> {
    let row = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT COALESCE(can_view, 1), COALESCE(can_control, 1)
        FROM user_device_access
        WHERE user_id = ? AND entity_id = ?
        "#,
    )
    .bind(user_id as i64)
    .bind(entity_id)
    .fetch_optional(pool)
    .await?;

    Ok(row
        .map(|(can_view, can_control)| AccessRule::from_row(can_view, can_control))
        .unwrap_or(AccessRule {
            can_view: true,
            can_control: true,
        }))
}

pub async fn get_device_notify_access(
    user_id: u64,
    entity_id: &str,
    pool: &SqlitePool,
) -> Result<bool> {
    let can_notify = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COALESCE(can_notify, 1)
        FROM user_device_access
        WHERE user_id = ? AND entity_id = ?
        "#,
    )
    .bind(user_id as i64)
    .bind(entity_id)
    .fetch_optional(pool)
    .await?;

    Ok(can_notify != Some(0))
}

pub async fn toggle_device_access_mode(
    user_id: u64,
    entity_id: &str,
    pool: &SqlitePool,
) -> Result<AccessRule> {
    let current = get_device_access(user_id, entity_id, pool).await?;
    let next = match (current.can_view, current.can_control) {
        (true, true) => AccessRule {
            can_view: true,
            can_control: false,
        },
        (true, false) => AccessRule {
            can_view: false,
            can_control: false,
        },
        (false, _) => AccessRule {
            can_view: true,
            can_control: true,
        },
    };

    set_device_access(
        user_id,
        entity_id,
        next.can_view,
        next.can_control,
        true,
        pool,
    )
    .await?;

    Ok(next)
}

pub async fn toggle_device_notify_access(
    user_id: u64,
    entity_id: &str,
    pool: &SqlitePool,
) -> Result<bool> {
    let next_notify = !get_device_notify_access(user_id, entity_id, pool).await?;

    sqlx::query(
        r#"
        INSERT INTO user_device_access (user_id, entity_id, can_view, can_control, can_notify)
        SELECT
            ?,
            d.entity_id,
            COALESCE(ura.can_view, 1),
            COALESCE(ura.can_control, 1),
            ?
        FROM devices d
        LEFT JOIN user_room_access ura ON ura.room_id = d.room_id AND ura.user_id = ?
        WHERE d.entity_id = ? AND d.archived = 0
        ON CONFLICT(user_id, entity_id) DO UPDATE SET
            can_notify = excluded.can_notify
        "#,
    )
    .bind(user_id as i64)
    .bind(next_notify as i64)
    .bind(user_id as i64)
    .bind(entity_id)
    .execute(pool)
    .await?;

    Ok(next_notify)
}

pub async fn can_view_room(
    user_id: u64,
    is_admin: bool,
    room_id: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    if is_admin {
        return Ok(true);
    }

    let can_view = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COALESCE(ura.can_view, 1)
        FROM rooms r
        LEFT JOIN user_room_access ura ON ura.room_id = r.id AND ura.user_id = ?
        WHERE r.id = ? AND r.hide = 0
        "#,
    )
    .bind(user_id as i64)
    .bind(room_id)
    .fetch_optional(pool)
    .await?;

    Ok(can_view.is_some_and(|value| value != 0))
}

pub async fn can_view_device(
    user_id: u64,
    is_admin: bool,
    device_id: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    let rule = get_effective_device_rule(user_id, is_admin, device_id, pool).await?;
    Ok(rule.can_view)
}

pub async fn get_effective_device_access(
    user_id: u64,
    is_admin: bool,
    device_id: i64,
    pool: &SqlitePool,
) -> Result<AccessRule> {
    get_effective_device_rule(user_id, is_admin, device_id, pool).await
}

pub async fn can_control_device(
    user_id: u64,
    is_admin: bool,
    device_id: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    let rule = get_effective_device_rule(user_id, is_admin, device_id, pool).await?;
    Ok(rule.can_view && rule.can_control)
}

pub async fn can_notify_entity(user_id: u64, entity_id: &str, pool: &SqlitePool) -> Result<bool> {
    let can_notify = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT
            CASE
                WHEN COALESCE(uda.can_view, ura.can_view, 1) = 0 THEN 0
                ELSE COALESCE(uda.can_notify, 1)
            END
        FROM devices d
        LEFT JOIN user_room_access ura ON ura.room_id = d.room_id AND ura.user_id = ?
        LEFT JOIN user_device_access uda ON uda.entity_id = d.entity_id AND uda.user_id = ?
        WHERE d.entity_id = ? AND d.archived = 0
        "#,
    )
    .bind(user_id as i64)
    .bind(user_id as i64)
    .bind(entity_id)
    .fetch_optional(pool)
    .await?;

    Ok(can_notify != Some(0))
}

pub async fn can_notify_entity_for_device_id(
    user_id: u64,
    is_admin: bool,
    device_id: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    if is_admin {
        return Ok(true);
    }

    let entity_id = sqlx::query_scalar::<_, String>(
        "SELECT entity_id FROM devices WHERE id = ? AND archived = 0",
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    if let Some(entity_id) = entity_id {
        can_notify_entity(user_id, &entity_id, pool).await
    } else {
        Ok(false)
    }
}

async fn get_effective_device_rule(
    user_id: u64,
    is_admin: bool,
    device_id: i64,
    pool: &SqlitePool,
) -> Result<AccessRule> {
    if is_admin {
        return Ok(AccessRule {
            can_view: true,
            can_control: true,
        });
    }

    let row = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT
            COALESCE(uda.can_view, ura.can_view, 1) AS can_view,
            COALESCE(uda.can_control, ura.can_control, 1) AS can_control
        FROM devices d
        JOIN rooms r ON r.id = d.room_id
        LEFT JOIN user_room_access ura ON ura.room_id = d.room_id AND ura.user_id = ?
        LEFT JOIN user_device_access uda ON uda.entity_id = d.entity_id AND uda.user_id = ?
        WHERE d.id = ? AND d.archived = 0 AND r.hide = 0
        "#,
    )
    .bind(user_id as i64)
    .bind(user_id as i64)
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    Ok(row
        .map(|(can_view, can_control)| AccessRule::from_row(can_view, can_control))
        .unwrap_or(AccessRule {
            can_view: false,
            can_control: false,
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Result<SqlitePool> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;

        sqlx::query("CREATE TABLE users (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await?;
        sqlx::query("CREATE TABLE rooms (id INTEGER PRIMARY KEY, hide INTEGER NOT NULL DEFAULT 0)")
            .execute(&pool)
            .await?;
        sqlx::query(
            r#"
            CREATE TABLE devices (
                id INTEGER PRIMARY KEY,
                room_id INTEGER NOT NULL,
                entity_id TEXT NOT NULL,
                archived INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE user_profiles (
                user_id INTEGER PRIMARY KEY,
                role TEXT NOT NULL DEFAULT 'user',
                can_use_voice INTEGER NOT NULL DEFAULT 1,
                voice_command_engine TEXT NOT NULL DEFAULT 'local_parser'
            )
            "#,
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE TABLE user_room_access (user_id INTEGER, room_id INTEGER, can_view INTEGER, can_control INTEGER, PRIMARY KEY (user_id, room_id))",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "CREATE TABLE user_device_access (user_id INTEGER, entity_id TEXT, can_view INTEGER, can_control INTEGER, can_notify INTEGER, PRIMARY KEY (user_id, entity_id))",
        )
        .execute(&pool)
        .await?;

        sqlx::query("INSERT INTO rooms (id, hide) VALUES (1, 0)")
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO devices (id, room_id, entity_id, archived) VALUES (1, 1, 'switch.test', 0)",
        )
        .execute(&pool)
        .await?;

        Ok(pool)
    }

    #[tokio::test]
    async fn missing_rules_allow_view_and_control_by_default() -> Result<()> {
        let pool = test_pool().await?;

        assert!(can_view_room(10, false, 1, &pool).await?);
        assert!(can_view_device(10, false, 1, &pool).await?);
        assert!(can_control_device(10, false, 1, &pool).await?);

        Ok(())
    }

    #[tokio::test]
    async fn device_rule_can_deny_control_without_hiding_device() -> Result<()> {
        let pool = test_pool().await?;
        set_device_access(10, "switch.test", true, false, true, &pool).await?;

        assert!(can_view_device(10, false, 1, &pool).await?);
        assert!(!can_control_device(10, false, 1, &pool).await?);

        Ok(())
    }

    #[tokio::test]
    async fn room_rule_hides_devices_by_default() -> Result<()> {
        let pool = test_pool().await?;
        set_room_access(10, 1, false, false, &pool).await?;

        assert!(!can_view_room(10, false, 1, &pool).await?);
        assert!(!can_view_device(10, false, 1, &pool).await?);
        assert!(!can_notify_entity(10, "switch.test", &pool).await?);

        Ok(())
    }

    #[tokio::test]
    async fn room_access_toggle_cycles_full_view_deny() -> Result<()> {
        let pool = test_pool().await?;

        let view_only = toggle_room_view_access(10, 1, &pool).await?;
        assert_eq!(
            view_only,
            AccessRule {
                can_view: true,
                can_control: false
            }
        );

        let denied = toggle_room_view_access(10, 1, &pool).await?;
        assert_eq!(
            denied,
            AccessRule {
                can_view: false,
                can_control: false
            }
        );

        let full = toggle_room_view_access(10, 1, &pool).await?;
        assert_eq!(
            full,
            AccessRule {
                can_view: true,
                can_control: true
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn device_access_toggle_cycles_full_view_deny() -> Result<()> {
        let pool = test_pool().await?;

        let view_only = toggle_device_access_mode(10, "switch.test", &pool).await?;
        assert_eq!(
            view_only,
            AccessRule {
                can_view: true,
                can_control: false
            }
        );

        let denied = toggle_device_access_mode(10, "switch.test", &pool).await?;
        assert_eq!(
            denied,
            AccessRule {
                can_view: false,
                can_control: false
            }
        );

        let full = toggle_device_access_mode(10, "switch.test", &pool).await?;
        assert_eq!(
            full,
            AccessRule {
                can_view: true,
                can_control: true
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn device_notify_toggle_preserves_view_and_control_access() -> Result<()> {
        let pool = test_pool().await?;
        set_device_access(10, "switch.test", true, false, true, &pool).await?;

        let notify_enabled = toggle_device_notify_access(10, "switch.test", &pool).await?;
        let access = get_device_access(10, "switch.test", &pool).await?;

        assert!(!notify_enabled);
        assert_eq!(
            access,
            AccessRule {
                can_view: true,
                can_control: false
            }
        );
        assert!(!can_notify_entity(10, "switch.test", &pool).await?);

        Ok(())
    }

    #[tokio::test]
    async fn notify_toggle_does_not_make_room_denied_device_visible() -> Result<()> {
        let pool = test_pool().await?;
        set_room_access(10, 1, false, false, &pool).await?;

        let notify_enabled = toggle_device_notify_access(10, "switch.test", &pool).await?;
        let effective = get_effective_device_access(10, false, 1, &pool).await?;

        assert!(!notify_enabled);
        assert_eq!(
            effective,
            AccessRule {
                can_view: false,
                can_control: false
            }
        );

        Ok(())
    }

    #[tokio::test]
    async fn role_cycle_applies_safe_defaults_and_clears_overrides() -> Result<()> {
        let pool = test_pool().await?;

        let child = cycle_user_role(10, &pool).await?;
        assert_eq!(child, "child");
        assert!(!can_view_room(10, false, 1, &pool).await?);

        let guest = cycle_user_role(10, &pool).await?;
        assert_eq!(guest, "guest");
        assert!(!can_view_room(10, false, 1, &pool).await?);

        let user = cycle_user_role(10, &pool).await?;
        assert_eq!(user, "user");
        assert!(can_view_room(10, false, 1, &pool).await?);

        Ok(())
    }

    #[tokio::test]
    async fn access_summary_counts_effective_modes() -> Result<()> {
        let pool = test_pool().await?;
        set_room_access(10, 1, true, false, &pool).await?;
        set_device_access(10, "switch.test", true, false, false, &pool).await?;

        let summary = get_access_summary(10, false, &pool).await?;

        assert_eq!(summary.rooms_full, 0);
        assert_eq!(summary.rooms_view, 1);
        assert_eq!(summary.rooms_hidden, 0);
        assert_eq!(summary.devices_full, 0);
        assert_eq!(summary.devices_view, 1);
        assert_eq!(summary.devices_hidden, 0);
        assert_eq!(summary.notifications_disabled, 1);

        Ok(())
    }

    #[tokio::test]
    async fn reset_user_access_restores_default_profile() -> Result<()> {
        let pool = test_pool().await?;
        set_user_role(10, "guest", &pool).await?;
        set_room_access(10, 1, false, false, &pool).await?;
        set_device_access(10, "switch.test", false, false, false, &pool).await?;

        reset_user_access(10, &pool).await?;

        assert_eq!(get_user_role(10, &pool).await?, "user");
        assert!(can_view_room(10, false, 1, &pool).await?);
        assert!(can_control_device(10, false, 1, &pool).await?);
        assert!(can_notify_entity(10, "switch.test", &pool).await?);

        Ok(())
    }

    #[tokio::test]
    async fn voice_access_defaults_to_enabled_and_can_toggle() -> Result<()> {
        let pool = test_pool().await?;

        assert!(can_use_voice(10, false, &pool).await?);

        let disabled = toggle_user_voice_access(10, &pool).await?;
        assert!(!disabled);
        assert!(!can_use_voice(10, false, &pool).await?);

        let enabled = toggle_user_voice_access(10, &pool).await?;
        assert!(enabled);
        assert!(can_use_voice(10, false, &pool).await?);

        assert!(can_use_voice(10, true, &pool).await?);

        Ok(())
    }

    #[tokio::test]
    async fn user_voice_engine_can_cycle_per_profile() -> Result<()> {
        let pool = test_pool().await?;

        assert_eq!(
            get_user_voice_command_engine(10, VoiceCommandEngine::LocalParser, &pool).await?,
            VoiceCommandEngine::LocalParser
        );

        assert_eq!(
            cycle_user_voice_command_engine(10, VoiceCommandEngine::LocalParser, &pool).await?,
            VoiceCommandEngine::HaConversationReadonly
        );
        assert_eq!(
            cycle_user_voice_command_engine(10, VoiceCommandEngine::LocalParser, &pool).await?,
            VoiceCommandEngine::HaConversationFull
        );
        assert_eq!(
            cycle_user_voice_command_engine(10, VoiceCommandEngine::LocalParser, &pool).await?,
            VoiceCommandEngine::LocalParser
        );

        Ok(())
    }
}
