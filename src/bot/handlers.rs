use std::os::unix::raw::time_t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use crate::models::{AppConfig, UserSession};
use anyhow::Result;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ParseMode};
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

            let sent = bot.send_message(chat_id, "🏠 *Главное меню*")
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(super::keyboards::main_menu_hub())
                .await?;

            config.sessions.insert(user_id, UserSession {
                last_menu_id: sent.id.0,
                current_context: "hub".to_string(),
                header_entities: std::collections::HashSet::new(),
            });

            let context = "hub";
            crate::core::update_user_state(&config, user_id, sent.id.0, context).await;
        }
    }
    super::utils::spawn_delayed_delete(bot.clone(), chat_id, msg.id, 1);
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

    let notify = super::view::format_header(config.get_header_data(user_id).await);

    let header_text = "🏠 *ZEGBI SMART HOME*\n────────────────────\n\n";

    let (body_text, kb) = match context {
        "m" | "hub" => ("Выберите раздел:".to_string(), super::keyboards::main_menu_hub()),
        _ => ("Главное меню:".to_string(), super::keyboards::main_menu_hub()),
    };

    let full_text = format!("{}{}{}", header_text, notify, body_text);

    let res = bot.edit_message_text(chat_id, message_id, full_text)
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(kb)
        .await;

    if let Err(e) = res {
        handle_edit_error(bot, chat_id, user_id, config, e).await?;
    }

    Ok(())
}

async fn handle_edit_error(bot: &Bot, chat_id: ChatId, user_id: u64, config: &AppConfig, err: RequestError) -> Result<()> {
    match err {
        RequestError::Api(teloxide::ApiError::MessageNotModified) => Ok(()),
        _ => {
            // Если не удалось изменить (например, это был график-фото),
            // просто переотправляем меню (здесь логика send_exclusive_menu)
            Ok(())
        }
    }
}