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
