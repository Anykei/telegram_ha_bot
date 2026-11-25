use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::time::Duration;

#[tokio::test]
async fn unchanged_sync_and_idle_expiry_do_not_wait_for_sqlite_writer() -> Result<()> {
    let path = std::env::temp_dir().join(format!(
        "ha-idle-writes-{}-{}.db",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    super::rooms::sync_rooms_from_ha("kitchen", "Kitchen", &pool).await?;
    super::devices::sync_device("light.test", "kitchen", "Light", "light", &pool).await?;
    sqlx::query("UPDATE rooms SET hide = 1; UPDATE hidden_entities SET hide = 0")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO action_schedule_runs (schedule_id, local_run_key, planned_at, deadline_at, revision, target_type, target_id, command, snapshot_json, status) VALUES (1, 'test', 1000, 1060, 1, 'bot_group', 1, 'turn_on', '[]', 'pending')")
        .execute(&pool).await?;

    let writer = pool.begin_with("BEGIN IMMEDIATE").await?;
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        super::rooms::sync_rooms_from_ha("kitchen", "Changed HA room", &pool).await?;
        super::devices::sync_device("light.test", "kitchen", "Light", "light", &pool).await?;
        assert_eq!(
            super::devices::archive_missing_devices(&["light.test".into()], &pool).await?,
            0
        );
        assert_eq!(super::action_groups::runs::expire(1059, &pool).await?, 0);
        assert_eq!(super::action_groups::runs::recover_started(&pool).await?, 0);
        assert_eq!(
            super::activity_log::purge_older_than(chrono::Utc::now(), &pool).await?,
            0
        );
        assert_eq!(super::pending_commands::expire_old(&pool).await?, 0);
        assert_eq!(super::ui_confirmations::purge_expired(&pool).await?, 0);
        super::ui_drafts::purge_expired(&pool).await?;
        anyhow::Ok(())
    })
    .await;
    writer.rollback().await?;
    result??;

    assert_eq!(super::action_groups::runs::expire(1060, &pool).await?, 1);
    assert_eq!(super::action_groups::runs::expire(1061, &pool).await?, 0);
    let room: (String, i64) = sqlx::query_as("SELECT alias, hide FROM rooms")
        .fetch_one(&pool)
        .await?;
    assert_eq!(room, ("Kitchen".into(), 1));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT hide FROM hidden_entities")
            .fetch_one(&pool)
            .await?,
        0
    );

    // Changed metadata and legacy incomplete records must still be repaired.
    sqlx::query("UPDATE devices SET alias = '', archived = 1; DELETE FROM hidden_entities; UPDATE rooms SET alias = NULL")
        .execute(&pool).await?;
    super::rooms::sync_rooms_from_ha("kitchen", "New kitchen", &pool).await?;
    super::devices::sync_device("light.test", "kitchen", "New light", "changed", &pool).await?;
    let device: (String, String, i64) =
        sqlx::query_as("SELECT alias, device_class, archived FROM devices")
            .fetch_one(&pool)
            .await?;
    assert_eq!(device, ("New light".into(), "changed".into(), 0));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT alias FROM rooms")
            .fetch_one(&pool)
            .await?,
        "New kitchen"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT hide FROM hidden_entities")
            .fetch_one(&pool)
            .await?,
        1
    );
    pool.close().await;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn existing_admission_and_unchanged_display_do_not_wait_for_sqlite_writer() -> Result<()> {
    let path = std::env::temp_dir().join(format!(
        "ha-user-read-path-{}-{}.db",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    let user_id = 42;
    super::add_user(user_id, &pool).await?;
    let generation = super::admission_generation(user_id, &pool).await?.unwrap();
    super::user_display::capture(user_id, "First", Some("Last"), Some("user"), &pool).await?;
    super::user_display::set_alias(user_id, Some("Admin alias"), &pool).await?;

    let writer = pool.begin_with("BEGIN IMMEDIATE").await?;
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        super::add_user(user_id, &pool).await?;
        super::user_display::capture(user_id, " First ", Some("Last"), Some("user"), &pool).await?;
        assert!(super::admission_is_current(user_id, &generation, &pool).await?);
        anyhow::Ok(())
    })
    .await;
    writer.rollback().await?;
    result??;

    super::user_display::capture(user_id, "Renamed", None, None, &pool).await?;
    let metadata = super::user_display::get(user_id, &pool).await?;
    assert_eq!(metadata.display_name.as_deref(), Some("Renamed"));
    assert_eq!(metadata.username, None);
    assert_eq!(metadata.admin_alias.as_deref(), Some("Admin alias"));

    super::delete_user(user_id, &pool).await?;
    super::add_user(user_id, &pool).await?;
    assert!(super::admission_generation(user_id, &pool).await?.is_some());
    assert!(!super::admission_is_current(user_id, &generation, &pool).await?);
    pool.close().await;
    std::fs::remove_file(path)?;
    Ok(())
}
