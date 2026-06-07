use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingRuleGroup {
    pub id: i64,
    pub name: String,
    pub enabled: i64,
    pub access_scope: String,
    pub rules_count: i64,
    pub created_at: DateTime<Utc>,
}

impl RecordingRuleGroup {
    pub fn is_enabled(&self) -> bool {
        self.enabled != 0
    }

    pub fn is_visible_to_all_users(&self) -> bool {
        self.access_scope == "all_users"
    }

    pub fn access_scope_label(&self) -> &'static str {
        if self.is_visible_to_all_users() {
            "доступно всем"
        } else {
            "только администратору"
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingRuleGroupRule {
    pub rule_id: i64,
    pub rule_name: String,
    pub rule_enabled: i64,
    pub camera_id: i64,
    pub camera_name: String,
    pub room_id: Option<i64>,
    pub room_area: Option<String>,
    pub room_alias: Option<String>,
    pub selected: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingRuleGroupCamera {
    pub camera_id: i64,
    pub camera_name: String,
    pub room_id: Option<i64>,
    pub room_area: Option<String>,
    pub room_alias: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct RecordingRuleGroupRoom {
    pub room_id: Option<i64>,
    pub room_area: Option<String>,
    pub room_alias: Option<String>,
}

impl RecordingRuleGroupRule {
    pub fn is_rule_enabled(&self) -> bool {
        self.rule_enabled != 0
    }

    pub fn is_selected(&self) -> bool {
        self.selected != 0
    }
}

pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<RecordingRuleGroup>> {
    Ok(sqlx::query_as::<_, RecordingRuleGroup>(
        r#"
        SELECT g.id, g.name, g.enabled, g.access_scope, g.created_at, COUNT(r.id) AS rules_count
        FROM camera_recording_rule_groups g
        LEFT JOIN camera_recording_rule_group_items i ON i.group_id = g.id
        LEFT JOIN camera_recording_rules r ON r.id = i.rule_id AND r.deleted_at IS NULL
        GROUP BY g.id, g.name, g.enabled, g.access_scope, g.created_at
        ORDER BY g.name
        "#,
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_group(group_id: i64, pool: &SqlitePool) -> Result<Option<RecordingRuleGroup>> {
    Ok(sqlx::query_as::<_, RecordingRuleGroup>(
        r#"
        SELECT g.id, g.name, g.enabled, g.access_scope, g.created_at, COUNT(r.id) AS rules_count
        FROM camera_recording_rule_groups g
        LEFT JOIN camera_recording_rule_group_items i ON i.group_id = g.id
        LEFT JOIN camera_recording_rules r ON r.id = i.rule_id AND r.deleted_at IS NULL
        WHERE g.id = ?
        GROUP BY g.id, g.name, g.enabled, g.access_scope, g.created_at
        "#,
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn create_group(name: &str, pool: &SqlitePool) -> Result<i64> {
    create_group_with_enabled(name, true, pool).await
}

async fn create_group_with_enabled(name: &str, enabled: bool, pool: &SqlitePool) -> Result<i64> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Group name is empty"));
    }

    sqlx::query(
        r#"
        INSERT INTO camera_recording_rule_groups (name, enabled, updated_at)
        VALUES (?, ?, CURRENT_TIMESTAMP)
        ON CONFLICT(name) DO UPDATE SET updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(trimmed)
    .bind(if enabled { 1 } else { 0 })
    .execute(pool)
    .await?;

    Ok(
        sqlx::query_scalar("SELECT id FROM camera_recording_rule_groups WHERE name = ?")
            .bind(trimmed)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn ensure_default_groups(pool: &SqlitePool) -> Result<()> {
    for (name, enabled) in [
        ("Охрана", true),
        ("Тест", false),
        ("Ночь", true),
        ("Двери", true),
    ] {
        create_group_with_enabled(name, enabled, pool).await?;
    }
    Ok(())
}

pub async fn rename_group(group_id: i64, name: &str, pool: &SqlitePool) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Group name is empty"));
    }

    let existing_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM camera_recording_rule_groups WHERE name = ?")
            .bind(trimmed)
            .fetch_optional(pool)
            .await?;
    if existing_id.is_some_and(|id| id != group_id) {
        return Err(anyhow!("Group name already exists"));
    }

    let result = sqlx::query(
        r#"
        UPDATE camera_recording_rule_groups
        SET name = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(trimmed)
    .bind(group_id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule group not found"));
    }

    Ok(())
}

pub async fn delete_group(group_id: i64, pool: &SqlitePool) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM camera_recording_rule_group_items WHERE group_id = ?")
        .bind(group_id)
        .execute(&mut *tx)
        .await?;

    let result = sqlx::query("DELETE FROM camera_recording_rule_groups WHERE id = ?")
        .bind(group_id)
        .execute(&mut *tx)
        .await?;

    if result.rows_affected() == 0 {
        return Err(anyhow!("Recording rule group not found"));
    }

    tx.commit().await?;
    Ok(())
}

pub async fn toggle_group_enabled(group_id: i64, pool: &SqlitePool) -> Result<bool> {
    sqlx::query(
        r#"
        UPDATE camera_recording_rule_groups
        SET enabled = CASE WHEN enabled = 0 THEN 1 ELSE 0 END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(group_id)
    .execute(pool)
    .await?;

    let enabled: Option<i64> =
        sqlx::query_scalar("SELECT enabled FROM camera_recording_rule_groups WHERE id = ?")
            .bind(group_id)
            .fetch_optional(pool)
            .await?;
    enabled
        .map(|value| value != 0)
        .ok_or_else(|| anyhow!("Recording rule group not found"))
}

pub async fn cycle_group_access_scope(group_id: i64, pool: &SqlitePool) -> Result<String> {
    let current: Option<String> =
        sqlx::query_scalar("SELECT access_scope FROM camera_recording_rule_groups WHERE id = ?")
            .bind(group_id)
            .fetch_optional(pool)
            .await?;
    let current = current.ok_or_else(|| anyhow!("Recording rule group not found"))?;
    let next = if current == "all_users" {
        "admin_only"
    } else {
        "all_users"
    };

    sqlx::query(
        r#"
        UPDATE camera_recording_rule_groups
        SET access_scope = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(next)
    .bind(group_id)
    .execute(pool)
    .await?;

    Ok(next.to_string())
}

pub async fn list_groups_visible_to_user(
    user_id: u64,
    root_user: u64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingRuleGroup>> {
    if user_id == root_user {
        return list_groups(pool).await;
    }

    let groups = sqlx::query_as::<_, RecordingRuleGroup>(
        r#"
        SELECT g.id, g.name, g.enabled, g.access_scope, g.created_at, COUNT(r.id) AS rules_count
        FROM camera_recording_rule_groups g
        LEFT JOIN camera_recording_rule_group_items i ON i.group_id = g.id
        LEFT JOIN camera_recording_rules r ON r.id = i.rule_id AND r.deleted_at IS NULL
        WHERE g.access_scope = 'all_users'
        GROUP BY g.id, g.name, g.enabled, g.access_scope, g.created_at
        ORDER BY g.name
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut visible = Vec::new();
    for group in groups {
        if user_can_access_all_group_cameras(user_id, false, group.id, pool).await? {
            visible.push(group);
        }
    }

    Ok(visible)
}

pub async fn can_user_toggle_group(
    user_id: u64,
    root_user: u64,
    group_id: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    if user_id == root_user {
        return Ok(get_group(group_id, pool).await?.is_some());
    }

    let access_scope: Option<String> =
        sqlx::query_scalar("SELECT access_scope FROM camera_recording_rule_groups WHERE id = ?")
            .bind(group_id)
            .fetch_optional(pool)
            .await?;

    if access_scope.as_deref() != Some("all_users") {
        return Ok(false);
    }

    user_can_access_all_group_cameras(user_id, false, group_id, pool).await
}

pub async fn toggle_rule_in_group(rule_id: i64, group_id: i64, pool: &SqlitePool) -> Result<bool> {
    let mut conn = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    let result: Result<bool> = async {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM camera_recording_rule_group_items WHERE rule_id = ? AND group_id = ?",
        )
        .bind(rule_id)
        .bind(group_id)
        .fetch_optional(&mut *conn)
        .await?;

        if exists.is_some() {
            sqlx::query(
                "DELETE FROM camera_recording_rule_group_items WHERE rule_id = ? AND group_id = ?",
            )
            .bind(rule_id)
            .bind(group_id)
            .execute(&mut *conn)
            .await?;
            Ok(false)
        } else {
            sqlx::query(
                "INSERT INTO camera_recording_rule_group_items (rule_id, group_id) VALUES (?, ?)",
            )
            .bind(rule_id)
            .bind(group_id)
            .execute(&mut *conn)
            .await?;
            Ok(true)
        }
    }
    .await;

    match result {
        Ok(selected) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            Ok(selected)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            Err(error)
        }
    }
}

pub async fn add_rule_to_group(rule_id: i64, group_id: i64, pool: &SqlitePool) -> Result<bool> {
    let result = sqlx::query(
        r#"
        INSERT OR IGNORE INTO camera_recording_rule_group_items (rule_id, group_id)
        VALUES (?, ?)
        "#,
    )
    .bind(rule_id)
    .bind(group_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn list_group_rules(
    group_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingRuleGroupRule>> {
    Ok(sqlx::query_as::<_, RecordingRuleGroupRule>(
        r#"
        SELECT
            r.id AS rule_id,
            r.name AS rule_name,
            r.enabled AS rule_enabled,
            c.id AS camera_id,
            c.name AS camera_name,
            room.id AS room_id,
            room.area AS room_area,
            room.alias AS room_alias,
            CASE WHEN item.group_id IS NULL THEN 0 ELSE 1 END AS selected
        FROM camera_recording_rules r
        JOIN cameras c ON c.id = r.camera_id AND c.enabled != 0
        LEFT JOIN rooms room ON room.id = c.room_id
        LEFT JOIN camera_recording_rule_group_items item
            ON item.rule_id = r.id AND item.group_id = ?
        WHERE r.deleted_at IS NULL
        ORDER BY
            COALESCE(NULLIF(TRIM(room.alias), ''), NULLIF(TRIM(room.area), ''), 'Без комнаты') COLLATE NOCASE,
            c.name COLLATE NOCASE,
            r.name COLLATE NOCASE
        "#,
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_group_cameras(
    group_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingRuleGroupCamera>> {
    Ok(sqlx::query_as::<_, RecordingRuleGroupCamera>(
        r#"
        SELECT DISTINCT
            c.id AS camera_id,
            c.name AS camera_name,
            room.id AS room_id,
            room.area AS room_area,
            room.alias AS room_alias
        FROM camera_recording_rule_group_items item
        JOIN camera_recording_rules r ON r.id = item.rule_id AND r.deleted_at IS NULL
        JOIN cameras c ON c.id = r.camera_id AND c.enabled != 0
        LEFT JOIN rooms room ON room.id = c.room_id
        WHERE item.group_id = ?
        ORDER BY
            COALESCE(NULLIF(TRIM(room.alias), ''), NULLIF(TRIM(room.area), ''), 'Без комнаты') COLLATE NOCASE,
            c.name COLLATE NOCASE
        "#,
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_group_cameras_visible_to_user(
    user_id: u64,
    is_admin: bool,
    group_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingRuleGroupCamera>> {
    let cameras = list_group_cameras(group_id, pool).await?;
    if is_admin {
        return Ok(cameras);
    }

    let mut visible = Vec::new();
    for camera in cameras {
        if user_can_access_group_camera(user_id, false, &camera, pool).await? {
            visible.push(camera);
        }
    }
    Ok(visible)
}

async fn user_can_access_all_group_cameras(
    user_id: u64,
    is_admin: bool,
    group_id: i64,
    pool: &SqlitePool,
) -> Result<bool> {
    let cameras = list_group_cameras(group_id, pool).await?;
    for camera in cameras {
        if !user_can_access_group_camera(user_id, is_admin, &camera, pool).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn user_can_access_group_camera(
    user_id: u64,
    is_admin: bool,
    camera: &RecordingRuleGroupCamera,
    pool: &SqlitePool,
) -> Result<bool> {
    let Some(room_id) = camera.room_id else {
        return Ok(false);
    };

    crate::db::access::can_view_room(user_id, is_admin, room_id, pool).await
}

#[allow(dead_code)]
pub async fn list_group_rooms(
    group_id: i64,
    pool: &SqlitePool,
) -> Result<Vec<RecordingRuleGroupRoom>> {
    Ok(sqlx::query_as::<_, RecordingRuleGroupRoom>(
        r#"
        SELECT DISTINCT
            room.id AS room_id,
            room.area AS room_area,
            room.alias AS room_alias
        FROM camera_recording_rule_group_items item
        JOIN camera_recording_rules r ON r.id = item.rule_id AND r.deleted_at IS NULL
        JOIN cameras c ON c.id = r.camera_id AND c.enabled != 0
        LEFT JOIN rooms room ON room.id = c.room_id
        WHERE item.group_id = ?
        ORDER BY
            COALESCE(NULLIF(TRIM(room.alias), ''), NULLIF(TRIM(room.area), ''), 'Без комнаты') COLLATE NOCASE
        "#,
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?)
}

pub async fn list_rule_group_ids(rule_id: i64, pool: &SqlitePool) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT group_id
        FROM camera_recording_rule_group_items
        WHERE rule_id = ?
        ORDER BY group_id
        "#,
    )
    .bind(rule_id)
    .fetch_all(pool)
    .await?)
}

pub async fn rule_groups_enabled(rule_id: i64, pool: &SqlitePool) -> Result<bool> {
    let disabled_count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM camera_recording_rule_group_items i
        JOIN camera_recording_rule_groups g ON g.id = i.group_id
        WHERE i.rule_id = ? AND g.enabled = 0
        "#,
    )
    .bind(rule_id)
    .fetch_one(pool)
    .await?;

    Ok(disabled_count == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("sqlite memory pool");
        sqlx::query(
            r#"
            CREATE TABLE camera_recording_rule_groups (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                enabled INTEGER NOT NULL DEFAULT 1,
                access_scope TEXT NOT NULL DEFAULT 'admin_only',
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create groups");
        sqlx::query(
            r#"
            CREATE TABLE camera_recording_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                camera_id INTEGER,
                deleted_at TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create rules");
        sqlx::query(
            r#"
            CREATE TABLE camera_recording_rule_group_items (
                group_id INTEGER NOT NULL,
                rule_id INTEGER NOT NULL,
                PRIMARY KEY(group_id, rule_id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create group items");
        sqlx::query(
            r#"
            CREATE TABLE rooms (
                id INTEGER PRIMARY KEY,
                area TEXT,
                alias TEXT,
                hide INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create rooms");
        sqlx::query(
            r#"
            CREATE TABLE cameras (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                room_id INTEGER,
                enabled INTEGER NOT NULL DEFAULT 1
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create cameras");
        sqlx::query(
            r#"
            CREATE TABLE user_room_access (
                user_id INTEGER,
                room_id INTEGER,
                can_view INTEGER,
                can_control INTEGER,
                PRIMARY KEY (user_id, room_id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .expect("create user room access");
        pool
    }

    async fn create_rule_for_camera(camera_id: i64, pool: &SqlitePool) -> Result<i64> {
        let result =
            sqlx::query("INSERT INTO camera_recording_rules (name, camera_id) VALUES (?, ?)")
                .bind(format!("Правило камеры {camera_id}"))
                .bind(camera_id)
                .execute(pool)
                .await?;
        Ok(result.last_insert_rowid())
    }

    async fn create_camera(room_id: i64, name: &str, pool: &SqlitePool) -> Result<i64> {
        let result = sqlx::query("INSERT INTO cameras (name, room_id) VALUES (?, ?)")
            .bind(name)
            .bind(room_id)
            .execute(pool)
            .await?;
        Ok(result.last_insert_rowid())
    }

    async fn create_room(room_id: i64, alias: &str, pool: &SqlitePool) -> Result<()> {
        sqlx::query("INSERT INTO rooms (id, alias, hide) VALUES (?, ?, 0)")
            .bind(room_id)
            .bind(alias)
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn attach_rule_to_group(rule_id: i64, group_id: i64, pool: &SqlitePool) -> Result<()> {
        sqlx::query(
            "INSERT INTO camera_recording_rule_group_items (group_id, rule_id) VALUES (?, ?)",
        )
        .bind(group_id)
        .bind(rule_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn add_rule_to_group_is_idempotent() {
        let pool = setup_pool().await;
        let group_id = create_group("Охрана", &pool).await.expect("create group");
        let rule_id = create_rule_for_camera(1, &pool).await.expect("create rule");

        assert!(add_rule_to_group(rule_id, group_id, &pool)
            .await
            .expect("first add"));
        assert!(!add_rule_to_group(rule_id, group_id, &pool)
            .await
            .expect("second add"));

        let group_ids = list_rule_group_ids(rule_id, &pool)
            .await
            .expect("group ids");
        assert_eq!(group_ids, vec![group_id]);
    }

    #[tokio::test]
    async fn default_groups_create_test_paused_and_work_groups_enabled() {
        let pool = setup_pool().await;

        ensure_default_groups(&pool).await.expect("default groups");
        let groups = list_groups(&pool).await.expect("list groups");

        let test = groups
            .iter()
            .find(|group| group.name == "Тест")
            .expect("test group");
        assert!(!test.is_enabled());

        for name in ["Охрана", "Ночь", "Двери"] {
            let group = groups
                .iter()
                .find(|group| group.name == name)
                .unwrap_or_else(|| panic!("missing default group {name}"));
            assert!(group.is_enabled(), "{name} should be enabled");
        }
    }

    #[tokio::test]
    async fn default_groups_do_not_change_existing_statuses() {
        let pool = setup_pool().await;
        let group_id = create_group("Тест", &pool).await.expect("create group");
        assert!(get_group(group_id, &pool)
            .await
            .expect("get group")
            .expect("group")
            .is_enabled());

        ensure_default_groups(&pool).await.expect("default groups");

        assert!(get_group(group_id, &pool)
            .await
            .expect("get group")
            .expect("group")
            .is_enabled());
    }

    #[tokio::test]
    async fn all_users_group_is_visible_and_toggleable_for_regular_user() {
        let pool = setup_pool().await;
        let group_id = create_group("Охрана", &pool).await.expect("create group");
        cycle_group_access_scope(group_id, &pool)
            .await
            .expect("cycle access");

        let visible = list_groups_visible_to_user(42, 1, &pool)
            .await
            .expect("visible groups");

        assert_eq!(visible.len(), 1);
        assert!(can_user_toggle_group(42, 1, group_id, &pool)
            .await
            .expect("can toggle"));
    }

    #[tokio::test]
    async fn admin_only_group_is_hidden_from_regular_user() {
        let pool = setup_pool().await;
        let group_id = create_group("Тест", &pool).await.expect("create group");

        let visible = list_groups_visible_to_user(42, 1, &pool)
            .await
            .expect("visible groups");

        assert!(visible.is_empty());
        assert!(!can_user_toggle_group(42, 1, group_id, &pool)
            .await
            .expect("can toggle"));
    }

    #[tokio::test]
    async fn all_users_group_with_inaccessible_camera_is_hidden_from_regular_user() {
        let pool = setup_pool().await;
        let group_id = create_group("Охрана", &pool).await.expect("create group");
        cycle_group_access_scope(group_id, &pool)
            .await
            .expect("cycle access");
        create_room(1, "Коридор", &pool).await.expect("room 1");
        create_room(2, "Кабинет", &pool).await.expect("room 2");
        let visible_camera = create_camera(1, "Доступная камера", &pool)
            .await
            .expect("camera 1");
        let hidden_camera = create_camera(2, "Скрытая камера", &pool)
            .await
            .expect("camera 2");
        attach_rule_to_group(
            create_rule_for_camera(visible_camera, &pool)
                .await
                .expect("rule 1"),
            group_id,
            &pool,
        )
        .await
        .expect("attach rule 1");
        attach_rule_to_group(
            create_rule_for_camera(hidden_camera, &pool)
                .await
                .expect("rule 2"),
            group_id,
            &pool,
        )
        .await
        .expect("attach rule 2");
        crate::db::access::set_room_access(42, 2, false, false, &pool)
            .await
            .expect("deny room");

        let visible = list_groups_visible_to_user(42, 1, &pool)
            .await
            .expect("visible groups");

        assert!(visible.is_empty());
        assert!(!can_user_toggle_group(42, 1, group_id, &pool)
            .await
            .expect("can toggle"));
    }

    #[tokio::test]
    async fn group_cameras_visible_to_user_filters_inaccessible_cameras() {
        let pool = setup_pool().await;
        let group_id = create_group("Охрана", &pool).await.expect("create group");
        create_room(1, "Коридор", &pool).await.expect("room 1");
        create_room(2, "Кабинет", &pool).await.expect("room 2");
        let visible_camera = create_camera(1, "Доступная камера", &pool)
            .await
            .expect("camera 1");
        let hidden_camera = create_camera(2, "Скрытая камера", &pool)
            .await
            .expect("camera 2");
        attach_rule_to_group(
            create_rule_for_camera(visible_camera, &pool)
                .await
                .expect("rule 1"),
            group_id,
            &pool,
        )
        .await
        .expect("attach rule 1");
        attach_rule_to_group(
            create_rule_for_camera(hidden_camera, &pool)
                .await
                .expect("rule 2"),
            group_id,
            &pool,
        )
        .await
        .expect("attach rule 2");
        crate::db::access::set_room_access(42, 2, false, false, &pool)
            .await
            .expect("deny room");

        let cameras = list_group_cameras_visible_to_user(42, false, group_id, &pool)
            .await
            .expect("visible cameras");

        assert_eq!(cameras.len(), 1);
        assert_eq!(cameras[0].camera_id, visible_camera);
    }
}
