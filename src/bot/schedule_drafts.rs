use super::{
    models::View,
    personal::words,
    router::{AdminPayload, Payload, RenderContext, State},
};
use crate::{
    db::{
        self,
        action_groups::{ActionScheduleCommand, ActionTargetRef},
        schedule_drafts::{ScheduleDraft, KIND},
    },
    i18n::t,
    models::AppConfig,
};
use anyhow::{Context, Result};
use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton as Button, InlineKeyboardMarkup, MessageId},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SchedulePayload {
    Start {
        target: ActionTargetRef,
        command: ActionScheduleCommand,
    },
    Edit {
        target: ActionTargetRef,
        schedule: i64,
    },
    Review {
        id: i64,
    },
    Time {
        id: i64,
        revision: i64,
    },
    Day {
        id: i64,
        revision: i64,
        day: u8,
    },
    Command {
        id: i64,
        revision: i64,
    },
    Save {
        id: i64,
        revision: i64,
        enabled: bool,
    },
    Cancel {
        id: i64,
        revision: i64,
    },
}
impl SchedulePayload {
    pub fn for_refresh(&self) -> Payload {
        match *self {
            Self::Start { target, .. } => Payload::Admin(AdminPayload::ActionSchedules { target }),
            Self::Edit { target, schedule } => {
                Payload::Admin(AdminPayload::ActionScheduleDetail { target, schedule })
            }
            Self::Time { .. } => Payload::Schedule(self.clone()),
            Self::Review { id }
            | Self::Day { id, .. }
            | Self::Command { id, .. }
            | Self::Save { id, .. }
            | Self::Cancel { id, .. } => Payload::Schedule(Self::Review { id }),
        }
    }
}
fn button(label: impl Into<String>, p: SchedulePayload) -> Button {
    Button::callback(label, Payload::Schedule(p).to_string())
}

pub async fn route(ctx: RenderContext, p: SchedulePayload) -> Result<View> {
    if !ctx.is_admin {
        return Ok(expired(
            &ctx,
            words(ctx.lang, "Доступ запрещён", "Access denied"),
        ));
    }
    let original = p.clone();
    match route_inner(ctx.clone(), p).await {
        Ok(view) => Ok(view),
        Err(error) => {
            log::warn!("Schedule draft rejected for {}: {}", ctx.user_id, error);
            let mut view =
                if let Payload::Schedule(SchedulePayload::Review { id }) = original.for_refresh() {
                    review(ctx.clone(), id, false).await.unwrap_or_else(|_| {
                        expired(
                            &ctx,
                            words(
                                ctx.lang,
                                "Черновик истёк или отменён",
                                "Draft expired or cancelled",
                            ),
                        )
                    })
                } else {
                    expired(
                        &ctx,
                        words(ctx.lang, "Расписание недоступно", "Schedule unavailable"),
                    )
                };
            view.alert = Some(words(ctx.lang,"Изменение не сохранено. Проверьте время, дни и цель. Возможно, черновик или расписание уже изменены; откройте актуальную версию. Данные черновика сохранены.","Change was not saved. Check time, days and target. The draft or schedule may have changed; reopen its current version. Draft data is retained.").into());
            Ok(view)
        }
    }
}

async fn route_inner(ctx: RenderContext, p: SchedulePayload) -> Result<View> {
    match p {
        SchedulePayload::Start { target, command } => {
            let data = ScheduleDraft {
                target,
                command,
                time_minute: None,
                days_mask: 127,
                enabled: false,
                existing: None,
            };
            let draft = db::ui_drafts::create(
                ctx.user_id,
                KIND,
                &serde_json::to_string(&data)?,
                ctx.config.session_ttl_hours,
                &ctx.config.db,
            )
            .await?;
            review(ctx, draft.id, true).await
        }
        SchedulePayload::Edit { target, schedule } => {
            let data = ScheduleDraft::from_existing(schedule, target, &ctx.config.db).await?;
            let draft = db::ui_drafts::create(
                ctx.user_id,
                KIND,
                &serde_json::to_string(&data)?,
                ctx.config.session_ttl_hours,
                &ctx.config.db,
            )
            .await?;
            review(ctx, draft.id, false).await
        }
        SchedulePayload::Review { id } => review(ctx, id, false).await,
        SchedulePayload::Time { id, revision } => {
            let draft = db::ui_drafts::load(id, ctx.user_id, KIND, &ctx.config.db)
                .await?
                .context("Draft expired")?;
            anyhow::ensure!(draft.revision == revision, "Draft changed");
            review(ctx, id, true).await
        }
        SchedulePayload::Day { id, revision, day } => {
            anyhow::ensure!(day < 7, "Invalid weekday");
            edit(&ctx, id, revision, |data| {
                data.days_mask ^= 1_i64 << day;
            })
            .await?;
            review(ctx, id, false).await
        }
        SchedulePayload::Command { id, revision } => {
            edit(&ctx, id, revision, |data| {
                data.command = match data.command {
                    ActionScheduleCommand::TurnOn => ActionScheduleCommand::TurnOff,
                    ActionScheduleCommand::TurnOff => ActionScheduleCommand::TurnOn,
                    ActionScheduleCommand::Execute => ActionScheduleCommand::Execute,
                };
            })
            .await?;
            review(ctx, id, false).await
        }
        SchedulePayload::Save {
            id,
            revision,
            enabled,
        } => {
            db::schedule_drafts::commit(id, ctx.user_id, revision, enabled, &ctx.config.db).await?;
            review(ctx, id, false).await
        }
        SchedulePayload::Cancel { id, revision } => {
            let draft = db::ui_drafts::load(id, ctx.user_id, KIND, &ctx.config.db)
                .await?
                .context("Draft expired")?;
            let data: ScheduleDraft = serde_json::from_str(&draft.payload)?;
            db::ui_drafts::cancel(id, ctx.user_id, revision, &ctx.config.db).await?;
            super::screens::action_groups::render_schedules(ctx, data.target).await
        }
    }
}

async fn edit(
    ctx: &RenderContext,
    id: i64,
    revision: i64,
    update: impl FnOnce(&mut ScheduleDraft),
) -> Result<()> {
    let draft = db::ui_drafts::load(id, ctx.user_id, KIND, &ctx.config.db)
        .await?
        .context("Draft expired")?;
    anyhow::ensure!(draft.revision == revision, "Draft changed");
    let mut data: ScheduleDraft = serde_json::from_str(&draft.payload)?;
    update(&mut data);
    db::ui_drafts::save(
        id,
        ctx.user_id,
        revision,
        &serde_json::to_string(&data)?,
        "review",
        "",
        true,
        ctx.config.session_ttl_hours,
        &ctx.config.db,
    )
    .await?;
    Ok(())
}

fn expired(ctx: &RenderContext, text: &str) -> View {
    View {
        text: text.into(),
        lang: ctx.lang,
        kb: InlineKeyboardMarkup::new(vec![vec![super::screens::common::main_menu_button_lang(
            ctx.lang,
        )]]),
        ..Default::default()
    }
}

pub fn next_run(
    now: chrono::DateTime<chrono::Local>,
    minute: i64,
    days: i64,
    last_run: Option<&str>,
) -> Option<chrono::DateTime<chrono::Local>> {
    let start = now.with_second(0)?.with_nanosecond(0)?;
    (0..=8 * 24 * 60)
        .map(|offset| start + chrono::Duration::minutes(offset))
        .find(|date| {
            i64::from(date.hour()) * 60 + i64::from(date.minute()) == minute
                && days & (1 << date.weekday().num_days_from_monday()) != 0
                && last_run != Some(date.format("%Y-%m-%d %H:%M").to_string().as_str())
        })
}

async fn review(ctx: RenderContext, id: i64, input: bool) -> Result<View> {
    let Some(draft) = db::ui_drafts::load(id, ctx.user_id, KIND, &ctx.config.db).await? else {
        return Ok(expired(
            &ctx,
            words(
                ctx.lang,
                "Черновик истёк или отменён. Откройте расписания для создания нового.",
                "Draft expired or cancelled. Open schedules to create another.",
            ),
        ));
    };
    let data: ScheduleDraft = serde_json::from_str(&draft.payload)?;
    if let Some(saved) = draft.committed_target_id {
        return super::screens::action_groups::render_schedule_detail(ctx, data.target, saved)
            .await;
    }
    let revision = draft.revision;
    let w = |ru, en| words(ctx.lang, ru, en);
    let title = match data.target {
        ActionTargetRef::BotGroup(id) => db::action_groups::get_action_group(id, &ctx.config.db)
            .await?
            .map(|t| {
                ctx.track_group(t.id);
                t.name
            }),
        ActionTargetRef::HaNative(id) => {
            db::action_groups::get_ha_native_target(id, &ctx.config.db)
                .await?
                .filter(|t| !t.is_archived())
                .map(|t| {
                    ctx.track_native(t.id, &t.entity_id);
                    t.display_name().to_owned()
                })
        }
    }
    .unwrap_or_else(|| {
        w(
            "Цель недоступна — исправьте расписание",
            "Target unavailable — correct the schedule",
        )
        .into()
    });
    let command = match data.command {
        ActionScheduleCommand::TurnOn => w("Включить", "Turn on"),
        ActionScheduleCommand::TurnOff => w("Выключить", "Turn off"),
        ActionScheduleCommand::Execute => w("Запустить", "Run"),
    };
    let now = chrono::Local::now();
    let last = if let Some((schedule, _)) = data.existing {
        db::action_groups::get_action_schedule(schedule, &ctx.config.db)
            .await?
            .and_then(|s| s.last_run_at)
    } else {
        None
    };
    let next = data
        .time_minute
        .and_then(|minute| next_run(now, minute, data.days_mask, last.as_deref()))
        .map(|date| date.format("%Y-%m-%d %H:%M %:z").to_string())
        .unwrap_or_else(|| w("Не задан", "Not set").into());
    let labels = if ctx.lang == crate::i18n::Language::Ru {
        ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"]
    } else {
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
    };
    let days = labels
        .iter()
        .enumerate()
        .filter(|(i, _)| data.days_mask & (1 << i) != 0)
        .map(|(_, label)| *label)
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!(
        "{}\n{title}\n{}: {command}\n{}: {}\n{}: {days}\n{} (UTC {})\n{}: {next}\n\n{}",
        w("Проверка расписания", "Review schedule"),
        w("Команда", "Command"),
        w("Время", "Time"),
        data.time_minute
            .map(db::action_groups::format_time_minute)
            .unwrap_or_else(|| "—".into()),
        w("Дни", "Days"),
        w("Время сервера", "Server time"),
        now.format("%:z"),
        w("Ближайший расчётный запуск", "Next calculated run"),
        if data.existing.is_some() {
            w(
                "Действующая версия работает до сохранения изменений.",
                "The active version continues until you save changes.",
            )
        } else {
            w(
                "Черновик не выполняется до явного включения.",
                "The draft will not run until explicitly enabled.",
            )
        }
    );
    if ctx
        .config
        .runtime_status
        .read()
        .await
        .last_ha_sync_error
        .is_some()
    {
        text.push_str(w("\n\nПоследняя синхронизация HA завершилась ошибкой. Настройки сохраняются локально; выполнение зависит от связи с HA.", "\n\nThe last HA sync failed. Settings are saved locally; execution depends on the HA connection."));
    }
    let mut rows = vec![vec![button(
        w("Изменить время", "Change time"),
        SchedulePayload::Time { id, revision },
    )]];
    for chunk in (0..7).collect::<Vec<_>>().chunks(4) {
        rows.push(
            chunk
                .iter()
                .map(|day| {
                    button(
                        format!(
                            "{} {}",
                            if data.days_mask & (1 << day) != 0 {
                                "✓"
                            } else {
                                "○"
                            },
                            labels[*day]
                        ),
                        SchedulePayload::Day {
                            id,
                            revision,
                            day: *day as u8,
                        },
                    )
                })
                .collect(),
        );
    }
    if matches!(data.target, ActionTargetRef::BotGroup(_)) {
        rows.push(vec![button(
            w("Изменить команду", "Change command"),
            SchedulePayload::Command { id, revision },
        )]);
    }
    if data.time_minute.is_some() && data.days_mask > 0 {
        if data.existing.is_some() {
            rows.push(vec![button(
                w("Сохранить изменения", "Save changes"),
                SchedulePayload::Save {
                    id,
                    revision,
                    enabled: data.enabled,
                },
            )]);
        } else {
            rows.push(vec![
                button(
                    w("Включить расписание", "Enable schedule"),
                    SchedulePayload::Save {
                        id,
                        revision,
                        enabled: true,
                    },
                ),
                button(
                    w("Сохранить выключенным", "Save disabled"),
                    SchedulePayload::Save {
                        id,
                        revision,
                        enabled: false,
                    },
                ),
            ]);
        }
    }
    rows.push(vec![button(
        w("Отменить создание / изменения", "Cancel draft / changes"),
        SchedulePayload::Cancel { id, revision },
    )]);
    rows.push(vec![
        Button::callback(
            t(ctx.lang, "common.back"),
            Payload::Admin(AdminPayload::ActionSchedules {
                target: data.target,
            })
            .to_string(),
        ),
        super::screens::common::main_menu_button_lang(ctx.lang),
    ]);
    if input {
        text.push_str(w(
            "\n\nВведите HH:MM (00:00–23:59). /cancel — назад с сохранением полей.",
            "\n\nEnter HH:MM (00:00–23:59). /cancel returns without losing saved fields.",
        ));
    }
    Ok(View {
        text,
        // Cancel this field without abandoning the draft or changing its revision.
        html_back: Some(if input {
            Payload::Schedule(SchedulePayload::Review { id })
        } else {
            Payload::Admin(AdminPayload::ActionSchedules {
                target: data.target,
            })
        }),
        kb: InlineKeyboardMarkup::new(rows),
        payload: Payload::Schedule(if input {
            SchedulePayload::Time { id, revision }
        } else {
            SchedulePayload::Review { id }
        }),
        next_state: input.then_some(State::ScheduleDraftTime { id, revision }),
        lang: ctx.lang,
        compact: true,
        notifications: ctx.notifications,
        ..Default::default()
    })
}

pub async fn handle_time(
    bot: Bot,
    msg: Message,
    config: Arc<AppConfig>,
    dialogue: super::handlers::MyDialogue,
    (id, revision): (i64, i64),
) -> Result<()> {
    let user = msg.from.as_ref().context("User missing")?.id.0;
    if user != config.root_user {
        dialogue.exit().await?;
        return Ok(());
    }
    // Navigation invalidates delivery even while the accepted draft write waits for SQLite.
    let token = config.begin_view_update(user);
    let result = async {
        let lang = db::get_user_language(user, &config.db)
            .await?
            .unwrap_or(config.default_language);
        let minute = match db::action_groups::parse_time_minute(msg.text().unwrap_or("")) {
            Ok(v) => v,
            Err(_) => {
                if token.is_cancelled() {
                    return Ok(());
                }
                bot.send_message(
                    msg.chat.id,
                    words(
                        lang,
                        "Введите время HH:MM от 00:00 до 23:59.",
                        "Enter HH:MM from 00:00 to 23:59.",
                    ),
                )
                .await?;
                return Ok(());
            }
        };
        let ctx = RenderContext {
            displayed_resources: Default::default(),
            user_id: user,
            admission_generation: db::user::admission_generation(user, &config.db)
                .await?
                .context("User admission revoked")?,
            is_admin: true,
            config: config.clone(),
            notifications: vec![],
            lang,
            settings_origin: super::router::SettingsOrigin::Home,
        };
        if edit(&ctx, id, revision, |data| data.time_minute = Some(minute))
            .await
            .is_err()
        {
            if token.is_cancelled() {
                return Ok(());
            }
            bot.send_message(
                msg.chat.id,
                words(
                    lang,
                    "Черновик изменён или истёк. Откройте актуальную версию.",
                    "Draft changed or expired. Reopen its current version.",
                ),
            )
            .await?;
            return Ok(());
        }
        if token.is_cancelled() {
            return Ok(());
        }
        let view = super::router::router(
            Payload::Schedule(SchedulePayload::Review { id }),
            user,
            config.clone(),
        )
        .await?;
        if token.is_cancelled() {
            return Ok(());
        }
        dialogue.exit().await?;
        let mid = config
            .sessions
            .get(&user)
            .map(|s| MessageId(s.last_menu_id));
        if let Some(mid) = mid {
            super::handlers::update_view_cancellable(
                &bot,
                msg.chat.id,
                mid,
                user,
                view,
                config.clone(),
                Some(&token),
            )
            .await?;
        } else {
            super::handlers::send_new_view_inner(
                &bot,
                msg.chat.id,
                user,
                view,
                config.clone(),
                Some(&token),
            )
            .await?;
        }
        Ok(())
    }
    .await;
    config.finish_view_update(user, &token);
    result
}
