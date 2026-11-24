use super::*;
use crate::bot::help::Topic;
use crate::i18n::Language;

#[tokio::test]
async fn help_topics_fit_one_wire_page_and_preserve_read_only_navigation() -> Result<()> {
    let (config, ha) = super::super::config().await?;
    let mut fixtures = Vec::new();
    for lang in [Language::Ru, Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        let mut routes = vec![Payload::Help];
        routes.extend(Topic::ALL.map(Payload::HelpTopic));
        for route in routes {
            let encoded = route.to_string();
            assert_eq!(Payload::from_string(&encoded)?, route);
            assert!(encoded.len() <= 64);
            assert_eq!(route.for_refresh(), route);
            let view = router(route.clone(), 1, config.clone()).await?;
            let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
            assert_complete(&view, &prepared)?;
            assert_eq!(
                prepared.pages.len(),
                1,
                "{lang:?} {route:?}: help must be compact"
            );
            let page = &prepared.pages[0];
            let parent = if route == Payload::Help {
                Payload::Home
            } else {
                Payload::Help
            };
            assert_eq!(
                prepared.actions.get(page.back.as_ref().unwrap()),
                Some(&parent)
            );
            assert!(page.json()?.len() <= 1000);
            assert!(view.visible_notifications().is_empty());
            assert!(view.next_state.is_none());
            for action in prepared.actions.values() {
                assert_eq!(
                    action.for_refresh(),
                    *action,
                    "Help must not execute commands"
                );
            }
            if route == Payload::Help {
                for topic in Topic::ALL {
                    assert!(prepared
                        .actions
                        .values()
                        .any(|p| *p == Payload::HelpTopic(topic)));
                }
            }
            fixtures.push(
                json!({"id":format!("help-{lang:?}-{route:?}"),"renderer":"release","page":page}),
            );
            // Switching presentation must preserve the same topics and destinations.
            let mut classic = view.clone();
            crate::bot::text_pages::prepare_classic(&mut classic, 1, &config).await?;
            assert!(classic.text_page.is_none());
            assert_eq!(classic.kb, view.kb);
            assert!(classic.get_plain_text().contains(&view.text));
        }
    }
    assert!(ha.calls.lock().unwrap().is_empty());
    if let Some(path) = std::env::var_os("HA_WIDGET_HELP_PATH") {
        std::fs::write(path, serde_json::to_vec_pretty(&fixtures)?)?;
    }
    Ok(())
}

#[tokio::test]
async fn help_voice_guidance_obeys_global_and_user_permissions() -> Result<()> {
    let (mut config, _) = super::super::config().await?;
    db::add_user(2, &config.db).await?;
    db::user::set_user_language(2, Language::Ru, &config.db).await?;
    let route = Payload::HelpTopic(Topic::Input);
    let view = router(route.clone(), 2, config.clone()).await?;
    assert!(view.text.contains("Голосовой ввод вам недоступен"));
    Arc::get_mut(&mut config).unwrap().voice_enabled = true;
    let view = router(route.clone(), 2, config.clone()).await?;
    assert!(view.text.contains("можно отправить голосовое сообщение"));
    db::access::toggle_user_voice_access(2, &config.db).await?;
    let view = router(route, 2, config.clone()).await?;
    assert!(view.text.contains("Голосовой ввод вам недоступен"));
    assert!(view.text.contains("/cancel"));
    Ok(())
}

#[tokio::test]
async fn disabled_html_marks_classic_selected_without_changing_saved_preference() -> Result<()> {
    let (config, _) = super::super::config().await?;
    let interface = Payload::Personal(PersonalPayload::Interface);
    let generation = db::admission_generation(1, &config.db).await?.unwrap();
    config.html_ui.delivery_failed(1, &generation);
    let fallback = router(interface.clone(), 1, config.clone()).await?;
    let labels: Vec<_> = fallback
        .kb
        .inline_keyboard
        .iter()
        .flatten()
        .map(|button| button.text.as_str())
        .collect();
    assert!(labels.contains(&"✓ Обычный"));
    assert!(labels.contains(&"HTML"));
    router(
        Payload::Personal(PersonalPayload::SetInterface {
            mode: db::interface::InterfaceMode::Html,
        }),
        1,
        config.clone(),
    )
    .await?;
    assert!(preferred(&config, 1).await);

    config.html_ui.disable_widget_updates();
    for lang in [Language::Ru, Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        let view = router(interface.clone(), 1, config.clone()).await?;
        let selections: Vec<_> = view
            .kb
            .inline_keyboard
            .iter()
            .flatten()
            .filter_map(|button| {
                let teloxide::types::InlineKeyboardButtonKind::CallbackData(data) = &button.kind
                else {
                    return None;
                };
                match Payload::from_string(data).ok()? {
                    Payload::Personal(PersonalPayload::SetInterface { mode }) => {
                        Some((mode, button.text.as_str()))
                    }
                    _ => None,
                }
            })
            .collect();
        assert_eq!(
            selections,
            [(
                db::interface::InterfaceMode::Classic,
                crate::bot::personal::words(lang, "✓ Обычный", "✓ Classic")
            )]
        );
        assert!(view.text.contains(crate::bot::personal::words(
            lang,
            "HTML-интерфейс недоступен",
            "The HTML interface is unavailable"
        )));
        assert_eq!(
            db::interface::get(1, &config.db).await?,
            db::interface::InterfaceMode::Html
        );
        let navigation = router(Payload::HelpTopic(Topic::Navigation), 1, config.clone()).await?;
        assert!(!navigation.text.contains("/html"));
        assert!(navigation.text.contains("/ui"));
    }
    Ok(())
}
