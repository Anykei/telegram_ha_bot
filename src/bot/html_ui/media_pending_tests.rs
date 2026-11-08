use super::super::super::media_pending::{Outcome, Target};
use super::*;

#[tokio::test]
async fn video_pending_remains_after_read_new_revision_and_navigation_without_replaying(
) -> Result<()> {
    let (config, _) = config().await?;
    let camera = db::cameras::add_manual_camera(
        db::cameras::NewCamera {
            name: "Video test",
            room_id: 1,
            stream_url: "mock://unused",
            snapshot_url: None,
            clip_seconds: 10,
        },
        &config.db,
    )
    .await?;
    // Prevent the camera preview path from scheduling a real capture.
    config.camera_snapshot_cache.lock().await.insert(
        camera,
        crate::models::CameraSnapshotCache {
            bytes: None,
            captured_at: None,
            failed_at: Some(chrono::Utc::now()),
            last_warned_at: None,
            last_error: None,
            refreshing: false,
        },
    );
    let fake = FakeTelegram::new().await?;
    let route = Payload::Camera(CameraPayload::CameraDetail { id: camera });
    publish(&fake, &config, route.clone()).await?;
    let generation = current(&config).generation;
    let target = Target::Clip {
        camera,
        seconds: 10,
    };
    let operation = config
        .html_ui
        .media
        .begin(&config, 1, &generation, target.clone())
        .unwrap();
    let first = publish(&fake, &config, route.clone()).await?;
    let action = Payload::Camera(CameraPayload::Clip {
        id: camera,
        seconds: 10,
    });
    let pending_kind = |s: &WidgetSession| {
        s.pages
            .iter()
            .flat_map(|p| p.rows.iter().flatten())
            .find(|b| s.actions.get(&b.key) == Some(&action))
            .unwrap()
            .kind
            .clone()
    };
    assert_eq!(pending_kind(&first), "wait");
    let snapshot = first
        .pages
        .iter()
        .flat_map(|p| p.rows.iter().flatten())
        .find(|b| {
            matches!(
                first.actions.get(&b.key),
                Some(Payload::Camera(CameraPayload::Snapshot { .. }))
            )
        })
        .unwrap();
    assert_eq!(
        snapshot.kind, "action",
        "Independent actions stay available"
    );
    let blocked = resolve(&fake.bot, &action_query(&config, &action), &config).await?;
    assert_eq!(
        blocked,
        Some(route.to_string()),
        "Fresh duplicate tap only requests a read"
    );
    let read = resolve(&fake.bot, &query(&current(&config), "read"), &config).await?;
    assert_eq!(read, Some(route.to_string()));
    publish(&fake, &config, Payload::Home).await?;
    let returned = publish(&fake, &config, route.clone()).await?;
    assert_eq!(pending_kind(&returned), "wait");
    assert!(config
        .html_ui
        .media
        .begin(&config, 1, &generation, target)
        .is_none());
    // Completion while elsewhere must not resurrect the camera page.
    let home = publish(&fake, &config, Payload::Home).await?;
    fake.clear();
    operation.complete(Outcome::sent()).await;
    assert_eq!(current(&config).message, home.message);
    assert!(fake.calls().is_empty());
    let finished = publish(&fake, &config, route).await?;
    assert_eq!(pending_kind(&finished), "action");
    assert!(finished.pages.iter().any(|p| !p.notice.is_empty()));
    assert!(!fake
        .calls()
        .iter()
        .any(|(m, _)| m.eq_ignore_ascii_case("sendVideo")));
    Ok(())
}

#[tokio::test]
async fn archive_send_all_button_stays_visible_and_busy_until_worker_finishes() -> Result<()> {
    let (mut config, _) = config().await?;
    let suffix: String = sqlx::query_scalar("SELECT lower(hex(randomblob(16)))")
        .fetch_one(&config.db)
        .await?;
    let root = std::env::temp_dir().join(format!("ha-media-pending-{suffix}"));
    std::fs::create_dir_all(&root)?;
    Arc::get_mut(&mut config)
        .unwrap()
        .camera_recording_storage_root = root.to_string_lossy().into();
    let camera = db::cameras::add_manual_camera(
        db::cameras::NewCamera {
            name: "Video test",
            room_id: 1,
            stream_url: "mock://unused",
            snapshot_url: None,
            clip_seconds: 10,
        },
        &config.db,
    )
    .await?;
    let rule = sqlx::query("INSERT INTO camera_recording_rules(name,camera_id) VALUES('Test',?)")
        .bind(camera)
        .execute(&config.db)
        .await?
        .last_insert_rowid();
    let session = sqlx::query("INSERT INTO camera_recording_sessions(event_group_id,rule_id,camera_id,trigger_summary,status,first_event_at,last_event_at,stop_after_at,expires_at) VALUES('media-test',?,?,'Test','completed',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,datetime('now','+1 day'))").bind(rule).bind(camera).execute(&config.db).await?.last_insert_rowid();
    for index in 0..2 {
        let path = format!("{index}.mp4");
        std::fs::write(root.join(&path), b"test video")?;
        std::fs::write(
            root.join(format!("{index}.preview.jpg")),
            crate::bot::utils::UI_PLACEHOLDER_BYTES,
        )?;
        sqlx::query("INSERT INTO camera_recording_segments(session_id,camera_id,segment_index,file_path,duration_s,status,expires_at) VALUES(?,?,?,?,10,'ready',datetime('now','+1 day'))")
            .bind(session).bind(camera).bind(index).bind(&path).execute(&config.db).await?;
    }
    let fake = FakeTelegram::new().await?;
    let route = Payload::Camera(CameraPayload::RecordingSession { camera, session });
    let initial = publish(&fake, &config, route.clone()).await?;
    let target = Target::All { camera, session };
    let operation = config
        .html_ui
        .media
        .begin(&config, 1, &initial.generation, target)
        .unwrap();
    let pending = publish(&fake, &config, route.clone()).await?;
    let button = pending
        .pages
        .iter()
        .flat_map(|p| p.rows.iter().flatten())
        .find(|b| {
            matches!(
                pending.actions.get(&b.key),
                Some(Payload::Camera(CameraPayload::SendRecordingAll { .. }))
            )
        })
        .expect("Send all stays present while busy");
    assert_eq!(button.kind, "wait");
    drop(operation);
    let finished = publish(&fake, &config, route).await?;
    assert!(finished.pages.iter().any(|p| !p.alert.is_empty()));
    assert!(root.join("0.mp4").exists());
    std::fs::remove_dir_all(root)?;
    Ok(())
}

#[tokio::test]
async fn cancelled_video_worker_queues_unknown_receipt_without_restoring_old_navigation(
) -> Result<()> {
    for leave in [false, true] {
        let (config, _) = config().await?;
        let camera = db::cameras::add_manual_camera(
            db::cameras::NewCamera {
                name: "Video test",
                room_id: 1,
                stream_url: "mock://unused",
                snapshot_url: None,
                clip_seconds: 10,
            },
            &config.db,
        )
        .await?;
        config.camera_snapshot_cache.lock().await.insert(
            camera,
            crate::models::CameraSnapshotCache {
                bytes: None,
                captured_at: None,
                failed_at: Some(chrono::Utc::now()),
                last_warned_at: None,
                last_error: None,
                refreshing: false,
            },
        );
        let fake = FakeTelegram::new().await?;
        let route = Payload::Camera(CameraPayload::CameraDetail { id: camera });
        let initial = publish(&fake, &config, route.clone()).await?;
        let target = Target::Clip {
            camera,
            seconds: 10,
        };
        let operation = config
            .html_ui
            .media
            .begin(&config, 1, &initial.generation, target.clone())
            .unwrap();
        publish(
            &fake,
            &config,
            if leave { Payload::Home } else { route.clone() },
        )
        .await?;
        let before = current(&config).message;
        let task = tokio::spawn(async move {
            let _operation = operation;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        task.abort();
        assert!(task.await.is_err());
        assert!(!config.html_ui.media.blocks(1, &initial.generation, &target));
        if leave {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            assert_eq!(config.runtime_status.read().await.ui_refresh_queued, 0);
            assert_eq!(current(&config).message, before);
        } else {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while config.runtime_status.read().await.ui_refresh_queued == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await?;
            let receipt = publish(&fake, &config, route).await?;
            assert!(receipt
                .pages
                .iter()
                .any(|p| p.alert.contains("не подтверждена")));
        }
    }
    Ok(())
}
