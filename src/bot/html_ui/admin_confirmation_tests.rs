use super::*;

#[tokio::test]
async fn administrative_confirmation_read_keeps_question_and_cancel_is_read_only() -> Result<()> {
    let (config, _) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, _) = device(&config, "light.floor_lamp").await?;
    let camera = db::cameras::add_manual_camera(db::cameras::NewCamera {
        name: "Confirmation camera", room_id: room, stream_url: "mock://camera",
        snapshot_url: None, clip_seconds: 10,
    }, &config.db).await?;
    for route in [
        Payload::Admin(AdminPayload::ConfirmDeleteCamera { room, camera }),
        Payload::Admin(AdminPayload::ConfirmBackup),
    ] {
        let view = router(route.clone(), 1, config.clone()).await?;
        assert_eq!(view.payload.for_refresh(), route);
        let back = view.html_back.clone().context("Cancel route")?;
        assert_ne!(back, route);
        let rendered = presentation::prepare(&view, "c".repeat(32), 1)?;
        assert_eq!(rendered.pages[0].kind, "confirmation");
        assert_eq!(rendered.actions.get(rendered.pages[0].back.as_ref().unwrap()), Some(&back));
        publish(&fake, &config, route.clone()).await?;
        let snapshot = current(&config);
        let read = resolve(&fake.bot, &query(&snapshot, "read"), &config).await?.context("Read route")?;
        assert_eq!(Payload::from_string(&read)?, route);
        let refreshed = router(Payload::from_string(&read)?, 1, config.clone()).await?;
        assert_eq!(refreshed.text, view.text);
        assert_eq!(refreshed.kb, view.kb);
        router(back, 1, config.clone()).await?;
        assert!(db::cameras::get_camera(camera, &config.db).await?.is_some());
    }
    assert!(!fake.calls().iter().any(|(method, _)| method == "sendDocument"));
    Ok(())
}
