use crate::bot::router::Payload;
use crate::bot::State;
use crate::core::HeaderItem;
use crate::i18n::{t, Language};
use chrono::Local;
use serde::{Deserialize, Serialize};
use teloxide::types::InlineKeyboardMarkup;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct HtmlSection {
    /// Start a named continuation at this source keyboard row.
    pub start_row: usize,
    pub title: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct View {
    #[serde(default)]
    pub displayed_resources: Vec<crate::core::types::DisplaySource>,
    #[serde(default)]
    pub admission_generation: Option<String>,
    pub header: Option<String>,
    /// Semantic title and context for compact cards; old serialized views remain valid.
    #[serde(default)]
    pub html_title: Option<String>,
    #[serde(default)]
    pub html_context: Option<String>,
    #[serde(default)]
    pub html_kind: Option<String>,
    /// Short confirmed state shown with controls; full text stays in details.
    #[serde(default)]
    pub html_summary: Option<String>,
    #[serde(default)]
    pub html_sections: Vec<HtmlSection>,
    /// Label deferred full text, with a return to the first page.
    #[serde(default)]
    pub html_details_title: Option<String>,
    #[serde(default)]
    pub html_chart: Option<crate::charts::widget::WidgetChart>,
    pub notifications: Vec<HeaderItem>,
    pub text: String,
    pub kb: InlineKeyboardMarkup,
    pub payload: Payload,
    pub next_state: Option<State>,
    pub alert: Option<String>,
    pub notice: Option<String>,
    pub image: Option<Vec<u8>>,
    pub lang: Language,
    #[serde(default)]
    pub compact: bool,
    #[serde(default)]
    pub rendered_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub text_page: Option<super::text_pages::TextPage>,
}

impl View {
    /// House-wide events belong on Home and its status continuation. Keep the
    /// original sources on View for admission checks even when they are hidden.
    pub(crate) fn visible_notifications(&self) -> &[HeaderItem] {
        if matches!(self.payload, Payload::Home)
            || self.text_page.is_some_and(|page| page.from_home)
        {
            &self.notifications
        } else {
            &[]
        }
    }

    pub fn get_text(&self) -> String {
        if self.text_page.is_some() {
            return super::utils::md::plain(&self.get_plain_text());
        }
        if self.compact {
            return super::utils::md::plain(&self.get_compact_text());
        }
        let header_title = self
            .header
            .as_deref()
            .map(super::utils::md::plain)
            .unwrap_or_else(|| format!("🏠 {}", super::utils::md::bold(t(self.lang, "app.title"))));
        let separator = "────────────────────";

        let mut status_lines = Vec::new();
        for item in self.visible_notifications() {
            // Используем &, чтобы не перемещать данные
            let safe_label = super::utils::md::plain(&item.label);
            let time_ago =
                crate::core::presentation::StateFormatter::format_last_update(item.last_update);

            status_lines.push(format!(
                "{} {}: {} {}",
                item.icon,
                safe_label,
                super::utils::md::bold(&item.value),
                super::utils::md::italic(&time_ago)
            ));
        }
        let status_block = status_lines.join("\n");

        let mut body_parts = Vec::new();

        if let Some(notice_msg) = &self.notice {
            body_parts.push(format!(
                "✅ {}\n{}",
                super::utils::md::bold(t(self.lang, "common.notice")),
                super::utils::md::italic(notice_msg)
            ));
        }

        if let Some(alert_msg) = &self.alert {
            body_parts.push(format!(
                "⚠️ {}\n{}",
                super::utils::md::bold(t(self.lang, "common.error")),
                super::utils::md::italic(alert_msg)
            ));
        }

        if !self.text.is_empty() {
            body_parts.push(super::utils::md::plain(&self.text));
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

        let refreshed_at = self
            .rendered_at
            .unwrap_or_else(chrono::Utc::now)
            .with_timezone(&Local)
            .format("%H:%M:%S")
            .to_string();
        final_parts.push(separator.to_string());
        final_parts.push(format!(
            "{} {}",
            super::utils::md::plain(t(self.lang, "common.updated")),
            super::utils::md::italic(&refreshed_at)
        ));

        final_parts.join("\n")
    }

    pub fn get_plain_text(&self) -> String {
        if let Some(page) = self.text_page {
            return super::text_pages::page_text(self, page.page);
        }
        self.get_full_plain_text()
    }

    pub(crate) fn get_full_plain_text(&self) -> String {
        if self.compact {
            return self.get_compact_text();
        }
        let header_title = self
            .header
            .clone()
            .unwrap_or_else(|| format!("🏠 {}", t(self.lang, "app.title")));
        let separator = "────────────────────";

        let mut status_lines = Vec::new();
        for item in self.visible_notifications() {
            let time_ago =
                crate::core::presentation::StateFormatter::format_last_update(item.last_update);

            status_lines.push(format!(
                "{} {}: {} {}",
                item.icon, item.label, item.value, time_ago
            ));
        }
        let status_block = status_lines.join("\n");

        let mut body_parts = Vec::new();

        if let Some(notice_msg) = &self.notice {
            body_parts.push(format!(
                "✅ {}\n{}",
                t(self.lang, "common.notice"),
                notice_msg
            ));
        }

        if let Some(alert_msg) = &self.alert {
            body_parts.push(format!(
                "⚠️ {}\n{}",
                t(self.lang, "common.error"),
                alert_msg
            ));
        }

        if !self.text.is_empty() {
            body_parts.push(self.text.clone());
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

        let refreshed_at = self
            .rendered_at
            .unwrap_or_else(chrono::Utc::now)
            .with_timezone(&Local)
            .format("%H:%M:%S")
            .to_string();
        final_parts.push(separator.to_string());
        final_parts.push(format!(
            "{} {}",
            t(self.lang, "common.updated"),
            refreshed_at
        ));

        final_parts.join("\n")
    }

    fn get_compact_text(&self) -> String {
        let mut parts = Vec::new();
        if let Some(header) = &self.header {
            parts.push(header.clone());
        }
        for item in self.visible_notifications() {
            let time_ago =
                crate::core::presentation::StateFormatter::format_last_update(item.last_update);
            parts.push(format!(
                "{} {}: {} {}",
                item.icon, item.label, item.value, time_ago
            ));
        }
        if let Some(notice) = &self.notice {
            parts.push(format!("✅ {notice}"));
        }
        if let Some(alert) = &self.alert {
            parts.push(format!("⚠️ {alert}"));
        }
        if !self.text.is_empty() {
            parts.push(self.text.clone());
        }
        if parts.is_empty() {
            parts.push(t(self.lang, "app.title").into());
        }
        parts.push(format!(
            "{} {}",
            t(self.lang, "common.updated"),
            self.rendered_at
                .unwrap_or_else(chrono::Utc::now)
                .with_timezone(&Local)
                .format("%H:%M:%S")
        ));
        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn house_status_is_home_only_in_all_text_presentations() {
        for payload in [Payload::Home, Payload::Help, Payload::InDev] {
            let home = matches!(payload, Payload::Home);
            for compact in [false, true] {
                let view = View {
                    payload: payload.clone(),
                    compact,
                    notifications: vec![HeaderItem {
                        label: "Общий статус".into(),
                        value: "Событие дома".into(),
                        ..Default::default()
                    }],
                    text: "Люстра включена".into(),
                    alert: Some("Ошибка устройства".into()),
                    notice: Some("Команда принята".into()),
                    ..Default::default()
                };
                for text in [view.get_text(), view.get_plain_text()] {
                    assert_eq!(text.contains("Событие дома"), home);
                    for item in ["Люстра включена", "Ошибка устройства", "Команда принята"]
                    {
                        assert!(text.contains(item));
                    }
                }
                assert_eq!(
                    view.notifications.len(),
                    1,
                    "Keep sources for admission checks"
                );
            }
        }
    }

    #[test]
    fn compact_view_keeps_system_status_and_alerts_without_separators() {
        let mut view = View {
            compact: true,
            text: "Кухня".into(),
            ..Default::default()
        };
        view.notifications = vec![
            HeaderItem {
                source: None,
                additional_sources: Vec::new(),
                icon: "✅".into(),
                label: t(Language::Ru, "system.label").into(),
                value: t(Language::Ru, "system.ok").into(),
                last_update: chrono::Utc::now(),
            },
            HeaderItem {
                source: None,
                additional_sources: Vec::new(),
                icon: "🔴".into(),
                label: "Дверь".into(),
                value: "Открыто (2)".into(),
                last_update: chrono::Utc::now(),
            },
        ];
        let text = view.get_plain_text();
        assert!(text.contains("🔴 Дверь: Открыто (2)"));
        assert!(text.contains("✅ Система: Все спокойно"));
        assert!(text.contains("только что"));
        assert!(text.contains("Меню обновлено"));
        assert!(!text.contains("──"));
        assert!(view.get_text().contains("Открыто \\(2\\)"));
    }

    #[test]
    fn view_text_contains_visible_refresh_time() {
        let text = View::default().get_text();

        assert!(text.contains("Меню обновлено:"));
    }

    #[test]
    fn notice_is_not_rendered_as_error() {
        let text = View {
            notice: Some("Язык пользователя изменен".to_string()),
            ..Default::default()
        }
        .get_text();

        assert!(text.contains("ГОТОВО:"));
        assert!(!text.contains("ОШИБКА:"));
    }

    #[test]
    fn plain_text_view_does_not_markdown_escape_body() {
        let text = View {
            text: "sensor.kitchen_temp (avg)_1!".to_string(),
            ..Default::default()
        }
        .get_plain_text();

        assert!(text.contains("sensor.kitchen_temp (avg)_1!"));
        assert!(!text.contains("sensor\\.kitchen"));
    }
}
