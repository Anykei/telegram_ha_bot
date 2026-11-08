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
    assert!(send_manual_camera_video(&bot, ChatId(42), vec![1; 1024], "One", &access).await?);
    assert!(send_manual_camera_video(&bot, ChatId(42), vec![2; 1024], "Two", &access).await?);
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
            send_manual_camera_video(&bot, ChatId(42), vec![1; 1024], "Test", &access).await;
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
