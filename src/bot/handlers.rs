use teloxide::Bot;
use teloxide::macros::BotCommands;
use anyhow::{Context, Result};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ParseMode};
use teloxide::RequestError;

use crate::bot::router::{router, Payload};
use crate::models::{AppConfig};
use super::models::View;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Доступные команды:")]
#[derive(Debug)]
pub enum Command {
    #[command(description = "Show general menu")]
    Start,
}

pub async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    config: Arc<AppConfig>,
) -> Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };

    let user_id = user.id.0;
    let chat_id = msg.chat.id;

    info!("Received command: {:?} from user {} ===", cmd, user_id);

    match cmd {
        Command::Start => {
            if let Some(old_msg) = config.sessions.get(&user_id).map(|s| s.last_menu_id) {
                let _ = bot.delete_message(chat_id, MessageId(old_msg)).await;
            }

            let view = router(Payload::Home {}, user_id, config.clone()).await?;
            send_new_view(&bot, chat_id, user_id, view, config).await?;
        }
    }

    super::utils::spawn_delayed_delete(bot.clone(), chat_id, msg.id, 1);
    Ok(())
}

pub async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    config: Arc<AppConfig>,
) -> Result<()> {
    let data = q.data.as_ref().context("No data")?;
    let user_id = q.from.id.0;
    let chat_id = q.message.as_ref().context("No msg")?.chat().id;
    let message_id = q.message.as_ref().context("No msg")?.id();

    debug!("Received callback data {}, from user {}", data, user_id);
    
    let payload: Payload = serde_json::from_str(data)
        .map_err(|e| { error!("Parse error: {}", e); e })?;

    let view = router(payload, user_id, config.clone()).await?;

    update_view(&bot, chat_id, message_id, user_id, view, config).await?;

    let _ = bot.answer_callback_query(q.id).await;
    Ok(())
}

pub async fn render_current_view(
    bot: &Bot,
    config: &Arc<AppConfig>,
    user_id: u64,
    chat_id: ChatId,
    message_id: MessageId,
    context: &str
) -> Result<()> {
    let payload: Payload = serde_json::from_str(context)
        .map_err(|e| { error!("Parse error: {}", e); e })?;

    let view = router(payload, user_id, config.clone()).await?;

    update_view(&bot, chat_id, message_id, user_id, view, config.clone()).await?;

    Ok(())
}

async fn send_new_view(
    bot: &Bot,
    chat_id: ChatId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>
) -> Result<()> {
    let sent = bot.send_message(chat_id, view.get_text())
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(view.kb)
        .await?;

    crate::core::update_user_state(&config, user_id, sent.id.0, view.payload.to_string().as_str()).await;
    Ok(())
}

async fn update_view(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    user_id: u64,
    view: View,
    config: Arc<AppConfig>
) -> Result<()> {
    let res = bot.edit_message_text(chat_id, message_id, view.get_text())
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(view.kb.clone())
        .await;

    match res {
        Ok(_) => {
            crate::core::update_user_state(&config, user_id, message_id.0, view.payload.to_string().as_str()).await;
        }
        Err(RequestError::Api(teloxide::ApiError::MessageNotModified)) => {
            debug!("Message not modified");
        }
        Err(_) => {
            send_new_view(bot, chat_id, user_id, view, config).await?;
        }
    }
    Ok(())
}