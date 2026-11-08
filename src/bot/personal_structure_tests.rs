use super::*;
use crate::bot::router::router;
use teloxide::types::InlineKeyboardButtonKind;

fn callback(button: &Button) -> &str {
    let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
        panic!("Expected callback button")
    };
    data
}

#[tokio::test]
async fn favorite_management_labels_identify_their_action_after_page_breaks() -> Result<()> {
    let config = crate::test_support::config().await;
    db::add_user(1, &config.db).await?;
    for index in 1..=12 {
        let id = db::action_groups::create_action_group(
            &format!("Длинное название сценария {index} · Свет гостиной"),
            &config.db,
        )
        .await?;
        db::personal::add_favorite(
            1,
            &serde_json::to_string(&FavoriteAction::GroupOn { id })?,
            &config.db,
        )
        .await?;
    }
    for lang in [Language::Ru, Language::En] {
        db::set_user_language(1, lang, &config.db).await?;
        for (page, count) in [(0, 10), (1, 2)] {
            let view = router(
                Payload::Personal(PersonalPayload::Favorites { page }),
                1,
                config.clone(),
            )
            .await?;
            for index in 0..count {
                let action = &view.kb.inline_keyboard[index * 2][0];
                let controls = &view.kb.inline_keyboard[index * 2 + 1];
                let context = format!("{}. {}", usize::from(page) * 10 + index + 1, action.text);
                assert_eq!(view.html_button_labels[callback(action)], context);
                for control in controls {
                    let html = &view.html_button_labels[callback(control)];
                    assert_eq!(
                        html,
                        &format!(
                            "{}\n{}",
                            control.text,
                            favorite_management_context(&context)
                        )
                    );
                    assert!(html.lines().nth(1).unwrap().len() <= 100);
                    assert!(html
                        .lines()
                        .nth(1)
                        .unwrap()
                        .starts_with(&format!("{}. ", usize::from(page) * 10 + index + 1)));
                    // The classic keyboard is unchanged; the HTML fallback
                    // carries context even when this row opens a new page.
                    assert!(!control.text.contains('\n'));
                    assert!(matches!(
                        Payload::from_string(callback(control))?,
                        Payload::Personal(
                            PersonalPayload::FirstFavorite { .. }
                                | PersonalPayload::RemoveFavorite { .. }
                        )
                    ));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn long_favorite_context_keeps_its_number_within_wire_budget() {
    let label = format!("12. {}", "Люстра 🛋️ ".repeat(40));
    let short = favorite_management_context(&label);
    assert!(short.starts_with("12. Люстра"));
    assert!(short.ends_with('…'));
    assert!(short.len() <= 100);
    assert_eq!(favorite_management_context("1. Свет"), "1. Свет");
}

async fn mock_config() -> Result<Arc<AppConfig>> {
    let mut config = crate::test_support::config().await;
    let (tx, _) = tokio::sync::mpsc::channel(10);
    let ha = Arc::new(crate::ha::mock::MockHa::open(config.db.clone(), tx).await?);
    ha.seed_inventory().await?;
    Arc::get_mut(&mut config).unwrap().ha_client = ha;
    db::add_user(1, &config.db).await?;
    Ok(config)
}

#[tokio::test]
async fn temperature_form_has_its_own_prompt_target_limits_and_cancel() -> Result<()> {
    let config = mock_config().await?;
    let (room, device): (i64, i64) =
        sqlx::query_as("SELECT room_id, id FROM devices WHERE entity_id = 'climate.bedroom'")
            .fetch_one(&config.db)
            .await?;
    for lang in [Language::Ru, Language::En] {
        db::set_user_language(1, lang, &config.db).await?;
        let view = router(
            Payload::TemperatureInput { room, device },
            1,
            config.clone(),
        )
        .await?;
        let parent = Payload::Control(ControlPayload::DeviceControl { room, device });
        assert_eq!(view.html_back, Some(parent.clone()));
        assert_eq!(view.html_kind.as_deref(), Some("form"));
        assert_eq!(
            view.html_title.as_deref(),
            Some(words(lang, "Задать температуру", "Set temperature"))
        );
        assert!(view.html_context.as_deref().unwrap().contains("Термостат"));
        assert!(view.text.contains("22°C"));
        assert!(view.text.contains("16°C – 28°C"));
        assert!(view.text.contains(words(
            lang,
            "Отправьте температуру в чат",
            "Send the temperature in chat"
        )));
        assert!(!view.text.contains(t(lang, "climate.choose_target")));
        assert!(!view.text.contains(t(lang, "climate.step")));
        assert_eq!(view.kb.inline_keyboard.len(), 1);
        assert_eq!(view.kb.inline_keyboard[0][0].text, t(lang, "common.cancel"));
        assert_eq!(
            Payload::from_string(callback(&view.kb.inline_keyboard[0][0]))?,
            parent
        );
        assert!(
            matches!(view.next_state, Some(State::ExactTemperature { room_id, device_id }) if room_id == room && device_id == device)
        );
    }
    Ok(())
}

#[tokio::test]
async fn quiet_hours_input_has_only_exit_actions_and_does_not_change_preferences() -> Result<()> {
    let config = mock_config().await?;
    db::personal::set_quiet(1, true, &config.db).await?;
    for lang in [Language::Ru, Language::En] {
        db::set_user_language(1, lang, &config.db).await?;
        let before = db::user_notification_schedule::get(1, &config.db)
            .await?
            .unwrap();
        let view = router(
            Payload::Personal(PersonalPayload::QuietInput),
            1,
            config.clone(),
        )
        .await?;
        assert_eq!(view.html_kind.as_deref(), Some("form"));
        assert_eq!(
            view.html_title.as_deref(),
            Some(words(lang, "Интервал тихих часов", "Quiet hours interval"))
        );
        assert_eq!(
            view.html_back,
            Some(Payload::Personal(PersonalPayload::Settings))
        );
        assert!(view.text.contains("HH:MM–HH:MM"));
        assert!(matches!(view.next_state, Some(State::PersonalQuietHours)));
        for button in view.kb.inline_keyboard.iter().flatten() {
            assert!(matches!(
                Payload::from_string(callback(button))?,
                Payload::Home | Payload::Personal(PersonalPayload::Settings)
            ));
        }
        let after = db::user_notification_schedule::get(1, &config.db)
            .await?
            .unwrap();
        assert_eq!(before.quiet_enabled, after.quiet_enabled);
        assert_eq!(before.quiet_from, after.quiet_from);
        assert_eq!(before.quiet_to, after.quiet_to);
        assert_eq!(before.timezone, after.timezone);
    }
    Ok(())
}

#[tokio::test]
async fn device_details_separate_short_state_and_actions_from_full_diagnostics() -> Result<()> {
    let config = mock_config().await?;
    let (room, device): (i64, i64) =
        sqlx::query_as("SELECT room_id, id FROM devices WHERE entity_id = 'light.floor_lamp'")
            .fetch_one(&config.db)
            .await?;
    for lang in [Language::Ru, Language::En] {
        db::set_user_language(1, lang, &config.db).await?;
        let view = router(Payload::DeviceDetails { room, device }, 1, config.clone()).await?;
        assert!(view.html_summary.as_deref().unwrap().contains("Торшер"));
        assert!(!view
            .html_summary
            .as_deref()
            .unwrap()
            .contains("EntityAttributes"));
        assert!(view.text.contains("light.floor_lamp"));
        assert!(view.text.contains("EntityAttributes"));
        assert_eq!(
            view.html_details_title.as_deref(),
            Some(words(lang, "Технические сведения", "Technical details"))
        );
        assert_eq!(
            view.html_back,
            Some(Payload::Control(ControlPayload::DeviceControl {
                room,
                device
            }))
        );
        let actions = view
            .kb
            .inline_keyboard
            .iter()
            .flatten()
            .map(|button| Payload::from_string(callback(button)))
            .collect::<Result<Vec<_>>>()?;
        assert!(actions.iter().any(|payload| matches!(
            payload,
            Payload::Personal(PersonalPayload::FavoriteOptions { .. })
        )));
        assert!(actions.iter().any(|payload| matches!(
            payload,
            Payload::Personal(PersonalPayload::SetSubscription { .. })
        )));
    }
    Ok(())
}
