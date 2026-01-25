use chrono::{DateTime, Duration, Local, Utc};
use crate::db::rooms::Room;

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
        match state {
            "on" => "ВКЛ",
            "off" => "ВЫКЛ",
            "unavailable" => "Н/Д",
            "home" => "Дома",
            "not_home" => "Ушел",
            "locked" => "Закрыто",
            "unlocked" => "Открыто",
            _ => state, // Возвращаем как есть, если нет перевода
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

        Self::translate_state(state).to_string()
    }

    /// Собирает итоговую строку для кнопки или уведомления.
    /// Пример: "🌡 Кухня (22.50°C)"
    pub fn format_device_label_with_state(alias: &str, domain: &str, class: &str, state: &str) -> String {
        let icon = Self::get_icon(domain, class, state);
        let value = Self::format_state_value(domain, class, state);

        format!("{} {} ({})", icon, alias, value)
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
            super::types::RoomViewMode::Settings => "⚙️ *Настройки*\nВыберите комнату для настройки:",
        }
    }

    pub fn format_event_line(
        room_name: Option<&str>,
        alias: &str,
        domain: &str,
        class: &str,
        state: &str,
        count: i32,
        seconds_ago: i64,
    ) -> String {
        let icon = Self::get_icon(domain, class, state);
        let human_state = Self::format_state_value(domain, class, state);

        // Хлебные крошки (комната)
        let room_part = room_name.map(|n| format!("*{}* • ", n)).unwrap_or_default();

        // Мета-информация (повторы и время)
        let mut meta = Vec::new();
        if count > 1 { meta.push(format!("⟲{}", count)); }
        if seconds_ago > 0 { meta.push(format!("{}с", seconds_ago)); }

        let meta_str = if meta.is_empty() {
            String::new()
        } else {
            format!(" _{}_", meta.join(" "))
        };

        format!("{}{} {}: *{}*{}", icon, room_part, alias, human_state, meta_str)
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
            local_dt.format("%d %b %H:%M").to_string()
        }
    }
}