use super::*;
use crate::bot::router::router;

#[tokio::test]
async fn large_group_keeps_commands_before_deferred_inventory() -> Result<()> {
    let config = crate::test_support::config().await;
    db::add_user(1, &config.db).await?;
    db::rooms::sync_rooms_from_ha("living", "Гостиная", &config.db).await?;
    let group = db::action_groups::create_action_group("Вечерний свет", &config.db).await?;
    for index in 1..=14 {
        let entity = format!("light.lamp_{index}");
        db::devices::sync_device(
            &entity,
            "living",
            &format!("Лампа_{index}"),
            "light",
            &config.db,
        )
        .await?;
        let device: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE entity_id = ?")
            .bind(entity)
            .fetch_one(&config.db)
            .await?;
        db::action_groups::toggle_action_group_item(group, device, &config.db).await?;
    }
    for lang in [Language::Ru, Language::En] {
        db::set_user_language(1, lang, &config.db).await?;
        for payload in [
            Payload::Control(ControlPayload::ActionGroupDetail { group }),
            Payload::Admin(AdminPayload::ActionGroupDetail { group }),
        ] {
            let view = router(payload, 1, config.clone()).await?;
            let summary = view.html_summary.as_deref().unwrap();
            assert!(summary.contains("14/14"));
            assert!(!summary.contains("Лампа_"));
            assert!(summary.lines().count() <= 2);
            assert!(view.text.contains("Лампа_1"));
            assert!(view.text.contains(t(lang, "action_groups.and_more")));
            assert!(view.html_details_title.is_some());
            assert!(view
                .kb
                .inline_keyboard
                .iter()
                .flatten()
                .any(|button| button.text == t(lang, "action_groups.turn_on_all")));
            assert!(view
                .kb
                .inline_keyboard
                .iter()
                .flatten()
                .any(|button| button.text == t(lang, "action_groups.turn_off_all")));
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_action_keeps_execution_before_deferred_metadata() -> Result<()> {
    let config = crate::test_support::config().await;
    db::add_user(1, &config.db).await?;
    db::action_groups::sync_ha_native_targets(
        &[db::action_groups::HaNativeDiscovery {
            domain: "scene".into(),
            entity_id: "scene.evening".into(),
            ha_name: "Вечерний сценарий".into(),
        }],
        true,
        &config.db,
    )
    .await?;
    let action: i64 =
        sqlx::query_scalar("SELECT id FROM ha_native_targets WHERE entity_id = 'scene.evening'")
            .fetch_one(&config.db)
            .await?;
    for lang in [Language::Ru, Language::En] {
        db::set_user_language(1, lang, &config.db).await?;
        for payload in [
            Payload::Control(ControlPayload::HaNativeActionDetail { action }),
            Payload::Admin(AdminPayload::HaNativeActionDetail { action }),
        ] {
            let view = router(payload, 1, config.clone()).await?;
            let summary = view.html_summary.as_deref().unwrap();
            assert!(summary.contains("Home Assistant · scene"));
            assert!(!summary.contains("Entity:"));
            assert!(view.text.contains("Entity: scene.evening"));
            assert!(view.html_details_title.is_some());
            assert_eq!(
                view.kb.inline_keyboard[0][0].text,
                t(lang, "action_groups.execute")
            );
        }
    }
    Ok(())
}
