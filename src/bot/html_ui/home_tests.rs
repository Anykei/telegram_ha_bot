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
                assert!(!first.body.is_empty() && first.context.is_empty() && first.back.is_none());
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
                assert_eq!(
                    prepared.pages.len(),
                    if filled { 2 } else { 1 },
                    "Short statuses need no extra page"
                );
                for item in &view.notifications {
                    assert!(first.body.contains(&item.label) && first.body.contains(&item.value));
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
async fn home_shows_states_counts_and_age_without_hiding_navigation() -> Result<()> {
    let (config, _) = mock_config().await?;
    for lang in [Language::Ru, Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        let mut view = router(Payload::Home, 1, config.clone()).await?;
        view.notifications = vec![crate::core::HeaderItem {
            icon: "🔕".into(),
            label: words(lang, "Коридор · Замок", "Hall · Lock").into(),
            value: words(lang, "Закрыта [x4]", "Closed [x4]").into(),
            last_update: chrono::Utc::now() - chrono::Duration::minutes(5),
            ..Default::default()
        }];
        let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
        assert_complete(&view, &prepared)?;
        assert_eq!(prepared.pages.len(), 1);
        assert_eq!(prepared.pages[0].rows.iter().flatten().count(), 7);
        let status = &view.notifications[0];
        let body = &prepared.pages[0].body;
        assert!(body.contains(&status.label) && body.contains(&status.value));
        assert!(body.contains(
            &crate::core::presentation::StateFormatter::format_last_update(status.last_update)
        ));

        view.notifications[0].value = words(lang, "Открыта [x5]", "Open [x5]").into();
        let updated = presentation::prepare(&view, "b".repeat(32), 2)?;
        assert!(updated.pages[0].body.contains(&view.notifications[0].value));
        assert!(!updated.pages[0].body.contains("[x4]"));
        assert_eq!(updated.pages.len(), 1);

        for index in 1..12 {
            let mut item = view.notifications[0].clone();
            item.label = format!("Датчик {index}");
            item.value = format!("Состояние {index}");
            view.notifications.push(item);
        }
        let expanded = presentation::prepare(&view, "c".repeat(32), 3)?;
        assert_complete(&view, &expanded)?;
        let first = &expanded.pages[0];
        assert_eq!(
            first
                .rows
                .iter()
                .flatten()
                .filter(|b| expanded.actions.contains_key(&b.key))
                .count(),
            7
        );
        let link = &first.rows[0][0];
        assert!(link.text.contains(&format!(
            "{}: 12",
            words(lang, "Все статусы", "All statuses")
        )));
        let index: usize = link.key.strip_prefix('p').unwrap().parse()?;
        assert_eq!(expanded.pages[index].back.as_deref(), Some("p0"));
        let full = expanded.pages[index..]
            .iter()
            .map(|p| p.body.as_str())
            .collect::<String>();
        for item in &view.notifications {
            assert!(full.contains(&item.label) && full.contains(&item.value));
        }
    }
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
    assert_eq!(initial.pages[0].rows.len(), 5);
    let status_link = &initial.pages[0].rows[0][0];
    assert!(status_link.text.contains("Все статусы: 2"));
    assert!(status_link.key.starts_with('p'));
    assert!(
        resolve(&fake.bot, &query(&initial, &status_link.key), &config)
            .await?
            .is_none()
    );
    let details = current(&config);
    assert_eq!(details.pages[details.index].title, "Сводка дома");
    assert!(resolve(&fake.bot, &query(&details, "p0"), &config)
        .await?
        .is_none());
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
