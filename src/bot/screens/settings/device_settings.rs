use crate::bot::models::View;
use crate::bot::router::{Payload, RenderContext, SettingsPayload};

use crate::db;
use anyhow::{Context, Result};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn render(ctx: RenderContext, room_id: i64, device_id: i64) -> Result<View> {
    let db = &ctx.config.db;

    let dev = db::devices::get_device_by_id(device_id, db)
        .await?
        .context("Device not found")?;

    let subscribed = db::subscriptions::is_subscribed(ctx.user_id as i64, &dev.entity_id, db)
        .await
        .unwrap_or(false);
    let hidden = db::subscriptions::is_hidden(&dev.entity_id, db)
        .await
        .unwrap_or(false);
    let critical = db::devices::is_device_critical(&dev.entity_id, db)
        .await
        .unwrap_or(false);
    let can_control = db::access::can_control_device(ctx.user_id, ctx.is_admin, device_id, db)
        .await
        .unwrap_or(false);

    let ha_ent = ctx
        .config
        .ha_client
        .fetch_states_by_ids(std::slice::from_ref(&dev.entity_id))
        .await?
        .into_iter()
        .next()
        .context("HA offline")?;

    let status_text = crate::core::presentation::StateFormatter::translate_state(&ha_ent.state);

    let mut text = format!(
        "⚙️ Параметры\n\n\
        🛠 *Настройки устройства*\n\n\
        Имя: `{}`\n\
        ID: `{}`\n\
        Entity: `{}`\n\
        Статус: {}\n\
        Критичное: {}\n\
        ────────────────────\n\
        Настройте поведение устройства в боте:",
        dev.alias.as_deref().unwrap_or(&dev.entity_id),
        device_id,
        dev.entity_id,
        status_text,
        if critical { "да" } else { "нет" }
    );

    if !can_control {
        text.push_str("\n        Доступ: только просмотр");
    }

    let mut rows = vec![];

    let (sub_icon, sub_label) = if subscribed {
        ("🔔", "Уведомления: ВКЛ")
    } else {
        ("🔕", "Уведомления: ВЫКЛ")
    };
    rows.push(vec![InlineKeyboardButton::callback(
        format!("{} {}", sub_icon, sub_label),
        Payload::Settings(SettingsPayload::ToggleNotify {
            room: room_id,
            device: device_id,
        })
        .to_string(),
    )]);

    if ctx.is_admin {
        let (hide_icon, hide_label) = if hidden {
            ("👁", "Показать в управлении")
        } else {
            ("🚫", "Скрыть из управления")
        };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} {}", hide_icon, hide_label),
            Payload::Settings(SettingsPayload::ToggleHide {
                room: room_id,
                device: device_id,
            })
            .to_string(),
        )]);

        rows.push(vec![InlineKeyboardButton::callback(
            "✏️ Изменить имя",
            Payload::Settings(SettingsPayload::EditName {
                room: room_id,
                device: device_id,
            })
            .to_string(),
        )]);

        rows.push(vec![InlineKeyboardButton::callback(
            "🏷 Алиасы состояний",
            Payload::Settings(SettingsPayload::StateAliases {
                room: room_id,
                device: device_id,
            })
            .to_string(),
        )]);

        let (critical_icon, critical_label) = if critical {
            ("🛡", "Критичное: ДА")
        } else {
            ("⚪", "Критичное: НЕТ")
        };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{} {}", critical_icon, critical_label),
            Payload::Settings(SettingsPayload::ToggleCritical {
                room: room_id,
                device: device_id,
            })
            .to_string(),
        )]);
    }

    // Кнопка "Назад"
    rows.push(vec![InlineKeyboardButton::callback(
        "⬅️ Назад к списку",
        Payload::Settings(SettingsPayload::RoomDetail { room: room_id }).to_string(),
    )]);

    Ok(View {
        // header: Some("⚙️ Параметры".into()),
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Settings(SettingsPayload::DeviceDetail {
            room: room_id,
            device: device_id,
        }),
        ..Default::default()
    })
}

pub async fn render_state_aliases(
    ctx: RenderContext,
    room_id: i64,
    device_id: i64,
) -> Result<View> {
    let db = &ctx.config.db;

    let dev = db::devices::get_device_by_id(device_id, db)
        .await?
        .context("Device not found")?;
    let ha_ent = ctx
        .config
        .ha_client
        .fetch_states_by_ids(std::slice::from_ref(&dev.entity_id))
        .await?
        .into_iter()
        .next()
        .context("HA offline")?;

    let domain = dev.entity_id.split('.').next().unwrap_or("");
    let class = ha_ent.device_class.as_deref().unwrap_or("");
    let inverted = db::devices::is_state_inverted(&dev.entity_id, db)
        .await
        .unwrap_or(false);
    let logical_state =
        crate::core::presentation::StateFormatter::logical_state(&ha_ent.state, inverted);
    let aliases = db::devices::get_state_aliases_for_entity(&dev.entity_id, db).await?;
    let editable_states = editable_state_alias_states(&ha_ent.state);

    let current_display = crate::core::presentation::StateFormatter::format_state_value_with_alias(
        domain,
        class,
        &ha_ent.state,
        inverted,
        aliases.get(&ha_ent.state).map(String::as_str),
    );

    let aliases_text = if aliases.is_empty() {
        "Алиасы пока не заданы.".to_string()
    } else {
        aliases
            .iter()
            .map(|(state, alias)| format!("`{}` → {}", state, alias))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let editable_states_text = editable_states
        .iter()
        .map(|state| {
            let display = crate::core::presentation::StateFormatter::format_state_value_with_alias(
                domain,
                class,
                state,
                inverted,
                aliases.get(state).map(String::as_str),
            );
            if aliases.contains_key(state) {
                format!("`{}` → {}", state, display)
            } else {
                format!("`{}` → {} (по умолчанию)", state, display)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let inversion_label = if inverted {
        "🔁 Инверсия: ВКЛ"
    } else {
        "↔️ Инверсия: ВЫКЛ"
    };

    let text = format!(
        "🏷 *Алиасы состояний*\n\n\
        Устройство: `{}`\n\
        ID: `{}`\n\
        Entity: `{}`\n\
        Текущее состояние HA: `{}`\n\
        Логическое состояние: `{}`\n\
        На экране: {}\n\n\
        Алиасы:\n\
        {}\n\n\
        Состояния для настройки:\n\
        {}\n\n\
        Инверсия меняет смысл `on/off`, `open/closed`, `locked/unlocked` для отображения.",
        dev.alias.as_deref().unwrap_or(&dev.entity_id),
        device_id,
        dev.entity_id,
        ha_ent.state,
        logical_state,
        current_display,
        aliases_text,
        editable_states_text
    );

    let mut rows = vec![];

    if ctx.is_admin {
        for state in &editable_states {
            let edit_label = if aliases.contains_key(state) {
                format!("✏️ Изменить `{}`", state)
            } else {
                format!("➕ Задать `{}`", state)
            };
            rows.push(vec![InlineKeyboardButton::callback(
                edit_label,
                Payload::Settings(SettingsPayload::EditStateAlias {
                    room: room_id,
                    device: device_id,
                    state: state.clone(),
                })
                .to_string(),
            )]);

            if aliases.contains_key(state) {
                rows.push(vec![InlineKeyboardButton::callback(
                    format!("🧹 Сбросить `{}`", state),
                    Payload::Settings(SettingsPayload::ResetStateAlias {
                        room: room_id,
                        device: device_id,
                        state: state.clone(),
                    })
                    .to_string(),
                )]);
            }
        }

        rows.push(vec![InlineKeyboardButton::callback(
            inversion_label,
            Payload::Settings(SettingsPayload::ToggleStateInversion {
                room: room_id,
                device: device_id,
            })
            .to_string(),
        )]);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "⬅️ Назад к устройству",
        Payload::Settings(SettingsPayload::DeviceDetail {
            room: room_id,
            device: device_id,
        })
        .to_string(),
    )]);

    Ok(View {
        notifications: ctx.notifications,
        text,
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Settings(SettingsPayload::StateAliases {
            room: room_id,
            device: device_id,
        }),
        ..Default::default()
    })
}

fn editable_state_alias_states(current_state: &str) -> Vec<String> {
    let mut states = vec![current_state.to_string()];
    let pair = crate::core::presentation::StateFormatter::invert_state(current_state);

    if pair != current_state {
        states.push(pair);
    }

    states
}

#[cfg(test)]
mod tests {
    use super::editable_state_alias_states;

    #[test]
    fn editable_state_alias_states_adds_pair_for_binary_state() {
        assert_eq!(editable_state_alias_states("off"), vec!["off", "on"]);
        assert_eq!(
            editable_state_alias_states("locked"),
            vec!["locked", "unlocked"]
        );
    }

    #[test]
    fn editable_state_alias_states_keeps_single_unknown_state() {
        assert_eq!(
            editable_state_alias_states("unavailable"),
            vec!["unavailable"]
        );
    }
}
