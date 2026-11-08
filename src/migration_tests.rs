use std::borrow::Cow;
use std::path::Path;

#[tokio::test]
async fn remediation_upgrade_preserves_legacy_data_and_is_idempotent() -> anyhow::Result<()> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let mut baseline = sqlx::migrate::Migrator::new(Path::new("migrations")).await?;
    baseline.migrations = Cow::Owned(
        baseline
            .iter()
            .filter(|m| m.version < 20260930000000)
            .cloned()
            .collect(),
    );
    baseline.run(&pool).await?;
    sqlx::raw_sql(r#"
        INSERT INTO users(id,language,last_menu_id,current_context) VALUES(42,'en',123,'old-screen');
        INSERT INTO rooms(id,area,alias,hide) VALUES(1,'kitchen','Kitchen',0);
        INSERT INTO devices(id,entity_id,room_id,alias,ha_name,device_class) VALUES(1,'light.kitchen',1,'Alias','HA Name','light');
        INSERT INTO user_profiles(user_id,role) VALUES(42,'guest');
        INSERT INTO user_room_access(user_id,room_id,can_view,can_control) VALUES(42,1,0,0);
        INSERT INTO user_device_access(user_id,entity_id,can_view,can_control,can_notify) VALUES(42,'light.kitchen',1,1,0);
        INSERT INTO subscriptions(user_id,entity_id) VALUES(42,'light.kitchen');
        INSERT INTO cameras(id,key,name,stream_url) VALUES(1,'test','Test','rtsp://test.invalid');
        INSERT INTO camera_recording_rules(id,name,camera_id) VALUES(1,'Rule',1);
        INSERT INTO camera_recording_sessions(id,event_group_id,rule_id,camera_id,trigger_summary,status,first_event_at,last_event_at,stop_after_at,expires_at)
        VALUES(1,'test',1,1,'test','ready','2026-09-30T10:00:00Z','2026-09-30T10:00:00Z','2026-09-30T10:01:00Z','2026-10-30T10:00:00Z');
        INSERT INTO camera_recording_segments(id,session_id,camera_id,segment_index,file_path,duration_s,size_bytes,status,expires_at)
        VALUES(1,1,1,0,'/test/recordings/1.mp4',60,12345,'ready','2026-10-30T10:00:00Z');
        INSERT INTO action_groups(id,name) VALUES(1,'Lights');
        INSERT INTO action_group_items(group_id,device_id) VALUES(1,1);
        INSERT INTO action_schedules(id,target_type,target_id,command,time_minute,last_run_at)
        VALUES(1,'bot_group',1,'turn_off',600,'2026-09-30 10:00');
    "#).execute(&pool).await?;
    let upgrade = sqlx::migrate::Migrator::new(Path::new("migrations")).await?;
    upgrade.run(&pool).await?;
    upgrade.run(&pool).await?;
    let acl: (i64, i64, i64) = sqlx::query_as(
        "SELECT can_view,can_control,can_notify FROM user_device_access WHERE user_id=42",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(acl, (1, 1, 0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT can_notify FROM user_notification_access WHERE user_id=42"
        )
        .fetch_one(&pool)
        .await?,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM legacy_device_acl_report WHERE user_id=42"
        )
        .fetch_one(&pool)
        .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM subscriptions WHERE user_id=42")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT alias FROM devices WHERE id=1")
            .fetch_one(&pool)
            .await?,
        "Alias"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT language FROM users WHERE id=42")
            .fetch_one(&pool)
            .await?,
        "en"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT last_run_at FROM action_schedules WHERE id=1")
            .fetch_one(&pool)
            .await?,
        "2026-09-30 10:00"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT size_bytes FROM recording_files WHERE segment_id=1")
            .fetch_one(&pool)
            .await?,
        12345
    );
    assert!(crate::db::get_all_active_sessions(&pool).await?.is_empty());
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await?
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await?,
        "ok"
    );
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn manual_archive_upgrade_preserves_recordings_ownership_and_sequences() -> anyhow::Result<()>
{
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    let mut baseline = sqlx::migrate::Migrator::new(Path::new("migrations")).await?;
    baseline.migrations = Cow::Owned(
        baseline
            .iter()
            .filter(|m| m.version < 20261108090000)
            .cloned()
            .collect(),
    );
    baseline.run(&pool).await?;
    sqlx::raw_sql(r#"
        INSERT INTO cameras(id,key,name,stream_url) VALUES(1,'test','Test','rtsp://unused');
        INSERT INTO camera_recording_rules(id,name,camera_id) VALUES(1,'Rule',1);
        INSERT INTO camera_recording_sessions(id,event_group_id,rule_id,camera_id,trigger_summary,status,first_event_at,last_event_at,stop_after_at,expires_at,pinned_at,pre_roll_actual_seconds)
        VALUES(1,'event',1,1,'test','ready',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,datetime('now','+30 days'),CURRENT_TIMESTAMP,7);
        INSERT INTO camera_recording_segments(id,session_id,camera_id,segment_index,file_path,duration_s,size_bytes,status,expires_at)
        VALUES(1,1,1,1,'existing.mp4',10,1234,'ready',datetime('now','+30 days'));
        INSERT INTO recording_files(path,session_id,segment_id,size_bytes,delete_pending,attempts,last_error) VALUES('existing.mp4',1,1,1234,1,2,'retry');
        INSERT INTO recording_writers(id,session_id,segment_id,kind) VALUES(7,1,1,'capture');
        UPDATE sqlite_sequence SET seq=100 WHERE name='camera_recording_sessions';
    "#).execute(&pool).await?;
    let upgrade = sqlx::migrate::Migrator::new(Path::new("migrations")).await?;
    upgrade.run(&pool).await?;
    upgrade.run(&pool).await?;
    assert!(!crate::db::cameras::archives_manual_clips(1, &pool).await?);
    let session = crate::db::camera_recording_sessions::get_session(1, &pool)
        .await?
        .unwrap();
    assert_eq!(session.rule_id, Some(1));
    assert_eq!(session.pre_roll_actual_seconds, 7);
    assert!(session.pinned_at.is_some());
    let segments = crate::db::camera_recording_segments::list_ready_segments(1, &pool).await?;
    assert_eq!(segments[0].file_path.as_deref(), Some("existing.mp4"));
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64, String)>(
            "SELECT size_bytes,delete_pending,attempts,last_error FROM recording_files"
        )
        .fetch_one(&pool)
        .await?,
        (1234, 1, 2, "retry".into())
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64, String)>(
            "SELECT id,session_id,segment_id,kind FROM recording_writers"
        )
        .fetch_one(&pool)
        .await?,
        (7, 1, 1, "capture".into())
    );
    sqlx::query("UPDATE cameras SET archive_manual_clips=1 WHERE id=1")
        .execute(&pool)
        .await?;
    assert_eq!(
        crate::db::camera_recording_sessions::create_manual_session(1, 42, 10, 7, &pool).await?,
        Some(101)
    );
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await?
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await?,
        "ok"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await?,
        1
    );
    Ok(())
}
