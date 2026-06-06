use crate::db::rooms::Room;
use chrono::{DateTime, Datelike, Duration, Local, Utc};

pub struct StateFormatter;

impl Room {
    /// Возвращает иконку для комнаты, основываясь на её имени или алиасе
    pub fn icon(&self) -> &'static str {
        // Сначала пробуем взять имя из алиаса, если его нет — из технического area
        let name_for_icon = self.alias.as_deref().unwrap_or(&self.area);

        StateFormatter::get_room_icon(name_for_icon)
    }

    /// Возвращает красивое имя для отображения (с иконкой)
    /// Например: "🍳 Кухня"
    pub fn display_name(&self) -> String {
        let name = self.alias.as_deref().unwrap_or(&self.area);
        format!("{} {}", self.icon(), name)
    }
}

impl StateFormatter {
    fn short_month_ru(month: u32) -> &'static str {
        match month {
            1 => "янв",
            2 => "фев",
            3 => "мар",
            4 => "апр",
            5 => "мая",
            6 => "июн",
            7 => "июл",
            8 => "авг",
            9 => "сен",
            10 => "окт",
            11 => "ноя",
            12 => "дек",
            _ => "",
        }
    }

    fn format_ru_day_month_time(dt: DateTime<Local>) -> String {
        format!(
            "{} {} {}",
            dt.format("%d"),
            Self::short_month_ru(dt.month()),
            dt.format("%H:%M")
        )
    }

    pub fn invert_state(state: &str) -> String {
        match state {
            "on" => "off",
            "off" => "on",
            "true" => "false",
            "false" => "true",
            "open" => "closed",
            "closed" => "open",
            "locked" => "unlocked",
            "unlocked" => "locked",
            _ => state,
        }
        .to_string()
    }

    pub fn logical_state(state: &str, inverted: bool) -> String {
        if inverted {
            Self::invert_state(state)
        } else {
            state.to_string()
        }
    }

    pub fn format_state_value_with_alias(
        domain: &str,
        class: &str,
        state: &str,
        inverted: bool,
        alias: Option<&str>,
    ) -> String {
        if let Some(alias) = alias {
            return alias.to_string();
        }

        let logical_state = Self::logical_state(state, inverted);
        Self::format_state_value(domain, class, &logical_state)
    }

    pub fn format_device_label_with_state_alias(
        alias: &str,
        domain: &str,
        class: &str,
        state: &str,
        inverted: bool,
        state_alias: Option<&str>,
    ) -> String {
        let logical_state = Self::logical_state(state, inverted);
        let icon = Self::get_icon(domain, class, &logical_state);
        let value =
            Self::format_state_value_with_alias(domain, class, state, inverted, state_alias);

        format!("{} {} ({})", icon, alias, value)
    }

    /// Возвращает иконку устройства на основе его домена, класса и текущего состояния.
    pub fn get_icon(domain: &str, class: &str, state: &str) -> &'static str {
        match (domain, state) {
            ("light", "on") => "💡",
            ("light", _) => "🌑",

            ("switch", "on") => "🔌",
            ("switch", _) => "⚪",

            ("binary_sensor", "on") => "🔔",
            ("binary_sensor", _) => "🔕",

            ("climate", _) => "🌡",

            ("sensor", _) => match class {
                "temperature" => "🌡",
                "humidity" => "💧",
                "battery" => "🔋",
                "power" => "⚡",
                _ => "📊",
            },

            ("media_player", "playing") => "▶️",
            ("media_player", "paused") => "⏸",
            ("media_player", _) => "🔈",

            _ => "📦",
        }
    }

    /// Переводит техническое состояние Home Assistant на человеческий русский язык.
    pub fn translate_state(state: &str) -> &str {
        Self::translate_state_for("", "", state)
    }

    pub fn translate_state_for<'a>(domain: &str, class: &str, state: &'a str) -> &'a str {
        match (domain, class, state) {
            ("binary_sensor", "door" | "window" | "opening" | "garage_door", "on") => "Открыто",
            ("binary_sensor", "door" | "window" | "opening" | "garage_door", "off") => "Закрыто",
            ("binary_sensor", "motion" | "occupancy" | "presence", "on") => "Обнаружено",
            ("binary_sensor", "motion" | "occupancy" | "presence", "off") => "Нет",
            ("binary_sensor", _, "on") => "Активно",
            ("binary_sensor", _, "off") => "Неактивно",
            (_, _, "on") => "ВКЛ",
            (_, _, "off") => "ВЫКЛ",
            (_, _, "unavailable") => "Н/Д",
            (_, _, "home") => "Дома",
            (_, _, "not_home") => "Ушел",
            (_, _, "locked") => "Закрыто",
            (_, _, "unlocked") => "Открыто",
            _ => state,
        }
    }

    /// Финальная сборка всей строки кнопки
    pub fn format_device_label(alias: &str, domain: &str, class: &str, state: &str) -> String {
        let icon = Self::get_icon(domain, class, state);
        format!("{} {}", icon, alias)
    }

    pub fn format_state_value(domain: &str, class: &str, state: &str) -> String {
        if let Ok(val) = state.parse::<f64>() {
            let rounded = format!("{:.2}", val);

            return match domain {
                "climate" => format!("{}°C", rounded),
                "sensor" => match class {
                    "temperature" => format!("{}°C", rounded),
                    "humidity" => format!("{}%", rounded),
                    "battery" => format!("{}%", rounded),
                    "power" => format!("{} W", rounded),
                    "energy" => format!("{} kWh", rounded),
                    "voltage" => format!("{} V", rounded),
                    _ => rounded,
                },
                _ => rounded,
            };
        }

        Self::translate_state_for(domain, class, state).to_string()
    }

    pub fn get_room_icon(name: &str) -> &'static str {
        match name.to_lowercase().as_str() {
            "кухня" => "🍳",
            "спальня" => "🛌",
            "ванная" => "🛀",
            "коридор" => "🧥",
            "туалет" => "🚽",
            "гостиная" => "🛋",
            "детская" => "🧸",
            "кабинет" => "🖥",
            _ => "🚪", // Дефолтная иконка
        }
    }

    pub fn get_rooms_header(mode: &super::types::RoomViewMode) -> &'static str {
        match mode {
            super::types::RoomViewMode::Control => "🎮 *Управление*\nВыберите комнату:",
            super::types::RoomViewMode::Settings => {
                "⚙️ *Настройки*\nВыберите комнату для настройки:"
            }
        }
    }

    pub fn format_last_update(dt: DateTime<Utc>) -> String {
        let now = Utc::now();
        let diff = now - dt;
        let seconds = diff.num_seconds();

        if seconds < 60 {
            if seconds < 15 {
                return "только что".to_string();
            }
            let rounded_seconds = (seconds / 15) * 15;
            return format!("{}с назад", rounded_seconds);
        }

        if diff < Duration::hours(1) {
            return format!("{}м назад", diff.num_minutes());
        }

        let local_dt = dt.with_timezone(&Local);
        if local_dt.date_naive() == Local::now().date_naive() {
            local_dt.format("%H:%M").to_string()
        } else {
            Self::format_ru_day_month_time(local_dt)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StateFormatter;

    #[test]
    fn binary_door_states_are_human_readable() {
        assert_eq!(
            StateFormatter::format_state_value("binary_sensor", "door", "on"),
            "Открыто"
        );
        assert_eq!(
            StateFormatter::format_state_value("binary_sensor", "door", "off"),
            "Закрыто"
        );
    }

    #[test]
    fn switch_states_keep_generic_labels() {
        assert_eq!(
            StateFormatter::format_state_value("switch", "", "on"),
            "ВКЛ"
        );
        assert_eq!(
            StateFormatter::format_state_value("switch", "", "off"),
            "ВЫКЛ"
        );
    }
}
