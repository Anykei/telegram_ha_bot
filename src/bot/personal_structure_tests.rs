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
                        &format!("{}\n{}", control.text, favorite_management_context(&context))
                    );
                    assert!(html.lines().nth(1).unwrap().len() <= 100);
                    assert!(html.lines().nth(1).unwrap().starts_with(&format!(
                        "{}. ",
                        usize::from(page) * 10 + index + 1
                    )));
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
