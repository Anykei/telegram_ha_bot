use serde::{Serialize, Deserialize};
use teloxide::types::InlineKeyboardMarkup;
use crate::bot::router::Payload;
use crate::core::HeaderItem;

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub enum State {
    #[default]
    Idle,
    WaitingForName { entity_id: String, room_name: String },
    WaitingForStateAlias { entity_id: String, original_state: String, room_name: String },
    WaitingForGraphInterval { entity_id: String, room_name: String },
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct View {
    pub header: Option<String>,
    pub notifications: Vec<HeaderItem>,
    pub text: String,
    pub kb: InlineKeyboardMarkup,
    pub payload: Payload,

    pub alert: Option<String>,
}

impl View {
    pub fn get_text(&self) -> String {
        let header_title = self.header.as_deref().unwrap_or("🏠 *ZEGBI SMART HOME*");
        let separator = "────────────────────";

        let mut status_lines = Vec::new();
        for item in &self.notifications { // Используем &, чтобы не перемещать данные
            let safe_label = super::utils::escape_m2(&item.label);
            let time_ago = crate::core::presentation::StateFormatter::format_last_update(item.last_update);

            status_lines.push(format!(
                "{} {}: {} _{}_",
                item.icon, safe_label, item.value, time_ago
            ));
        }
        let status_block = status_lines.join("\n");

        let mut body_parts = Vec::new();

        if let Some(alert_msg) = &self.alert {
            body_parts.push(format!("⚠️ *ОШИБКА:*\n_{}_", super::utils::escape_m2(alert_msg)));
        }

        if !self.text.is_empty() {
            body_parts.push(super::utils::escape_m2(&self.text));
        }

        let mut final_parts = Vec::new();

        final_parts.push(format!("{}\n{}", header_title, separator));

        if !status_block.is_empty() {
            final_parts.push(status_block);
            final_parts.push(separator.to_string());
        }

        let body_content = body_parts.join("\n\n");
        if !body_content.is_empty() {
            final_parts.push(body_content);
        }

        final_parts.join("\n")
    }
}

