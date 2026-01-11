use teloxide::Bot;
use teloxide::macros::BotCommands;
use crate::models::{AppConfig};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode};
use teloxide::types::CallbackQueryId;
use teloxide::dispatching::dialogue::{InMemStorage, Dialogue};
use teloxide::RequestError;

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
) -> anyhow::Result<()> {
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };

    let user_id = user.id.0;
    let chat_id = msg.chat.id;

    let _ = bot.delete_message(chat_id, msg.id).await;

    info!("Received command: {:?} from user {} ===", cmd, user_id);

    Ok(())
}