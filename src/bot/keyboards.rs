use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use crate::ha::Room;

pub enum MenuMode { Control, Configure }

pub fn main_menu_hub() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback("🏠 Управление", "m_ctrl")],
        vec![InlineKeyboardButton::callback("⚙️ Настройки", "m_cfg")],
        vec![InlineKeyboardButton::callback("🛠 Админка", "adm_list")],
    ])
}