use super::*;

#[tokio::test]
async fn ordinary_screens_keep_explicit_titles_and_exit_on_every_page() -> Result<()> {
    let (config, _) = mock_config().await?;
    let (room, device) = device(&config, "climate.bedroom").await?;
    let routes = [
        (Payload::Help, Payload::Home),
        (
            Payload::TemperatureInput { room, device },
            Payload::Control(ControlPayload::DeviceControl { room, device }),
        ),
        (
            Payload::AdminSettings(SettingsPayload::DeviceDetail { room, device }),
            Payload::AdminSettings(SettingsPayload::RoomDetail { room }),
        ),
        (
            Payload::AdminSettings(SettingsPayload::StateAliases { room, device }),
            Payload::AdminSettings(SettingsPayload::DeviceDetail { room, device }),
        ),
        (
            Payload::Personal(PersonalPayload::QuietInput),
            Payload::Personal(PersonalPayload::Settings),
        ),
        (
            Payload::Admin(AdminPayload::UserRooms { id: 1 }),
            Payload::Admin(AdminPayload::UserProfile { id: 1 }),
        ),
    ];
    for lang in [crate::i18n::Language::Ru, crate::i18n::Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        for (route, parent) in &routes {
            let mut view = router(route.clone(), 1, config.clone()).await?;
            // Navigation must survive translation changes and a long continuation.
            for button in view.kb.inline_keyboard.iter_mut().flatten() {
                if matches!(&button.kind, teloxide::types::InlineKeyboardButtonKind::CallbackData(data)
                    if Payload::from_string(data).ok().as_ref() == Some(parent))
                {
                    button.text = "A deliberately different localized exit".into();
                }
            }
            view.text.push_str(&"\nLong read-only context. ".repeat(35));
            let prepared = presentation::prepare(&view, "c".repeat(32), 1)?;
            assert!(prepared.pages.len() > 1);
            for page in &prepared.pages {
                assert!(!matches!(page.title.as_str(), "Дом" | "Home"));
                let key = page.back.as_deref().context("Every page needs an exit")?;
                if key != "p0" {
                    assert_eq!(prepared.actions.get(key), Some(parent), "{route:?}");
                }
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn administrative_and_personal_titles_describe_the_actual_screen() -> Result<()> {
    let (config, _) = mock_config().await?;
    for (route, title) in [
        (
            Payload::Admin(AdminPayload::ListActions),
            "Администрирование",
        ),
        (Payload::Admin(AdminPayload::ListUsers), "Пользователи"),
        (
            Payload::Admin(AdminPayload::UserProfile { id: 1 }),
            "Профиль пользователя",
        ),
        (
            Payload::Personal(PersonalPayload::Notifications { page: 0 }),
            "Мои уведомления",
        ),
        (
            Payload::Personal(PersonalPayload::Favorites { page: 0 }),
            "Избранное",
        ),
    ] {
        let view = router(route, 1, config.clone()).await?;
        let prepared = presentation::prepare(&view, "c".repeat(32), 1)?;
        assert_eq!(prepared.pages[0].title, title);
    }
    Ok(())
}
