use super::super::{
    models::View,
    personal::PersonalPayload,
    router::{AdminPayload, CameraPayload, CommandPayload, ControlPayload, Payload},
};
use crate::i18n::{t, Language};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use teloxide::types::InlineKeyboardButtonKind;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Button {
    pub text: String,
    pub key: String,
    pub kind: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub v: u8,
    pub rev: String,
    pub seq: u64,
    pub lang: String,
    pub title: String,
    pub context: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub body: String,
    pub notice: String,
    pub alert: String,
    #[serde(rename = "b", alias = "rows", with = "button_rows")]
    pub rows: Vec<Vec<Button>>,
    pub read: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<crate::charts::widget::WidgetChart>,
    pub prev: Option<String>,
    pub next: Option<String>,
    pub page: usize,
    pub pages: usize,
}

mod button_rows {
    use super::Button;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        rows: &[Vec<Button>],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|b| (&b.text, &b.key, &b.kind))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Vec<Button>>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireButton {
            Compact((String, String, String)),
            Legacy(Button),
        }
        Ok(Vec::<Vec<WireButton>>::deserialize(deserializer)?
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|b| match b {
                        WireButton::Compact((text, key, kind)) => Button { text, key, kind },
                        WireButton::Legacy(button) => button,
                    })
                    .collect()
            })
            .collect())
    }
}
impl Page {
    pub fn json(&self) -> Result<String> {
        let json = serde_json::to_string(self)?;
        ensure!(
            json.len() <= 1000 && !json.contains("\\u0000"),
            "HTML page exceeds Telegram data budget"
        );
        Ok(json)
    }
}

pub fn supported(view: &View) -> bool {
    if view.image.is_some() && view.html_chart.is_none() {
        return false;
    }
    // Every text screen uses the same server-issued callback contract. Access
    // control and command semantics belong to the existing router, not a UI whitelist.
    view.kb
        .inline_keyboard
        .iter()
        .flatten()
        .all(|button| matches!(button.kind, InlineKeyboardButtonKind::CallbackData(_)))
}

pub struct Prepared {
    pub pages: Vec<Page>,
    pub actions: HashMap<String, Payload>,
}
fn kind(payload: &Payload) -> &'static str {
    match payload {
        Payload::Control(ControlPayload::ConfirmAction { .. })
        | Payload::Command(CommandPayload::Confirm { .. })
        | Payload::Camera(CameraPayload::DeleteRecording { .. })
        | Payload::Admin(
            AdminPayload::DeleteCamera { .. }
            | AdminPayload::DeleteUser { .. }
            | AdminPayload::DeleteRecordingRule { .. }
            | AdminPayload::DeleteRecordingRuleGroup { .. }
            | AdminPayload::DeleteRecordingRuleGroupForRoom { .. }
            | AdminPayload::DeleteActionGroup { .. }
            | AdminPayload::DeleteActionSchedule { .. },
        ) => "danger",
        Payload::Control(ControlPayload::CancelAction { .. })
        | Payload::Command(CommandPayload::Cancel { .. } | CommandPayload::CancelChoice { .. }) => {
            "nav"
        }
        Payload::Control(ControlPayload::QuickAction { cmd, .. })
            if super::super::router::action_requires_control(cmd) =>
        {
            "action"
        }
        Payload::Personal(PersonalPayload::SetInterface { .. }) => "action",
        _ if payload == &payload.for_refresh() => "nav",
        _ => "action",
    }
}

fn screen_kind(view: &View) -> &str {
    if view.next_state.is_some() {
        return "form";
    }
    if view.kb.inline_keyboard.iter().flatten().any(|button| {
        matches!(&button.kind, InlineKeyboardButtonKind::CallbackData(data)
            if matches!(Payload::from_string(data), Ok(Payload::Control(ControlPayload::ConfirmAction { .. }) | Payload::Command(CommandPayload::Confirm { .. }))))
    }) {
        return "confirmation";
    }
    if let Some(kind @ ("menu" | "device" | "climate" | "confirmation" | "list")) =
        view.html_kind.as_deref()
    {
        return kind;
    }
    match view.payload {
        Payload::Home | Payload::Personal(_) | Payload::Admin(AdminPayload::ListActions) => "menu",
        Payload::DeviceDetails { .. } | Payload::Control(ControlPayload::DeviceControl { .. }) => {
            "device"
        }
        Payload::Control(ControlPayload::ReviewAction { .. }) => "confirmation",
        _ => "list",
    }
}
fn split_text(text: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    for ch in text.chars() {
        if part.len() + ch.len_utf8() > limit {
            parts.push(std::mem::take(&mut part));
        }
        part.push(ch);
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

fn append_text(
    mut current: Page,
    empty: &Page,
    parts: &mut Vec<Page>,
    field: &str,
    text: &str,
) -> Result<Page> {
    for part in split_text(text, 180) {
        let mut candidate = current.clone();
        let target = match field {
            "alert" => &mut candidate.alert,
            "notice" => &mut candidate.notice,
            _ => &mut candidate.body,
        };
        target.push_str(&part);
        let characters = candidate.body.chars().count()
            + candidate.alert.chars().count()
            + candidate.notice.chars().count()
            + candidate.hint.chars().count();
        if candidate.json().is_err() || characters > 200 {
            if !current.hint.is_empty()
                || !current.body.is_empty()
                || !current.alert.is_empty()
                || !current.notice.is_empty()
                || current.graph.is_some()
                || !current.rows.is_empty()
            {
                parts.push(current);
            }
            current = empty.clone();
            match field {
                "alert" => current.alert = part,
                "notice" => current.notice = part,
                _ => current.body = part,
            };
            current.json()?;
        } else {
            current = candidate;
        }
    }
    Ok(current)
}

pub fn prepare(view: &View, rev: String, seq: u64) -> Result<Prepared> {
    ensure!(supported(view), "Screen uses classic presentation");
    let english = view.lang == Language::En;
    let title = view
        .html_title
        .clone()
        .or_else(|| view.header.clone())
        .unwrap_or_else(|| {
            match &view.payload {
                Payload::Home => {
                    if english {
                        "My home"
                    } else {
                        "Мой дом"
                    }
                }
                Payload::Help => {
                    if english {
                        "Help"
                    } else {
                        "Помощь"
                    }
                }
                Payload::Personal(PersonalPayload::Interface) => {
                    if english {
                        "Interface"
                    } else {
                        "Вид интерфейса"
                    }
                }
                Payload::Personal(_) => {
                    if english {
                        "My settings"
                    } else {
                        "Мои настройки"
                    }
                }
                _ => {
                    if english {
                        "Home"
                    } else {
                        "Дом"
                    }
                }
            }
            .into()
        });
    let empty = Page {
        v: 1,
        rev,
        seq,
        lang: if english { "en" } else { "ru" }.into(),
        title,
        context: view.html_context.clone().unwrap_or_default(),
        kind: screen_kind(view).into(),
        hint: String::new(),
        body: String::new(),
        notice: String::new(),
        alert: String::new(),
        rows: Vec::new(),
        read: "read".into(),
        // The shared screen builder labels its existing parent callback with
        // common.back. Pin the last such button (the footer), without inventing
        // a parent route or losing list/wizard context. Keys keep their row order.
        back: view
            .kb
            .inline_keyboard
            .iter()
            .flatten()
            .enumerate()
            .filter(|(_, button)| button.text == t(view.lang, "common.back"))
            .map(|(index, _)| format!("a{index}"))
            .last(),
        prev: Some("p9999".into()),
        next: Some("p9999".into()),
        graph: None,
        page: 9999,
        pages: 9999,
    };
    // Reserve room for non-ASCII labels and pagination; oversized content falls back as a whole.
    ensure!(empty.json()?.len() <= 600, "Header is too large for HTML");
    let mut parts: Vec<Page> = Vec::new();
    let mut current = empty.clone();
    current.graph = view.html_chart.clone();
    current.json()?;
    if view.next_state.is_some() {
        current.hint = if english {
            "Reply with a message in this chat. /cancel to cancel."
        } else {
            "Ответьте сообщением в этот чат. /cancel — отмена."
        }
        .into();
    }
    let mut body = Vec::new();
    if view.text_page.is_some() {
        // Switching from a classic long screen preserves its source page and pager.
        body.push(view.get_plain_text());
    } else {
        // The graph renderer keeps its header compact; preserve the full room
        // context in the detail pages, including names too long for the header.
        if view.html_chart.is_some() {
            if let Some(context) = &view.html_context {
                body.push(context.clone());
            }
        }
        for item in &view.notifications {
            body.push(format!(
                "{} {}: {} · {}",
                item.icon,
                item.label,
                item.value,
                crate::core::presentation::StateFormatter::format_last_update(item.last_update)
            ));
        }
        if !view.text.is_empty() {
            body.push(view.text.clone());
        }
    }
    let body = body.join("\n\n");
    let summary = view
        .html_summary
        .as_deref()
        .filter(|_| view.text_page.is_none() && view.next_state.is_none());
    for (field, text) in [
        (
            "alert",
            if view.text_page.is_some() {
                ""
            } else {
                view.alert.as_deref().unwrap_or("")
            },
        ),
        (
            "notice",
            if view.text_page.is_some() {
                ""
            } else {
                view.notice.as_deref().unwrap_or("")
            },
        ),
        ("body", summary.unwrap_or(&body)),
    ] {
        current = append_text(current, &empty, &mut parts, field, text)?;
    }
    let mut actions = HashMap::new();
    for (row_index, row) in view.kb.inline_keyboard.iter().enumerate() {
        // Preserve every button, including source rows wider than the widget.
        // Graph time navigation (-day / current / +day) fits one 3-button row;
        // keeping it together leaves both rows of range presets on the first page.
        let columns = if view.html_chart.is_some() && row_index == 0 {
            3
        } else {
            2
        };
        for buttons in row.chunks(columns) {
            let mut rendered = Vec::new();
            for button in buttons {
                let InlineKeyboardButtonKind::CallbackData(data) = &button.kind else {
                    unreachable!()
                };
                let payload = Payload::from_string(data)?;
                let key = format!("a{}", actions.len());
                let pinned = empty.back.as_deref() == Some(&key)
                    || (button.text == t(view.lang, "control.refresh")
                        && payload == view.payload.for_refresh());
                if !pinned {
                    rendered.push(Button {
                        text: button.text.clone(),
                        key: key.clone(),
                        kind: if button.text.starts_with("✓ ") {
                            "selected"
                        } else {
                            kind(&payload)
                        }
                        .into(),
                    });
                }
                actions.insert(key, payload);
            }
            if rendered.is_empty() {
                continue;
            }
            let mut candidate = current.clone();
            candidate.rows.push(rendered.clone());
            let text_chars = candidate.body.chars().count()
                + candidate.notice.chars().count()
                + candidate.alert.chars().count()
                + candidate.hint.chars().count();
            if candidate.rows.len() > if current.graph.is_some() { 3 } else { 4 }
                || candidate.json().is_err()
                || (candidate.rows.len() > 2 && text_chars > 80)
            {
                parts.push(current);
                current = empty.clone();
                current.rows.push(rendered);
                current.json()?;
            } else {
                current = candidate;
            }
        }
    }
    if summary.is_some() && !body.is_empty() {
        parts.push(current);
        current = append_text(empty.clone(), &empty, &mut parts, "body", &body)?;
    }
    parts.push(current);
    ensure!(parts.len() <= 9999, "Too many HTML pages");
    let pages = parts.len();
    for (index, page) in parts.iter_mut().enumerate() {
        page.page = index + 1;
        page.pages = pages;
        page.prev = (index > 0).then(|| format!("p{}", index - 1));
        page.next = (index + 1 < pages).then(|| format!("p{}", index + 1));
        page.json()?;
    }
    Ok(Prepared {
        pages: parts,
        actions,
    })
}
