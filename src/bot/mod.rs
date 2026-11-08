pub(crate) mod command_choices;
pub(crate) mod confirmations;
pub(crate) mod format;
pub(crate) mod handlers;
pub(crate) mod html_ui;
pub(crate) mod models;
pub(crate) mod notification;
pub(crate) mod personal;
pub(crate) mod recording_rule_wizard;
pub(crate) mod router;
pub(crate) mod schedule_drafts;
pub(crate) mod screen_structure;
mod screens;
pub(crate) mod text_commands;
pub(crate) mod text_pages;
pub(crate) mod utils;
pub(crate) mod view_access;

pub(crate) use crate::bot::router::State;
use crate::db;
use crate::models::AppConfig;
use std::sync::Arc;
use std::time::Duration;
use teloxide::net;
use teloxide::types::UpdateKind;
use teloxide::{
    dispatching::{dialogue::InMemStorage, UpdateHandler},
    prelude::*,
    types::Update,
};

/// Инициализирует экземпляр бота.
pub fn init(token: String) -> Bot {
    let client = net::default_reqwest_settings()
        .timeout(Duration::from_secs(90))
        .build()
        .expect("Telegram HTTP client creation failed");

    Bot::with_client(token, client)
}

/// Строит дерево обработки обновлений (Update Hierarchy).
/// Соответствует Google Standard: разделение ответственности между уровнями фильтрации.
pub fn schema() -> UpdateHandler<anyhow::Error> {
    // Reject non-private updates before dialogue access, metadata, or data loading.
    let private_filter = dptree::filter_async(|update: Update, bot: Bot| async move {
        if is_verified_private_update(&update) {
            return true;
        }
        let notice = "Откройте личный чат с ботом / Open a private chat with the bot";
        match update.kind {
            UpdateKind::CallbackQuery(query) => {
                let _ = html_ui::reject(&bot, &query, notice).await;
            }
            UpdateKind::Message(message) => {
                let _ = bot.send_message(message.chat.id, notice).await;
            }
            _ => {}
        }
        false
    });
    let auth_filter = dptree::filter_map_async(
        |update: Update, config: Arc<AppConfig>, storage: Arc<InMemStorage<State>>, bot: Bot| async move {
            use teloxide::dispatching::dialogue::Storage;
            let user = update.from()?;
            let user_id = user.id.0;
            let _lifecycle = config.admission_lifecycle.lock().await;
            if config.root_user == user_id {
                if let Err(error) = db::add_user(user_id, &config.db).await {
                    log::error!("Root initialization failed: {}", error);
                    return None;
                }
            }
            let Ok(Some(generation)) = db::user::admission_generation(user_id, &config.db).await
            else {
                let _ = storage
                    .clone()
                    .remove_dialogue(ChatId(user_id as i64))
                    .await;
                config.sessions.remove(&user_id);
                drop(_lifecycle);
                if let UpdateKind::CallbackQuery(query) = &update.kind {
                    let _ = html_ui::reject(
                        &bot,
                        query,
                        "Откройте личный чат с ботом / Open a private chat with the bot",
                    )
                    .await;
                }
                return None;
            };
            let same = config
                .dialogue_generations
                .get(&user_id)
                .is_some_and(|old| *old == generation);
            if !same {
                let _ = storage
                    .clone()
                    .remove_dialogue(ChatId(user_id as i64))
                    .await;
                config
                    .dialogue_generations
                    .insert(user_id, generation.clone());
            }
            if let Err(error) = db::user_display::capture(
                user_id,
                &user.first_name,
                user.last_name.as_deref(),
                user.username.as_deref(),
                &config.db,
            )
            .await
            {
                log::warn!("Could not save Telegram display metadata: {}", error);
            }
            config
                .active_dialogues
                .insert(user_id, Dialogue::new(storage, ChatId(user_id as i64)));
            drop(_lifecycle);
            match crate::core::record_user_activity_for_admission(&config, user_id, &generation)
                .await
            {
                Ok(true) => {}
                Ok(false) => return None,
                Err(error) => {
                    log::warn!("User activity update failed for {}: {}", user_id, error);
                    return None;
                }
            }
            Some(db::user::Admission {
                user_id,
                generation,
            })
        },
    );
    // 2. Ветка команд: обрабатывает системные команды (начинающиеся с /).
    let command_handler = Update::filter_message()
        .filter_command::<handlers::Command>()
        .endpoint(handlers::handle_command);

    // 3. Ветка Callback-запросов: обрабатывает нажатия инлайн-кнопок.
    let callback_handler = Update::filter_callback_query().endpoint(handlers::handle_callback);

    let voice_handler = Update::filter_message()
        .filter(|msg: Message| msg.voice().is_some())
        .endpoint(handlers::handle_voice_message);

    let text_command_handler = Update::filter_message()
        .filter(|state: State| matches!(state, State::Idle))
        .filter(|msg: Message| msg.text().is_some_and(handlers::looks_like_text_command))
        .endpoint(handlers::handle_text_command_message);

    // 4. Ветка Диалогов: обрабатывает текстовый ввод в зависимости от состояния.
    let message_dialogues = Update::filter_message()
        .branch(
            dptree::filter_map(|state: State| match state {
                State::ScheduleDraftTime { id, revision } => Some((id, revision)),
                _ => None,
            })
            .endpoint(schedule_drafts::handle_time),
        )
        .branch(
            dptree::filter(|state: State| {
                matches!(
                    state,
                    State::PersonalQuietHours | State::ExactTemperature { .. }
                )
            })
            .endpoint(personal::handle_input),
        )
        // Известные команды уже обработаны выше. Неизвестная команда в Idle
        // получает подсказку, но не становится значением активной формы.
        .filter(|msg: Message, state: State| accepts_dialogue_text(&state, msg.text()))
        .branch(
            dptree::filter_map(|state: State| match state {
                State::WaitingForName { device_id, room_id } => Some((device_id, room_id, None)),
                State::WaitingForNameDraft {
                    device_id,
                    room_id,
                    draft_id,
                } => Some((device_id, room_id, Some(draft_id))),
                _ => None,
            })
            .endpoint(handlers::handle_device_name_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::WaitingForStateAlias {
                    device_id,
                    original_state,
                    room_id,
                } => Some((device_id, original_state, room_id)),
                _ => None,
            })
            .endpoint(handlers::handle_state_alias_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::WaitingForGraphInterval { device_id, room_id } => Some((device_id, room_id)),
                _ => None,
            })
            .endpoint(handlers::handle_custom_interval),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::EditUserAlias { user_id } => Some(user_id),
                _ => None,
            })
            .endpoint(handlers::handle_user_alias_input),
        )
        .branch(
            dptree::filter(|state: State| matches!(state, State::AddUser { .. }))
                .endpoint(handlers::handle_add_user_input),
        )
        .branch(
            dptree::filter(|state: State| matches!(state, State::DeleteUser { .. }))
                .endpoint(handlers::handle_delete_user_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::AddCamera { room_id } => Some(room_id),
                _ => None,
            })
            .endpoint(handlers::handle_add_camera_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::AddRecordingRule { room_id } => Some(room_id),
                _ => None,
            })
            .endpoint(handlers::handle_add_recording_rule_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::AddRecordingRuleGroup { room_id } => Some(room_id),
                _ => None,
            })
            .endpoint(handlers::handle_add_recording_rule_group_input),
        )
        .branch(
            dptree::filter(|state: State| matches!(state, State::AddRecordingRuleGroupForWizard))
                .endpoint(handlers::handle_add_recording_rule_group_for_wizard_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::AddRecordingRuleGroupForEdit { room_id, rule_id } => {
                    Some((room_id, rule_id))
                }
                _ => None,
            })
            .endpoint(handlers::handle_add_recording_rule_group_for_edit_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::RenameRecordingRuleGroup { room_id, group_id } => Some((room_id, group_id)),
                _ => None,
            })
            .endpoint(handlers::handle_rename_recording_rule_group_input),
        )
        .branch(
            dptree::filter(|state: State| matches!(state, State::AddActionGroup))
                .endpoint(handlers::handle_add_action_group_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::RenameActionGroup { group_id } => Some(group_id),
                _ => None,
            })
            .endpoint(handlers::handle_rename_action_group_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::RenameHaNativeTarget { target_id } => Some(target_id),
                _ => None,
            })
            .endpoint(handlers::handle_ha_native_alias_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::AddActionScheduleTime { target, command } => Some((target, command)),
                _ => None,
            })
            .endpoint(handlers::handle_add_action_schedule_time_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::EditActionScheduleTime {
                    target,
                    schedule_id,
                } => Some((target, schedule_id)),
                _ => None,
            })
            .endpoint(handlers::handle_edit_action_schedule_time_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::EditRecordingRule { room_id, rule_id } => Some((room_id, rule_id)),
                _ => None,
            })
            .endpoint(handlers::handle_edit_recording_rule_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::EditRecordingRuleNumber {
                    room_id,
                    rule_id,
                    field,
                } => Some((room_id, rule_id, field)),
                _ => None,
            })
            .endpoint(handlers::handle_edit_recording_rule_number_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::EditRecordingRuleActiveTime { room_id, rule_id } => Some((room_id, rule_id)),
                _ => None,
            })
            .endpoint(handlers::handle_edit_recording_rule_active_time_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::EditRecordingRuleConditionValue {
                    room_id,
                    rule_id,
                    device_id,
                    operator,
                } => Some((room_id, rule_id, device_id, operator)),
                _ => None,
            })
            .endpoint(handlers::handle_edit_recording_rule_condition_value_input),
        )
        .branch(
            dptree::filter_map(|state: State| match state {
                State::RecordingRuleWizardSourceValue { mode } => Some(mode),
                _ => None,
            })
            .endpoint(handlers::handle_recording_rule_wizard_source_value_input),
        )
        .branch(
            dptree::filter(|state: State| {
                matches!(state, State::RecordingRuleWizardActiveTimeValue)
            })
            .endpoint(handlers::handle_recording_rule_wizard_active_time_input),
        )
        .branch(
            dptree::filter(|state: State| {
                matches!(state, State::RecordingRuleWizardConditionValue)
            })
            .endpoint(handlers::handle_recording_rule_wizard_condition_value_input),
        )
        .branch(
            dptree::filter_map(
                |state: State, msg: Message, config: Arc<AppConfig>| match state {
                    State::Idle => recording_rule_room_from_current_context(&msg, &config),
                    _ => None,
                },
            )
            .endpoint(handlers::handle_add_recording_rule_input),
        )
        // Поглощаем сообщения в состоянии Idle, чтобы они не падали в Unhandled Update.
        .branch(
            dptree::filter(|state: State| matches!(state, State::Idle))
                .endpoint(handlers::handle_idle_text),
        );

    // 5. Итоговое дерево (Main Entry Point)
    dptree::entry()
        .chain(private_filter)
        .chain(auth_filter)
        .chain(admission_scope())
        .enter_dialogue::<Update, InMemStorage<State>, State>()
        .branch(command_handler)
        .branch(callback_handler)
        .branch(voice_handler)
        .branch(text_command_handler)
        .branch(message_dialogues)
        .endpoint(|update: Update| async move {
            let user_id = update.from().map(|u| u.id.0).unwrap_or(0);

            let update_type = match &update.kind {
                UpdateKind::Message(_) => "Message",
                UpdateKind::CallbackQuery(_) => "CallbackQuery",
                UpdateKind::EditedMessage(_) => "EditedMessage",
                UpdateKind::InlineQuery(_) => "InlineQuery",
                _ => "Other",
            };

            log::warn!(
                "⚠️ Unhandled Update: ID={:?}, User={}, Type={}",
                update.id,
                user_id,
                update_type
            );

            Ok::<(), anyhow::Error>(())
        })
}

#[track_caller]
fn admission_scope() -> UpdateHandler<anyhow::Error> {
    dptree::from_fn(
        |input: dptree::di::DependencyMap, continuation| async move {
            let admission = (*input.get::<db::user::Admission>()).clone();
            db::user::with_admission(admission, async move { continuation(input).await }).await
        },
        // This middleware consumes the admission supplied by auth_filter and
        // continues with the same dependencies. Entry is only valid at the root.
        dptree::HandlerSignature::Other {
            obligations: [(
                dptree::Type::of::<db::user::Admission>(),
                std::panic::Location::caller(),
            )]
            .into(),
            guaranteed_outcomes: Default::default(),
            conditional_outcomes: Default::default(),
            continues: true,
        },
    )
}

fn is_verified_private_update(update: &Update) -> bool {
    let Some(user) = update.from() else {
        return false;
    };
    let message = match &update.kind {
        UpdateKind::Message(message) => Some(message),
        UpdateKind::CallbackQuery(query) => query
            .message
            .as_ref()
            .and_then(|message| message.regular_message()),
        _ => None,
    };
    message
        .is_some_and(|message| message.chat.is_private() && message.chat.id.0 == user.id.0 as i64)
}

fn accepts_dialogue_text(state: &State, text: Option<&str>) -> bool {
    matches!(state, State::Idle) || text.is_none_or(|text| !text.starts_with('/'))
}

fn recording_rule_room_from_current_context(msg: &Message, config: &Arc<AppConfig>) -> Option<i64> {
    let user_id = msg.from.as_ref()?.id.0;
    let text = msg.text()?.trim();
    if !looks_like_recording_rule_input(text) {
        return None;
    }

    let session = config.sessions.get(&user_id)?;
    match router::Payload::from_string(&session.current_context).ok()? {
        router::Payload::Admin(router::AdminPayload::RecordingRules { room }) => Some(room),
        _ => None,
    }
}

fn looks_like_recording_rule_input(text: &str) -> bool {
    let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
    lines >= 8 && text.contains(';')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn production_schema_builds_and_type_checks_with_dispatcher_dependencies(
    ) -> anyhow::Result<()> {
        let handler = schema();
        let config = crate::test_support::config().await;
        let client = net::default_reqwest_settings().no_proxy().build()?;
        let bot = Bot::with_client("123:test", client);
        // Match main's dependencies. build() checks all branches without
        // polling Telegram; Admission must come from auth_filter itself.
        let _dispatcher = Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![config, InMemStorage::<State>::new()])
            .build();
        Ok(())
    }

    #[test]
    #[should_panic(expected = "Admission")]
    fn admission_scope_requires_authentication_dependency() {
        dptree::type_check(admission_scope().sig(), &dptree::deps![], &[]);
    }

    #[tokio::test]
    async fn admission_scope_keeps_original_generation_through_continuation_and_child(
    ) -> anyhow::Result<()> {
        let config = crate::test_support::config().await;
        db::add_user(42, &config.db).await?;
        let generation = db::admission_generation(42, &config.db).await?.unwrap();
        let admission = db::user::Admission {
            user_id: 42,
            generation: generation.clone(),
        };
        let handler = dptree::filter(|| true).chain(admission_scope()).endpoint(
            |config: Arc<AppConfig>| async move {
                tokio::task::yield_now().await;
                db::delete_user(42, &config.db).await?;
                db::add_user(42, &config.db).await?;
                // A handler cannot adopt a newly granted admission after await.
                assert!(db::admission_generation(42, &config.db).await?.is_none());
                let child_config = config.clone();
                let child = db::user::spawn_with_admission(async move {
                    db::admission_generation(42, &child_config.db).await
                });
                assert!(child.await??.is_none());
                Ok(())
            },
        );
        let input = dptree::deps![config.clone(), admission];
        dptree::type_check(handler.sig(), &input, &[]);
        let std::ops::ControlFlow::Break(result) = handler.dispatch(input).await else {
            panic!("authenticated continuation was not executed");
        };
        result?;
        // The original scope ends with the request and cannot leak into another.
        let current = db::admission_generation(42, &config.db).await?.unwrap();
        assert_ne!(current, generation);
        Ok(())
    }

    #[test]
    fn protected_updates_require_a_verified_private_message() {
        for user in [1, 42] {
            for chat_type in ["private", "group", "supergroup", "channel"] {
                let value = serde_json::json!({"update_id":1,"message":{"message_id":1,"date":1700000000,"from":{"id":user,"is_bot":false,"first_name":"User"},"chat":{"id":user,"type":chat_type,"title":"Group"},"text":"/start"}});
                let update: Update = serde_json::from_str(&value.to_string()).unwrap();
                assert_eq!(
                    is_verified_private_update(&update),
                    chat_type == "private",
                    "{update:?}"
                );
                let value = serde_json::json!({"update_id":2,"callback_query":{"id":"1","from":{"id":user,"is_bot":false,"first_name":"User"},"chat_instance":"x","data":"camera","message":{"message_id":1,"date":1700000000,"chat":{"id":user,"type":chat_type,"title":"Group"},"text":"old"}}});
                let update: Update = serde_json::from_str(&value.to_string()).unwrap();
                assert_eq!(
                    is_verified_private_update(&update),
                    chat_type == "private",
                    "{update:?}"
                );
            }
            let update: Update = serde_json::from_str(&serde_json::json!({"update_id":3,"callback_query":{"id":"1","from":{"id":user,"is_bot":false,"first_name":"User"},"chat_instance":"x","data":"camera"}}).to_string()).unwrap();
            assert!(!is_verified_private_update(&update));
        }
    }

    #[test]
    fn unknown_slash_commands_reach_idle_feedback_without_entering_active_forms() {
        assert!(accepts_dialogue_text(&State::Idle, Some("/unknown")));
        assert!(accepts_dialogue_text(
            &State::Idle,
            Some("/unknown@house_bot")
        ));

        let form = State::EditUserAlias { user_id: 42 };
        assert!(!accepts_dialogue_text(&form, Some("/unknown")));
        assert!(accepts_dialogue_text(&form, Some("Kitchen owner")));
        assert!(accepts_dialogue_text(&State::Idle, Some("непонятный ввод")));
    }

    #[test]
    fn recording_rule_input_detector_accepts_rule_form() {
        let text = "Тест: свет в коридоре\n3\nany\nlight.corridor;changed_from_to;off;on;\nlight.corridor;changed_from_to;on;off;\n60\n300\n0\n30";

        assert!(looks_like_recording_rule_input(text));
    }

    #[test]
    fn recording_rule_input_detector_rejects_plain_message() {
        assert!(!looks_like_recording_rule_input("привет"));
    }
}
