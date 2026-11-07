use super::*;
use crate::bot::router::ChartRangeEdit as Edit;
use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct ChartHa {
    inner: Arc<crate::ha::mock::MockHa>,
    reads: AtomicUsize,
    fail: AtomicBool,
    ranges: std::sync::Mutex<Vec<[i64; 2]>>,
}
#[async_trait::async_trait]
impl HomeAssistantClient for ChartHa {
    fn is_mock(&self) -> bool {
        true
    }
    async fn health_check(&self) -> Result<()> {
        self.inner.health_check().await
    }
    async fn fetch_rooms(&self) -> Result<Vec<crate::ha::Room>> {
        self.inner.fetch_rooms().await
    }
    async fn fetch_action_entities(&self) -> Result<Vec<crate::ha::models::Entity>> {
        self.inner.fetch_action_entities().await
    }
    async fn fetch_states_by_ids(&self, ids: &[String]) -> Result<Vec<crate::ha::models::Entity>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.fetch_states_by_ids(ids).await
    }
    async fn fetch_history(
        &self,
        id: &str,
        h: u32,
        o: i32,
    ) -> Result<crate::ha::client::HistoryResult> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.fetch_history(id, h, o).await
    }
    async fn fetch_history_range(
        &self,
        id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<crate::ha::client::HistoryResult> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.ranges
            .lock()
            .unwrap()
            .push([start.timestamp(), end.timestamp()]);
        anyhow::ensure!(
            !self.fail.load(Ordering::SeqCst),
            "test history unavailable"
        );
        self.inner.fetch_history_range(id, start, end).await
    }
    async fn call_service(&self, _: &str, _: &str, _: &str) -> Result<()> {
        panic!("calendar must never actuate devices")
    }
    async fn call_service_with_data(&self, _: &str, _: &str, _: &str, _: Value) -> Result<()> {
        panic!("calendar must never actuate devices")
    }
}
fn action(room: i64, device: i64, cmd: DeviceCmd) -> Payload {
    Payload::Control(ControlPayload::QuickAction { room, device, cmd })
}
fn edit_query(config: &AppConfig, edit: Edit) -> CallbackQuery {
    let s = current(config);
    let (key,_)=s.actions.iter().find(|(_,p)|matches!(p,Payload::Control(ControlPayload::QuickAction{cmd:DeviceCmd::EditChartRange{edit:e,..},..}) if *e==edit)).expect("issued editor action");
    query(&s, key)
}
#[tokio::test]
async fn graph_overview_replaces_presets_with_calendar_but_keeps_classic_keyboard() -> Result<()> {
    let (config, _) = mock_config().await?;
    for lang in [crate::i18n::Language::Ru, crate::i18n::Language::En] {
        db::user::set_user_language(1, lang, &config.db).await?;
        for entity in ["sensor.bedroom_humidity", "binary_sensor.kitchen_window"] {
            let (room, device) = device(&config, entity).await?;
            let view = router(
                Payload::Control(ControlPayload::DeviceControl { room, device }),
                1,
                config.clone(),
            )
            .await?;
            assert!(view.image.is_some());
            let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
            assert_eq!(prepared.pages.len(), 1);
            let page = &prepared.pages[0];
            assert_eq!(page.kind, "chart");
            assert_eq!(page.graph.as_ref().unwrap().plot.len(), 96);
            assert!(page.body.contains(view.html_summary.as_deref().unwrap()));
            assert!(page.body.contains("UTC"));
            assert!(page.json()?.len() <= 1000);
            assert_eq!(prepared.actions.len(), 3);
            assert!(prepared
                .actions
                .values()
                .any(|p| *p == action(room, device, DeviceCmd::OpenChartRange)));
            assert!(prepared
                .actions
                .values()
                .any(|p| *p == Payload::DeviceDetails { room, device }));
            for hours in [12, 24, 72, 168] {
                let classic =
                    action(room, device, DeviceCmd::ShowChart { h: hours, o: 0 }).to_string();
                assert!(view.kb.inline_keyboard.iter().flatten().any(|b|matches!(&b.kind,teloxide::types::InlineKeyboardButtonKind::CallbackData(s) if *s==classic)));
            }
        }
    }
    let (room, device) = device(&config, "sensor.bedroom_humidity").await?;
    sqlx::query("DELETE FROM ha_mock_history WHERE entity_id='sensor.bedroom_humidity'")
        .execute(&config.db)
        .await?;
    let view = router(
        Payload::Control(ControlPayload::DeviceControl { room, device }),
        1,
        config.clone(),
    )
    .await?;
    assert!(view.image.is_none());
    let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
    assert_eq!(
        prepared.pages[0].graph.as_ref().unwrap().plot,
        ".".repeat(96)
    );
    Ok(())
}
#[tokio::test]
async fn calendar_draft_cancel_apply_retry_and_refresh_keep_exact_bounds_in_same_message(
) -> Result<()> {
    let (mut config, mock) = mock_config().await?;
    let ha = Arc::new(ChartHa {
        inner: mock,
        reads: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
        ranges: Default::default(),
    });
    Arc::get_mut(&mut config).unwrap().ha_client = ha.clone();
    let fake = FakeTelegram::new().await?;
    let (room, device) = device(&config, "sensor.bedroom_humidity").await?;
    let open = Payload::Control(ControlPayload::DeviceControl { room, device });
    let previous = publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::RoomDetail { room }),
    )
    .await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    press(&fake, &config, &dialogue, action_query(&config, &open)).await?;
    let original = current(&config);
    assert_ne!(previous.message, original.message);
    assert_eq!(
        fake.calls()
            .iter()
            .rev()
            .find(|(m, _)| m == "sendWidget")
            .unwrap()
            .1["html"],
        GRAPH_HTML
    );
    let before = ha.reads.load(Ordering::SeqCst);
    press(
        &fake,
        &config,
        &dialogue,
        action_query(&config, &action(room, device, DeviceCmd::OpenChartRange)),
    )
    .await?;
    let stale = current(&config);
    for edit in [
        Edit::Date(false),
        Edit::Overview,
        Edit::Time(false),
        Edit::Hour(8),
        Edit::Minute(5),
        Edit::Nudge(1),
        Edit::Nudge(1),
        Edit::Overview,
    ] {
        press(&fake, &config, &dialogue, edit_query(&config, edit)).await?;
        assert_eq!(current(&config).message, original.message);
    }
    let editor = current(&config);
    press(&fake, &config, &dialogue, query(&editor, "read")).await?;
    assert_eq!(
        ha.reads.load(Ordering::SeqCst),
        before,
        "draft and read do not fetch HA"
    );
    press(&fake, &config, &dialogue, edit_query(&config, Edit::Cancel)).await?;
    assert_eq!(current(&config).view.html_range, original.view.html_range);
    assert_eq!(
        current(&config).view.kb,
        original.view.kb,
        "cancel does not duplicate favorites"
    );
    assert_eq!(ha.reads.load(Ordering::SeqCst), before);
    press(
        &fake,
        &config,
        &dialogue,
        action_query(&config, &action(room, device, DeviceCmd::OpenChartRange)),
    )
    .await?;
    // A callback from the old draft cannot change the replacement.
    let replacement = current(&config);
    assert!(resolve(
        &fake.bot,
        &query(&stale, stale.pages[0].back.as_deref().unwrap()),
        &config
    )
    .await?
    .is_none());
    assert_eq!(
        current(&config).view.html_range,
        replacement.view.html_range
    );
    for edit in [
        Edit::Time(false),
        Edit::Hour(8),
        Edit::Minute(5),
        Edit::Nudge(1),
        Edit::Nudge(1),
        Edit::Overview,
    ] {
        press(&fake, &config, &dialogue, edit_query(&config, edit)).await?;
    }
    let selected = current(&config).view.html_range.unwrap();
    if (selected[1] - selected[0]) % 3600 == 0 {
        for edit in [
            Edit::Time(false),
            Edit::Hour(8),
            Edit::Nudge(1),
            Edit::Overview,
        ] {
            press(&fake, &config, &dialogue, edit_query(&config, edit)).await?;
        }
    }
    let bounds = current(&config).view.html_range.unwrap();
    assert_eq!(bounds[0] % 60, 0);
    assert_ne!(
        (bounds[1] - bounds[0]) % 3600,
        0,
        "minute precision is not rounded to hours"
    );
    ha.fail.store(true, Ordering::SeqCst);
    press(&fake, &config, &dialogue, edit_query(&config, Edit::Apply)).await?;
    assert_eq!(current(&config).view.html_kind.as_deref(), Some("range"));
    assert_eq!(current(&config).view.html_range, Some(bounds));
    assert!(current(&config).view.alert.is_some());
    assert_eq!(current(&config).message, original.message);
    let reads = ha.reads.load(Ordering::SeqCst);
    let failed = current(&config);
    press(&fake, &config, &dialogue, query(&failed, "read")).await?;
    assert_eq!(
        ha.reads.load(Ordering::SeqCst),
        reads,
        "read recovers draft without retrying history"
    );
    ha.fail.store(false, Ordering::SeqCst);
    press(&fake, &config, &dialogue, edit_query(&config, Edit::Apply)).await?;
    let applied = current(&config);
    assert_eq!(applied.view.html_range, Some(bounds));
    assert_eq!(
        applied.view.payload,
        action(
            room,
            device,
            DeviceCmd::ShowChartRange {
                start: bounds[0],
                end: bounds[1]
            }
        )
    );
    assert_eq!(applied.message, original.message);
    press(&fake, &config, &dialogue, query(&applied, "read")).await?;
    assert_eq!(current(&config).view.html_range, Some(bounds));
    assert!(ha.ranges.lock().unwrap().iter().all(|r| *r == bounds));
    assert_eq!(ha.ranges.lock().unwrap().len(), 3);
    // Supplementary controls remain on the device details page.
    press(
        &fake,
        &config,
        &dialogue,
        action_query(&config, &Payload::DeviceDetails { room, device }),
    )
    .await?;
    assert_ne!(current(&config).message, original.message);
    let details = current(&config);
    assert!(details
        .actions
        .values()
        .any(|p| matches!(p, Payload::Personal(_))));
    press(&fake, &config, &dialogue, action_query(&config, &open)).await?;
    assert_eq!(
        current(&config).view.html_range,
        Some(bounds),
        "return restores absolute range"
    );
    let graph = current(&config);
    press(
        &fake,
        &config,
        &dialogue,
        query(&graph, graph.pages[0].back.as_deref().unwrap()),
    )
    .await?;
    assert_eq!(
        current(&config).view.payload,
        Payload::Control(ControlPayload::RoomDetail { room })
    );
    assert!(!fake
        .calls()
        .iter()
        .any(|(m, _)| m.eq_ignore_ascii_case("sendPhoto")));
    press(&fake, &config, &dialogue, action_query(&config, &open)).await?;
    press(
        &fake,
        &config,
        &dialogue,
        action_query(&config, &action(room, device, DeviceCmd::OpenChartRange)),
    )
    .await?;
    press(
        &fake,
        &config,
        &dialogue,
        edit_query(&config, Edit::Time(false)),
    )
    .await?;
    fake.clear();
    handlers::handle_command(
        fake.bot.clone(),
        input_message("/classic")?,
        handlers::Command::Classic,
        config.clone(),
        dialogue,
    )
    .await?;
    assert!(fake
        .calls()
        .iter()
        .any(|(m, _)| m.eq_ignore_ascii_case("sendPhoto")));
    assert_eq!(ha.ranges.lock().unwrap().last(), Some(&bounds));
    assert!(!config.html_ui.ranges.contains_key(&1));
    Ok(())
}

#[tokio::test]
async fn calendar_invalid_range_never_reads_history_and_regrant_invalidates_draft() -> Result<()> {
    use chrono::{Datelike, Local, Timelike};
    let (mut config, mock) = mock_config().await?;
    let ha = Arc::new(ChartHa {
        inner: mock,
        reads: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
        ranges: Default::default(),
    });
    Arc::get_mut(&mut config).unwrap().ha_client = ha.clone();
    let fake = FakeTelegram::new().await?;
    let (room, device) = device(&config, "sensor.bedroom_humidity").await?;
    publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::DeviceControl { room, device }),
    )
    .await?;
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(),
        ChatId(1),
    );
    press(
        &fake,
        &config,
        &dialogue,
        action_query(&config, &action(room, device, DeviceCmd::OpenChartRange)),
    )
    .await?;
    let reads = ha.reads.load(Ordering::SeqCst);
    let [start, end] = current(&config).view.html_range.unwrap().map(|s| {
        DateTime::from_timestamp(s, 0)
            .unwrap()
            .with_timezone(&Local)
    });
    press(
        &fake,
        &config,
        &dialogue,
        edit_query(&config, Edit::Date(false)),
    )
    .await?;
    if start.month() != end.month() {
        press(
            &fake,
            &config,
            &dialogue,
            edit_query(&config, Edit::Month(1)),
        )
        .await?;
    }
    for edit in [
        Edit::Day(end.day() as u8),
        Edit::Time(false),
        Edit::Hour(end.hour() as u8),
        Edit::Minute((end.minute() / 5 * 5) as u8),
    ] {
        press(&fake, &config, &dialogue, edit_query(&config, edit)).await?;
    }
    for _ in 0..end.minute() % 5 {
        press(
            &fake,
            &config,
            &dialogue,
            edit_query(&config, Edit::Nudge(1)),
        )
        .await?;
    }
    for edit in [Edit::Overview, Edit::Apply] {
        press(&fake, &config, &dialogue, edit_query(&config, edit)).await?;
    }
    let editor = current(&config);
    assert_eq!(
        editor.view.html_range,
        Some([end.timestamp(), end.timestamp()])
    );
    assert!(editor.view.alert.is_some());
    assert!(editor.pages[0].json()?.len() <= 1000);
    assert_eq!(ha.reads.load(Ordering::SeqCst), reads);
    let q = edit_query(&config, Edit::Apply);
    db::delete_user(1, &config.db).await?;
    db::add_user(1, &config.db).await?;
    assert!(resolve(&fake.bot, &q, &config).await?.is_none());
    assert_eq!(ha.reads.load(Ordering::SeqCst), reads);
    Ok(())
}
