use super::*;
use crate::i18n::Language;

async fn camera_config() -> Result<(Arc<AppConfig>, i64)> {
    let (mut config, _) = config().await?;
    Arc::get_mut(&mut config).unwrap().camera_clip_intervals_s = vec![5, 10, 15, 30];
    let camera = db::cameras::add_manual_camera(
        db::cameras::NewCamera {
            name: "USB камера",
            room_id: 1,
            stream_url: "mock://camera",
            snapshot_url: None,
            clip_seconds: 10,
        },
        &config.db,
    )
    .await?;
    config.camera_snapshot_cache.lock().await.insert(
        camera,
        crate::models::CameraSnapshotCache {
            bytes: Some(crate::bot::utils::UI_PLACEHOLDER_BYTES.to_vec()),
            captured_at: Some(chrono::Utc::now()),
            failed_at: Some(chrono::Utc::now()),
            last_warned_at: None,
            last_error: None,
            refreshing: false,
        },
    );
    Ok((config, camera))
}

#[tokio::test]
async fn camera_controls_keep_media_actions_and_classic_preview() -> Result<()> {
    let (config, camera) = camera_config().await?;
    let fake = FakeTelegram::new().await?;
    for lang in [Language::Ru, Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        for admin in [false, true] {
            let payload = if admin {
                Payload::Admin(AdminPayload::RoomCameraDetail { room: 1, camera })
            } else {
                Payload::Camera(CameraPayload::CameraDetail { id: camera })
            };
            let view = router(payload, 1, config.clone()).await?;
            assert!(view.image.is_some(), "Classic preview must be retained");
            let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
            assert_complete(&view, &prepared)?;
            let first = &prepared.pages[0];
            let fixtures: Vec<Value> = serde_json::from_str(include_str!(
                "../../../ui/widgets/preview/release-fixtures.json"
            ))?;
            let fixture_id = format!(
                "camera{}{}",
                if admin { "_admin" } else { "" },
                if lang == Language::En { "_en" } else { "" }
            );
            let fixture = fixtures.iter().find(|f| f["id"] == fixture_id).unwrap();
            assert_eq!(
                fixture["page"],
                serde_json::to_value(first)?,
                "Browser tests must use the backend's actual camera page"
            );
            assert_eq!(first.title, "USB камера");
            assert_eq!(
                first.body,
                crate::bot::screens::cameras::media_delivery_hint(lang)
            );
            assert!(first.back.is_some());
            assert!(first
                .rows
                .iter()
                .take(3)
                .flatten()
                .all(|b| b.kind == "action"));
            let first_actions = first
                .rows
                .iter()
                .flatten()
                .map(|b| &prepared.actions[&b.key])
                .collect::<Vec<_>>();
            for seconds in [5, 10, 15, 30] {
                let clip = if admin {
                    Payload::Admin(AdminPayload::RoomCameraClip {
                        room: 1,
                        camera,
                        seconds,
                    })
                } else {
                    Payload::Camera(CameraPayload::Clip {
                        id: camera,
                        seconds,
                    })
                };
                assert!(
                    first_actions.contains(&&clip),
                    "Video {seconds}s must be on the first page"
                );
            }
            if !admin {
                for action in [
                    CameraPayload::Snapshot { id: camera },
                    CameraPayload::RecordingArchive { camera },
                    CameraPayload::CameraDetails { id: camera },
                ] {
                    assert!(first_actions.contains(&&Payload::Camera(action)));
                }
            }
            assert!(!first.json()?.contains("mock://"));
            let mut unmarked = view.clone();
            unmarked.html_media_controls = false;
            assert!(
                !presentation::supported(&unmarked),
                "Other images still need native delivery"
            );
            handlers::send_new_view_inner(
                &fake.bot,
                ChatId(1),
                1,
                view.clone(),
                config.clone(),
                None,
            )
            .await?;
            assert_eq!(
                config.sessions.get(&1).unwrap().ui_message_mode,
                UiMessageMode::Html
            );
            assert!(!fake
                .calls()
                .iter()
                .any(|(m, _)| m.eq_ignore_ascii_case("sendPhoto")));
            db::interface::set(1, db::interface::InterfaceMode::Classic, &config.db).await?;
            fake.clear();
            handlers::send_new_view_inner(&fake.bot, ChatId(1), 1, view, config.clone(), None)
                .await?;
            assert!(fake
                .calls()
                .iter()
                .any(|(m, _)| m.eq_ignore_ascii_case("sendPhoto")));
            db::interface::set(1, db::interface::InterfaceMode::Html, &config.db).await?;
            fake.clear();
        }
    }
    // An unavailable camera's placeholder must not switch the controls to classic.
    config
        .camera_snapshot_cache
        .lock()
        .await
        .get_mut(&camera)
        .unwrap()
        .bytes = None;
    publish(
        &fake,
        &config,
        Payload::Camera(CameraPayload::CameraDetail { id: camera }),
    )
    .await?;
    assert_eq!(
        config.sessions.get(&1).unwrap().ui_message_mode,
        UiMessageMode::Html
    );
    Ok(())
}

#[tokio::test]
async fn camera_navigation_reuses_widget_and_rechecks_camera_access() -> Result<()> {
    let (config, camera) = camera_config().await?;
    let fake = FakeTelegram::new().await?;
    let first = publish(&fake, &config, Payload::Camera(CameraPayload::ListCameras)).await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    fake.clear();
    for payload in [
        CameraPayload::CameraDetail { id: camera },
        CameraPayload::CameraDetails { id: camera },
        CameraPayload::CameraDetail { id: camera },
        CameraPayload::RecordingArchive { camera },
        CameraPayload::CameraDetail { id: camera },
        CameraPayload::ListCameras,
    ] {
        let action = Payload::Camera(payload);
        press(&fake, &config, &dialogue, action_query(&config, &action)).await?;
        assert_eq!(current(&config).message, first.message);
    }
    assert!(!fake
        .calls()
        .iter()
        .any(|(m, _)| m.to_ascii_lowercase().starts_with("send")));
    let open = Payload::Camera(CameraPayload::CameraDetail { id: camera });
    press(&fake, &config, &dialogue, action_query(&config, &open)).await?;
    let q = action_query(
        &config,
        &Payload::Camera(CameraPayload::Snapshot { id: camera }),
    );
    sqlx::query("UPDATE cameras SET enabled=0 WHERE id=?")
        .bind(camera)
        .execute(&config.db)
        .await?;
    fake.clear();
    press(&fake, &config, &dialogue, q).await?;
    assert!(!fake
        .calls()
        .iter()
        .any(|(m, _)| m.eq_ignore_ascii_case("sendPhoto")));
    assert!(fake
        .calls()
        .iter()
        .any(|(m, body)| m.eq_ignore_ascii_case("answerWidgetQuery")
            && body.to_string().contains("denied")));
    Ok(())
}

#[tokio::test]
async fn camera_snapshot_action_sends_one_attachment_and_keeps_html_controls() -> Result<()> {
    let (config, camera) = camera_config().await?;
    let fake = FakeTelegram::new().await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/snapshot", listener.local_addr()?);
    sqlx::query("UPDATE cameras SET snapshot_url=? WHERE id=?")
        .bind(url)
        .bind(camera)
        .execute(&config.db)
        .await?;
    let snapshot_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        stream.read(&mut request).await.unwrap();
        let bytes = crate::bot::utils::UI_PLACEHOLDER_BYTES;
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).as_bytes()).await.unwrap();
        stream.write_all(bytes).await.unwrap();
    });
    let first = publish(
        &fake,
        &config,
        Payload::Camera(CameraPayload::CameraDetail { id: camera }),
    )
    .await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let q = action_query(
        &config,
        &Payload::Camera(CameraPayload::Snapshot { id: camera }),
    );
    fake.clear();
    press(&fake, &config, &dialogue, q.clone()).await?;
    // Replayed callbacks must not capture or send another photo.
    press(&fake, &config, &dialogue, q).await?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !fake
            .calls()
            .iter()
            .any(|(m, _)| m.eq_ignore_ascii_case("sendPhoto"))
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    snapshot_server.await?;
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|(m, _)| m.eq_ignore_ascii_case("sendPhoto"))
            .count(),
        1
    );
    assert_eq!(current(&config).message, first.message);
    assert_eq!(
        config.sessions.get(&1).unwrap().ui_message_mode,
        UiMessageMode::Html
    );
    Ok(())
}

#[tokio::test]
async fn camera_archive_thumbnails_keep_html_media_actions_and_recording_warnings() -> Result<()> {
    let (mut config, camera) = camera_config().await?;
    let suffix: String = sqlx::query_scalar("SELECT lower(hex(randomblob(16)))")
        .fetch_one(&config.db)
        .await?;
    let root = std::env::temp_dir().join(format!("ha-widget-camera-{suffix}"));
    std::fs::create_dir_all(&root)?;
    Arc::get_mut(&mut config)
        .unwrap()
        .camera_recording_storage_root = root.to_string_lossy().into();
    std::fs::write(root.join("clip.mp4"), b"test video")?;
    std::fs::write(
        root.join("clip.preview.jpg"),
        crate::bot::utils::UI_PLACEHOLDER_BYTES,
    )?;
    let rule =
        sqlx::query("INSERT INTO camera_recording_rules(name,camera_id) VALUES('Движение',?)")
            .bind(camera)
            .execute(&config.db)
            .await?
            .last_insert_rowid();
    let session = sqlx::query("INSERT INTO camera_recording_sessions(event_group_id,rule_id,camera_id,trigger_summary,status,first_event_at,last_event_at,stop_after_at,expires_at) VALUES('widget-test',?,?,'Движение в комнате','recording',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP,datetime('now','+1 minute'),datetime('now','+1 day'))")
        .bind(rule).bind(camera).execute(&config.db).await?.last_insert_rowid();
    let active = router(
        Payload::Camera(CameraPayload::CameraDetail { id: camera }),
        1,
        config.clone(),
    )
    .await?;
    let prepared = presentation::prepare(&active, "a".repeat(32), 1)?;
    assert!(prepared.pages[0].body.contains("Идёт запись"));
    assert!(prepared
        .actions
        .values()
        .any(|p| p == &Payload::Camera(CameraPayload::StopRecording { camera, session })));
    sqlx::query("UPDATE camera_recording_sessions SET status='failed',error='Test recording interrupted' WHERE id=?")
        .bind(session).execute(&config.db).await?;
    let segment = sqlx::query("INSERT INTO camera_recording_segments(session_id,camera_id,segment_index,file_path,duration_s,status,expires_at) VALUES(?,?,0,'clip.mp4',10,'ready',datetime('now','+1 day'))")
        .bind(session).bind(camera).execute(&config.db).await?.last_insert_rowid();
    for action in [
        CameraPayload::RecordingArchive { camera },
        CameraPayload::RecordingSession { camera, session },
    ] {
        let view = router(Payload::Camera(action), 1, config.clone()).await?;
        assert!(view.image.is_some(), "Test uses a cached thumbnail");
        let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
        assert_complete(&view, &prepared)?;
        if matches!(
            view.payload,
            Payload::Camera(CameraPayload::RecordingSession { .. })
        ) {
            assert!(prepared
                .pages
                .iter()
                .any(|p| p.body.contains("Test recording interrupted")));
            for action in [
                CameraPayload::SendRecordingSegment {
                    camera,
                    session,
                    segment,
                },
                CameraPayload::ToggleRecordingPin { camera, session },
                CameraPayload::ConfirmDeleteRecording { camera, session },
            ] {
                assert!(prepared
                    .actions
                    .values()
                    .any(|p| p == &Payload::Camera(action.clone())));
            }
        }
    }
    std::fs::remove_dir_all(root)?;
    Ok(())
}
