use super::*;
use serde_json::{json, Value};

fn message(id: i32) -> Value {
    json!({"ok":true,"result":{"message_id":id,"date":1_700_000_000,"chat":{"id":42,"type":"private"},"text":"ok"}})
}

async fn camera_config(timeout: u64) -> Result<Arc<AppConfig>> {
    let mut config = crate::test_support::config().await;
    Arc::get_mut(&mut config)
        .unwrap()
        .delete_notification_messages_timeout_s = timeout;
    db::add_user(42, &config.db).await?;
    db::rooms::sync_rooms_from_ha("test", "Test", &config.db).await?;
    sqlx::query("INSERT INTO cameras(id,key,name,room_id,stream_url) VALUES(1,'test','Test',1,'rtsp://unused.invalid')").execute(&config.db).await?;
    Ok(config)
}

#[tokio::test]
async fn delivered_videos_each_expire_using_the_existing_message_timeout() -> Result<()> {
    let config = camera_config(1).await?;
    let access = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
    let (bot, server) = crate::bot::router::control_tests::mock_telegram_edits(vec![
        (200, message(701)), (200, message(702)),
        (200, json!({"ok":true,"result":true})),
        (400, json!({"ok":false,"error_code":400,"description":"Bad Request: message to delete not found"})),
    ]).await?;
    let delivered_after = tokio::time::Instant::now();
    assert!(send_manual_camera_video(&bot, ChatId(42), vec![1; 1024], "One", 10, &access).await?);
    assert!(send_manual_camera_video(&bot, ChatId(42), vec![2; 1024], "Two", 10, &access).await?);
    assert!(!server.is_finished(), "Deletion must not be immediate");
    let requests = tokio::time::timeout(StdDuration::from_secs(5), server).await???;
    assert!(delivered_after.elapsed() >= StdDuration::from_secs(1));
    assert_eq!(requests.len(), 4);
    assert!(requests[..2].iter().all(|r| r["method"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("sendvideo")));
    let mut ids = requests[2..]
        .iter()
        .map(|r| {
            assert!(r["method"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("deletemessage"));
            assert_eq!(r["chat_id"], 42);
            r["message_id"].as_i64().unwrap()
        })
        .collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![701, 702],
        "Only delivered video message IDs are deleted"
    );
    Ok(())
}

#[tokio::test]
async fn zero_timeout_and_failed_or_unknown_video_upload_do_not_schedule_deletion_or_retry(
) -> Result<()> {
    for (timeout, response, expected) in [
        (0, message(701), "sent"),
        (
            1,
            json!({"ok":false,"error_code":400,"description":"Bad Request: wrong file identifier/HTTP URL specified"}),
            "failed",
        ),
        (1, json!({"ok":true,"result":true}), "uncertain"),
    ] {
        let config = camera_config(timeout).await?;
        let access = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
        let (bot, mut server) = crate::bot::router::control_tests::mock_telegram_edits(vec![
            (200, response),
            (200, json!({"ok":true,"result":true})),
        ])
        .await?;
        let result =
            send_manual_camera_video(&bot, ChatId(42), vec![1; 1024], "Test", 10, &access).await;
        match expected {
            "sent" => assert!(result?),
            "uncertain" => assert!(result.unwrap_err().is::<UploadUncertain>()),
            _ => assert!(result.is_err()),
        }
        let extra_request = tokio::time::timeout(StdDuration::from_millis(1200), &mut server).await;
        server.abort();
        assert!(
            extra_request.is_err(),
            "No delete or automatic retry follows {expected}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn archive_delivery_reports_partial_success_and_keeps_source_files() -> Result<()> {
    let mut config = camera_config(0).await?;
    let suffix: String = sqlx::query_scalar("SELECT lower(hex(randomblob(16)))")
        .fetch_one(&config.db)
        .await?;
    let root = std::env::temp_dir().join(format!("ha-video-receipt-{suffix}"));
    std::fs::create_dir_all(&root)?;
    Arc::get_mut(&mut config)
        .unwrap()
        .camera_recording_storage_root = root.to_string_lossy().into();
    let rule = sqlx::query("INSERT INTO camera_recording_rules(name,camera_id) VALUES('Test',1)")
        .execute(&config.db)
        .await?
        .last_insert_rowid();
    let session = sqlx::query("INSERT INTO camera_recording_sessions(event_group_id,rule_id,camera_id,trigger_summary,status,first_event_at,last_event_at,stop_after_at,expires_at) VALUES('video-test',?,1,'Test','completed',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,datetime('now','+1 day'))").bind(rule).execute(&config.db).await?.last_insert_rowid();
    for index in 0..3 {
        let path = format!("{index}.mp4");
        if index < 2 {
            std::fs::write(root.join(&path), b"test video")?;
            std::fs::write(
                root.join(format!("{index}.preview.jpg")),
                crate::bot::utils::UI_PLACEHOLDER_BYTES,
            )?;
        }
        sqlx::query("INSERT INTO camera_recording_segments(session_id,camera_id,segment_index,file_path,duration_s,status,expires_at) VALUES(?,1,?,?,10,'ready',datetime('now','+1 day'))")
            .bind(session).bind(index).bind(&path).execute(&config.db).await?;
    }
    let access = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
    let (bot, server) = crate::bot::router::control_tests::mock_telegram_edits(vec![
        (200, message(700)),
        (200, message(701)),
        (200, message(702)),
    ])
    .await?;
    let outcome = send_recording_all(&bot, ChatId(42), 42, 1, session, &config, &access).await?;
    assert_eq!(
        outcome,
        VideoOutcome {
            sent: 2,
            failed: 1,
            uncertain: false
        }
    );
    let requests = tokio::time::timeout(StdDuration::from_secs(5), server).await???;
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("sendvideo"))
            .count(),
        2
    );
    assert!(root.join("0.mp4").exists());
    assert!(root.join("1.mp4").exists());
    std::fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test]
async fn manual_archive_preserves_original_after_chat_expiry_and_obeys_retention() -> Result<()> {
    let storage = crate::core::cameras::TemporaryMedia::new()?;
    let root = storage.path().parent().unwrap().to_path_buf();
    let mut config = camera_config(1).await?;
    Arc::get_mut(&mut config)
        .unwrap()
        .camera_recording_storage_root = root.to_string_lossy().into();
    let access = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
    assert!(!db::cameras::archives_manual_clips(1, &config.db).await?);
    db::cameras::set_manual_clip_archive(1, 1, true, &config.db).await?;
    db::settings::set_i64(
        db::settings::CAMERA_RECORDING_DEFAULT_RETENTION_DAYS,
        7,
        &config.db,
    )
    .await?;
    let (bot, server) = crate::bot::router::control_tests::mock_telegram_edits(vec![
        (200, message(801)),
        (200, json!({"ok":true,"result":true})),
    ])
    .await?;
    let bytes = vec![42; 1024];
    assert!(
        send_manual_camera_video(&bot, ChatId(42), bytes.clone(), "Manual", 15, &access).await?
    );
    let calls = tokio::time::timeout(StdDuration::from_secs(5), server).await???;
    assert!(calls[0]["multipart"]
        .as_str()
        .unwrap()
        .contains("Сохранено в архив"));
    assert!(calls[1]["method"]
        .as_str()
        .unwrap()
        .to_lowercase().contains("deletemessage"));
    let sessions = db::camera_recording_sessions::list_camera_sessions(1, 20, &config.db).await?;
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.rule_id, None);
    assert_eq!(session.status, "ready");
    assert!(
        session.notification_sent_at.is_some(),
        "No automation notifications for a manual clip"
    );
    assert!(session.expires_at > Utc::now() + chrono::Duration::days(6));
    assert!(session.expires_at <= Utc::now() + chrono::Duration::days(7));
    let segments =
        db::camera_recording_segments::list_ready_segments(session.id, &config.db).await?;
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].duration_s, 15);
    assert_eq!(segments[0].expires_at, session.expires_at);
    let path = root.join(segments[0].file_path.as_ref().unwrap());
    assert_eq!(
        std::fs::read(&path)?,
        bytes,
        "Chat expiry leaves the archived original intact"
    );
    assert_eq!(
        db::camera_recording_segments::sum_ready_size_bytes(&config.db).await?,
        1024
    );
    db::cameras::set_manual_clip_archive(1, 1, false, &config.db).await?;
    assert!(
        path.exists(),
        "Disabling saves does not delete existing recordings"
    );
    sqlx::query(
        "UPDATE camera_recording_sessions SET expires_at=datetime('now','-1 day') WHERE id=?",
    )
    .bind(session.id)
    .execute(&config.db)
    .await?;
    assert_eq!(
        db::camera_recording_sessions::find_expired_sessions(&config.db)
            .await?
            .len(),
        1
    );
    assert!(
        crate::core::camera_recording::delete_recording_session_files(&config, session.id).await?
    );
    assert!(!path.exists());
    assert!(
        db::camera_recording_sessions::get_session(session.id, &config.db)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn manual_archive_off_and_storage_failure_do_not_prevent_delivery() -> Result<()> {
    for enabled in [false, true] {
        let storage = crate::core::cameras::TemporaryMedia::new()?;
        std::fs::write(storage.path(), b"not a directory")?;
        let mut config = camera_config(0).await?;
        Arc::get_mut(&mut config)
            .unwrap()
            .camera_recording_storage_root = storage.path().to_string_lossy().into();
        db::cameras::set_manual_clip_archive(1, 1, enabled, &config.db).await?;
        let access = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
        let (bot, server) =
            crate::bot::router::control_tests::mock_telegram_edits(vec![(200, message(802))])
                .await?;
        assert!(
            send_manual_camera_video(&bot, ChatId(42), vec![1; 1024], "Manual", 10, &access)
                .await?
        );
        let calls = tokio::time::timeout(StdDuration::from_secs(5), server).await???;
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0]["multipart"]
                .as_str()
                .unwrap()
                .contains("Не удалось сохранить"),
            enabled
        );
        let sessions =
            db::camera_recording_sessions::list_camera_sessions(1, 20, &config.db).await?;
        assert_eq!(sessions.len(), usize::from(enabled));
        if enabled {
            assert_eq!(sessions[0].status, "failed");
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM recording_writers")
                .fetch_one(&config.db)
                .await?,
            0
        );
    }
    Ok(())
}

#[tokio::test]
async fn manual_archive_survives_upload_failure_and_is_independent_of_automatic_recording(
) -> Result<()> {
    let storage = crate::core::cameras::TemporaryMedia::new()?;
    let root = storage.path().parent().unwrap().to_path_buf();
    let mut config = camera_config(0).await?;
    Arc::get_mut(&mut config)
        .unwrap()
        .camera_recording_storage_root = root.to_string_lossy().into();
    db::cameras::set_manual_clip_archive(1, 1, true, &config.db).await?;
    let rule = sqlx::query("INSERT INTO camera_recording_rules(name,camera_id) VALUES('Auto',1)")
        .execute(&config.db)
        .await?
        .last_insert_rowid();
    let rule = db::camera_recording_rules::get_rule(rule, &config.db)
        .await?
        .unwrap();
    let importing = db::camera_recording_sessions::create_manual_session(1, 42, 10, 7, &config.db)
        .await?
        .unwrap();
    let db::camera_recording_sessions::SessionAction::Created(automatic) =
        db::camera_recording_sessions::start_or_extend_session(
            &rule,
            "event",
            "event",
            Utc::now(),
            &config.db,
        )
        .await?
    else {
        panic!("Manual import must not absorb an event");
    };
    assert_ne!(importing, automatic);
    assert_eq!(
        db::camera_recording_sessions::get_active_camera_session(1, &config.db)
            .await?
            .unwrap()
            .id,
        automatic
    );
    let access = CameraDeliveryAccess::new(ChatId(42), 42, 1, &config).await?;
    let (bot, server) = crate::bot::router::control_tests::mock_telegram_edits(vec![(
        400,
        json!({"ok":false,"error_code":400,"description":"Bad Request: upload failed"}),
    )])
    .await?;
    assert!(
        send_manual_camera_video(&bot, ChatId(42), vec![2; 1024], "Manual", 10, &access)
            .await
            .is_err()
    );
    let _ = tokio::time::timeout(StdDuration::from_secs(5), server).await???;
    let ready = db::camera_recording_sessions::list_camera_sessions_filtered(
        1,
        db::camera_recording_sessions::RecordingArchiveFilter::Ready,
        20,
        0,
        &config.db,
    )
    .await?;
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].rule_id, None);
    let segments =
        db::camera_recording_segments::list_ready_segments(ready[0].id, &config.db).await?;
    assert_eq!(
        std::fs::read(root.join(segments[0].file_path.as_ref().unwrap()))?,
        vec![2; 1024]
    );
    assert!(
        db::camera_recording_rules::get_rule(rule.id, &config.db)
            .await?
            .unwrap()
            .last_completed_at
            .is_none(),
        "Manual save must not start an automation cooldown"
    );
    Ok(())
}
