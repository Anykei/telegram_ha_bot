use super::*;
use crate::bot::personal::{words, FavoriteAction};
use crate::i18n::Language;

async fn add_favorites(config: &AppConfig, user: u64) -> Result<()> {
    let (room, lamp) = device(config, "light.floor_lamp").await?;
    let (bedroom, climate) = device(config, "climate.bedroom").await?;
    db::access::set_room_access(user, room, true, true, &config.db).await?;
    db::access::set_room_access(user, bedroom, true, true, &config.db).await?;
    for action in [
        FavoriteAction::Device { room, device: lamp },
        FavoriteAction::DeviceOn { room, device: lamp },
        FavoriteAction::DeviceOff { room, device: lamp },
        FavoriteAction::Device {
            room: bedroom,
            device: climate,
        },
    ] {
        db::personal::add_favorite(user, &serde_json::to_string(&action)?, &config.db).await?;
    }
    Ok(())
}

#[tokio::test]
async fn home_keeps_main_sections_on_first_page_with_or_without_favorites() -> Result<()> {
    let (config, ha) = mock_config().await?;
    db::add_user(42, &config.db).await?;
    let before = ha.fetch_states_by_ids(&["light.floor_lamp".into()]).await?[0]
        .state
        .clone();
    let main = vec![
        Payload::Control(ControlPayload::ListRooms),
        Payload::Control(ControlPayload::ActionGroups),
        Payload::Camera(CameraPayload::ListCameras),
        Payload::Personal(PersonalPayload::Favorites { page: 0 }),
        Payload::Personal(PersonalPayload::Settings),
        Payload::Help,
    ];
    for filled in [false, true] {
        for user in [1, 42] {
            if filled {
                add_favorites(&config, user).await?;
            }
            for lang in [Language::Ru, Language::En] {
                db::user::set_user_language(user, lang, &config.db).await?;
                let view = router(Payload::Home, user, config.clone()).await?;
                let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
                assert_complete(&view, &prepared)?;
                let first = &prepared.pages[0];
                assert_eq!(first.title, words(lang, "Мой дом", "My home"));
                assert!(first.body.is_empty() && first.context.is_empty() && first.back.is_none());
                let mut expected = main.clone();
                if user == 1 {
                    expected.push(Payload::Admin(AdminPayload::ListActions));
                }
                assert_eq!(
                    first.rows.iter().flatten().map(|b| prepared.actions[&b.key].clone()).collect::<Vec<_>>(),
                    expected,
                    "All main sections must be on the first page for user {user}, {lang:?}, favorites={filled}"
                );
                assert_eq!(
                    prepared.actions.len(),
                    expected.len() + if filled { 4 } else { 0 }
                );
                if user != 1 {
                    assert!(!prepared
                        .actions
                        .values()
                        .any(|p| matches!(p, Payload::Admin(_))));
                }
                let favorites = prepared
                    .pages
                    .iter()
                    .skip(1)
                    .filter(|p| !p.rows.is_empty())
                    .collect::<Vec<_>>();
                assert_eq!(
                    favorites.iter().map(|p| p.rows.len()).sum::<usize>(),
                    if filled { 4 } else { 0 }
                );
                for page in favorites {
                    assert_eq!(page.title, words(lang, "Избранное", "Favorites"));
                    assert_eq!(page.context, first.title);
                    assert_eq!(page.back.as_deref(), Some("p0"));
                }
                let summary = prepared
                    .pages
                    .iter()
                    .filter(|p| !p.body.is_empty())
                    .collect::<Vec<_>>();
                assert!(!summary.is_empty());
                for page in &summary {
                    assert_eq!(page.title, words(lang, "Сводка дома", "Home overview"));
                    assert_eq!(page.context, first.title);
                    assert_eq!(page.back.as_deref(), Some("p0"));
                }
                let text = summary.iter().map(|p| p.body.as_str()).collect::<String>();
                for item in &view.notifications {
                    assert!(text.contains(&item.label) && text.contains(&item.value));
                }
                // Classic mode still exposes the same complete keyboard and home status.
                assert!(view.get_plain_text().contains(&first.title));
                assert!(view.get_plain_text().contains(&view.notifications[0].value));
            }
        }
    }
    assert_eq!(
        ha.fetch_states_by_ids(&["light.floor_lamp".into()]).await?[0].state,
        before
    );
    Ok(())
}

#[tokio::test]
async fn home_continuations_return_to_menu_without_replaying_favorite_actions() -> Result<()> {
    let (config, ha) = mock_config().await?;
    add_favorites(&config, 1).await?;
    let fake = FakeTelegram::new().await?;
    let mut view = router(Payload::Home, 1, config.clone()).await?;
    // A long live summary must never push the main navigation off the first page.
    let long_status = "Подробное состояние датчика. ".repeat(35);
    view.notifications.push(crate::core::HeaderItem {
        label: "Датчики".into(),
        value: long_status.clone(),
        last_update: chrono::Utc::now(),
        ..Default::default()
    });
    let before = ha.fetch_states_by_ids(&["light.floor_lamp".into()]).await?[0]
        .state
        .clone();
    assert!(
        handlers::send_new_view_inner(&fake.bot, ChatId(1), 1, view, config.clone(), None).await?
    );
    let initial = current(&config);
    assert_eq!(initial.pages[0].rows.len(), 4);
    assert!(initial
        .pages
        .iter()
        .map(|p| p.body.as_str())
        .collect::<String>()
        .contains(&long_status));
    for index in 1..initial.pages.len() {
        let session = current(&config);
        assert!(
            resolve(&fake.bot, &query(&session, &format!("p{index}")), &config)
                .await?
                .is_none()
        );
        let session = current(&config);
        assert_eq!(session.index, index);
        let key = session.pages[index]
            .back
            .as_deref()
            .context("Missing return to home menu")?;
        assert!(resolve(&fake.bot, &query(&session, key), &config)
            .await?
            .is_none());
        let home = current(&config);
        assert_eq!(home.index, 0);
        assert_eq!(home.message, initial.message);
        assert_eq!(home.view.payload, Payload::Home);
        assert!(!home.claimed);
    }
    assert_eq!(
        ha.fetch_states_by_ids(&["light.floor_lamp".into()]).await?[0].state,
        before
    );
    Ok(())
}
