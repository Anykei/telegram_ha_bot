use super::*;

#[tokio::test]
async fn graph_pages_keep_history_controls_and_native_image_for_classic_mode() -> Result<()> {
    let (config, _) = mock_config().await?;
    for lang in [crate::i18n::Language::Ru, crate::i18n::Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        for entity in ["sensor.bedroom_humidity", "binary_sensor.kitchen_window"] {
            let (room, device) = device(&config, entity).await?;
            let view = router(
                Payload::Control(ControlPayload::DeviceControl { room, device }),
                1,
                config.clone(),
            )
            .await?;
            assert!(view.image.is_some(), "Classic graph keeps its full image");
            let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
            assert_complete(&view, &prepared)?;
            assert!(prepared
                .pages
                .iter()
                .map(|p| p.body.as_str())
                .collect::<String>()
                .contains(view.html_context.as_deref().unwrap()));
            let first = &prepared.pages[0];
            assert_eq!(first.body, view.html_summary.as_deref().unwrap());
            assert_eq!(first.graph.as_ref().unwrap().plot.len(), 96);
            assert!(prepared.pages.iter().skip(1).all(|p| p.graph.is_none()));
            assert!(first.back.is_some());
            for hours in [12, 24, 72, 168] {
                assert!(
                    first.rows.iter().flatten().any(|b| prepared.actions[&b.key]
                        == Payload::Control(ControlPayload::QuickAction {
                            room,
                            device,
                            cmd: DeviceCmd::ShowChart { h: hours, o: 0 }
                        })),
                    "Range {hours} missing from first graph page"
                );
            }
        }
    }
    let (room, device) = device(&config, "sensor.bedroom_humidity").await?;
    sqlx::query("DELETE FROM ha_mock_history WHERE entity_id='sensor.bedroom_humidity'")
        .execute(&config.db)
        .await?;
    let view = router(
        Payload::Control(ControlPayload::DeviceControl { room, device }),
        1,
        config.clone(),
    )
    .await?;
    assert!(view.image.is_none());
    let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
    assert_eq!(
        prepared.pages[0].graph.as_ref().unwrap().plot,
        ".".repeat(96)
    );
    assert_complete(&view, &prepared)?;
    Ok(())
}

#[tokio::test]
async fn graph_navigation_reanchors_only_when_renderer_changes_and_keeps_custom_range() -> Result<()>
{
    let (config, _) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    let (room, device) = device(&config, "sensor.bedroom_humidity").await?;
    let original = publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::RoomDetail { room }),
    )
    .await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    let open = Payload::Control(ControlPayload::DeviceControl { room, device });
    press(&fake, &config, &dialogue, action_query(&config, &open)).await?;
    let graph = current(&config);
    assert_ne!(
        graph.message, original.message,
        "Graph needs its own immutable HTML renderer"
    );
    assert!(graph.pages[0].graph.is_some());
    assert_eq!(
        fake.calls()
            .iter()
            .rev()
            .find(|(method, _)| method == "sendWidget")
            .unwrap()
            .1["html"],
        GRAPH_HTML
    );
    for (h, o) in [(12, 0), (12, -24), (168, -24)] {
        let range = Payload::Control(ControlPayload::QuickAction {
            room,
            device,
            cmd: DeviceCmd::ShowChart { h, o },
        });
        press(&fake, &config, &dialogue, action_query(&config, &range)).await?;
        assert_eq!(current(&config).view.payload, range);
        assert_eq!(current(&config).message, graph.message);
        let session = current(&config);
        for hours in [12, 24, 72, 168] {
            assert!(session.pages[0]
                .rows
                .iter()
                .flatten()
                .any(|b| session.actions[&b.key]
                    == Payload::Control(ControlPayload::QuickAction {
                        room,
                        device,
                        cmd: DeviceCmd::ShowChart { h: hours, o }
                    })));
        }
    }
    let before = current(&config);
    press(&fake, &config, &dialogue, query(&before, "read")).await?;
    assert_eq!(current(&config).view.payload, before.view.payload);
    assert_eq!(current(&config).message, graph.message);

    let custom = Payload::Control(ControlPayload::QuickAction {
        room,
        device,
        cmd: DeviceCmd::EnterManualInput,
    });
    press(&fake, &config, &dialogue, action_query(&config, &custom)).await?;
    assert!(matches!(
        dialogue.get().await?,
        Some(crate::bot::State::WaitingForGraphInterval { .. })
    ));
    let form = current(&config);
    assert!(form.view.html_chart.is_none());
    assert_ne!(form.message, graph.message);
    handlers::handle_custom_interval(
        fake.bot.clone(),
        input_message("48")?,
        config.clone(),
        dialogue.clone(),
        (device, room),
    )
    .await?;
    wait_idle(&config).await?;
    let restored = current(&config);
    assert!(restored.view.html_chart.is_some());
    assert_ne!(restored.message, form.message);
    assert_eq!(
        restored.view.payload,
        Payload::Control(ControlPayload::QuickAction {
            room,
            device,
            cmd: DeviceCmd::ShowChart { h: 48, o: -24 }
        })
    );
    press(
        &fake,
        &config,
        &dialogue,
        query(&restored, restored.pages[0].back.as_deref().unwrap()),
    )
    .await?;
    assert_eq!(
        current(&config).view.payload,
        Payload::Control(ControlPayload::RoomDetail { room })
    );
    assert!(current(&config).view.html_chart.is_none());
    assert_eq!(
        fake.calls()
            .iter()
            .rev()
            .find(|(method, _)| method == "sendWidget")
            .unwrap()
            .1["html"],
        HTML
    );
    assert!(!fake
        .calls()
        .iter()
        .any(|(method, _)| method.eq_ignore_ascii_case("sendPhoto")));
    fake.clear();
    assert!(resolve(&fake.bot, &query(&graph, "read"), &config)
        .await?
        .is_none());
    assert!(
        fake.calls()
            .iter()
            .any(|(method, body)| method == "answerWidgetQuery"
                && body["data"] == "{\"denied\":true}")
    );
    db::interface::set(1, db::interface::InterfaceMode::Classic, &config.db).await?;
    fake.clear();
    let view = router(open, 1, config.clone()).await?;
    assert!(
        handlers::send_new_view_inner(&fake.bot, ChatId(1), 1, view, config.clone(), None).await?
    );
    assert_eq!(
        config.sessions.get(&1).unwrap().ui_message_mode,
        UiMessageMode::Photo
    );
    assert!(fake
        .calls()
        .iter()
        .any(|(method, _)| method.eq_ignore_ascii_case("sendPhoto")));
    assert!(!fake
        .calls()
        .iter()
        .any(|(method, _)| method == "sendWidget"));
    Ok(())
}
