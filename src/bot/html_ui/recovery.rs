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
    // Persisted identity also works after restart, before the HTML action map is rebuilt.
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
    let token = config.begin_view_update(user);
    // A data edit cannot revive a closed JS runtime or change graph/range HTML.
    // Publish the replacement first; a failed delivery leaves the old anchor intact.
    let result = async {
        if !handlers::send_new_view_inner(
            bot,
            message.chat().id,
            user,
            view,
            config.clone(),
            Some(&token),
        )
        .await?
        {
            return Ok(());
        }
        let reset = handlers::reset_view_dialogue(config, user, &generation, &token).await;
        // Pending device/video operations are deliberately retained until their
        // real outcomes arrive; returning Home must not unlock duplicate writes.
        if let Err(error) = bot.delete_message(message.chat().id, message.id()).await {
            log::warn!(
                "HTML recovery: old widget cleanup failed for user {}: {}",
                user,
                error
            );
        }
        reset
    }
    .await;
    config.finish_view_update(user, &token);
    result
}
