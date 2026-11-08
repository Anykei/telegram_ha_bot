//! A locked client may request a fresh Home widget once. Never replay its action.
use super::*;

/// Called under the user's UI lock. Recovery intentionally does not require the
/// old view's resources: those may have disappeared or become inaccessible.
pub(super) async fn home(bot: &Bot, query: &CallbackQuery, config: &Arc<AppConfig>) -> Result<()> {
    let user = query.from.id.0;
    let Some(message) = query.message.as_ref() else {
        return answer(bot, query, "denied").await;
    };
    if !is_widget(query)
        || !message.chat().is_private()
        || message.chat().id.0 != user as i64
        || !preferred(config, user).await
    {
        return answer(bot, query, "denied").await;
    }
    let Some(generation) = db::user::admission_generation(user, &config.db).await? else {
        return answer(bot, query, "denied").await;
    };
    let saved_message: Option<i32> = sqlx::query_scalar(
        "SELECT last_menu_id FROM users WHERE id = ? AND admission_generation = ?",
    )
    .bind(user as i64)
    .bind(&generation)
    .fetch_optional(&config.db)
    .await?
    .flatten();
    // Persisted identity also works after restart, when HTML sessions are gone
    // and restored ordinary sessions still have the historical Photo mode.
    if saved_message != Some(message.id().0)
        || config
            .sessions
            .get(&user)
            .is_some_and(|s| s.last_menu_id != message.id().0)
        || config
            .html_ui
            .sessions
            .get(&user)
            .is_some_and(|s| s.message != message.id().0 || s.generation != generation)
    {
        return answer(bot, query, "denied").await;
    }
    let mut view = super::super::router::router(Payload::Home, user, config.clone()).await?;
    view.admission_generation = Some(generation.clone());
    if handlers::view_admission(&view, config, user, message.chat().id)
        .await?
        .is_none()
    {
        return answer(bot, query, "denied").await;
    }
    answer(bot, query, "").await?;
    config.cancel_view_update(user);
    // A data edit cannot revive a closed JS runtime or change graph/range HTML.
    // Publish the replacement first; a failed delivery leaves the old anchor intact.
    if let Some(replacement) =
        deliver(bot, config, user, message.chat().id, None, &view, None).await?
    {
        crate::core::update_user_state_for_admission(
            config,
            user,
            replacement.0,
            &Payload::Home.to_string(),
            UiMessageMode::Html,
            &generation,
        )
        .await;
        if !config
            .sessions
            .get(&user)
            .is_some_and(|s| s.last_menu_id == replacement.0)
        {
            return Ok(());
        }
        let dialogue = config.active_dialogues.get(&user).map(|d| d.clone());
        if let Some(dialogue) = dialogue {
            // The authentication layer may register a dialogue without a saved
            // state. Reset works for both open input and an already idle chat.
            dialogue.reset().await?;
        }
        if let Some(mut session) = config.sessions.get_mut(&user) {
            session.recording_rule_wizard = None;
        }
        // Pending device/video operations are deliberately retained until their
        // real outcomes arrive; returning Home must not unlock duplicate writes.
        if let Err(error) = bot.delete_message(message.chat().id, message.id()).await {
            log::warn!(
                "HTML recovery: old widget cleanup failed for user {}: {}",
                user,
                error
            );
        }
    }
    Ok(())
}
