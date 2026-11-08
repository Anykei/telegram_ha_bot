//! Opt-in acceptance against the configured HA, with an isolated database and no service calls.
use super::*;
use crate::bot::router::{AdminPayload, CameraPayload};
use crate::ha::{client::HistoryResult, models::Entity, HomeAssistantClient, Room};
use anyhow::ensure;
use sqlx::Connection;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

struct ReadOnlyHa {
    inner: crate::ha::HAClient,
    reads: AtomicUsize,
    writes: AtomicUsize,
}

#[async_trait::async_trait]
impl HomeAssistantClient for ReadOnlyHa {
    async fn health_check(&self) -> Result<()> {
        self.inner.health_check().await
    }
    async fn fetch_rooms(&self) -> Result<Vec<Room>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch_rooms().await
    }
    async fn fetch_states_by_ids(&self, ids: &[String]) -> Result<Vec<Entity>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch_states_by_ids(ids).await
    }
    async fn fetch_history(&self, id: &str, hours: u32, offset: i32) -> Result<HistoryResult> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch_history(id, hours, offset).await
    }
    async fn fetch_history_range(
        &self,
        id: &str,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> Result<HistoryResult> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch_history_range(id, start, end).await
    }
    async fn fetch_action_entities(&self) -> Result<Vec<Entity>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.fetch_action_entities().await
    }
    async fn call_service(&self, _: &str, _: &str, _: &str) -> Result<()> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        anyhow::bail!("Physical service call blocked by live UI test")
    }
    async fn call_service_with_data(&self, _: &str, _: &str, _: &str, _: Value) -> Result<()> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        anyhow::bail!("Physical service call blocked by live UI test")
    }
}

fn live_query(session: &WidgetSession, user: u64, key: &str) -> CallbackQuery {
    let raw = json!({"update_id":1,"widget_query":{"id":"live-check","from":{"id":user,"is_bot":false,"first_name":"Test"},"chat":{"id":user,"type":"private"},"message_id":session.message,"data":format!("r1:{}:{key}",session.pages[0].rev)}});
    let teloxide::types::UpdateKind::CallbackQuery(q) = transport::normalize(&raw).unwrap().kind
    else {
        unreachable!()
    };
    q
}

#[tokio::test]
#[ignore = "requires authorized local HA access; snapshots the DB and blocks all physical commands"]
async fn live_readonly_html_screens() -> Result<()> {
    let paths = crate::config::EnvPaths::load();
    ensure!(!paths.ha_token.is_empty(), "HA credentials missing");
    let options = crate::options::AppOptions::load(&paths.options)?;
    let user = options.root_user;
    let output = std::path::PathBuf::from(format!(
        "target/live-ui-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        std::process::id()
    ));
    let mut directory = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(&output)?;
    let copy = output.join("snapshot.sqlite3");
    // VACUUM INTO reads a consistent snapshot, including WAL, from a read-only connection.
    let mut source = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&paths.database)
            .read_only(true),
    )
    .await?;
    sqlx::query("VACUUM INTO ?")
        .bind(copy.to_string_lossy().as_ref())
        .execute(&mut source)
        .await?;
    source.close().await?;
    let pool = db::init(
        &format!("sqlite://{}", copy.display()),
        paths.migrations.to_str().context("Migration path")?,
    )
    .await?;
    let ha = Arc::new(ReadOnlyHa {
        inner: crate::ha::init(paths.ha_url, paths.ha_token.clone()),
        reads: AtomicUsize::new(0),
        writes: AtomicUsize::new(0),
    });
    ha.health_check().await?;
    let mut config = crate::test_support::config().await;
    let app = Arc::get_mut(&mut config).unwrap();
    app.db = pool;
    app.root_user = user;
    app.html_ui.enabled = true;
    app.ha_client = ha.clone();
    // Background workers and direct camera/voice clients are never started.
    db::add_user(user, &config.db).await?;
    let generation = db::admission_generation(user, &config.db)
        .await?
        .context("Admission")?;
    db::record_user_activity(user, &generation, chrono::Utc::now(), &config.db).await?;
    db::interface::set(user, db::interface::InterfaceMode::Html, &config.db).await?;
    let inventory = ha.fetch_rooms().await?;
    ensure!(!inventory.is_empty(), "HA returned no rooms");
    let mut synced = Vec::new();
    for room in &inventory {
        db::rooms::sync_rooms_from_ha(&room.id, &room.name, &config.db).await?;
        for entity in &room.entities {
            db::devices::sync_device(
                &entity.entity_id,
                &room.id,
                &entity.name,
                entity.device_class.as_deref().unwrap_or("undefined"),
                &config.db,
            )
            .await?;
            synced.push(entity.entity_id.clone());
        }
    }
    db::devices::archive_missing_devices(&synced, &config.db).await?;
    for (id, name) in db::devices::get_all_display_names(&config.db).await? {
        config.name_aliases.insert(id, name);
    }
    for (id, aliases) in db::get_state_aliases(&config.db).await {
        config.state_aliases.insert(id, aliases);
    }
    let mut routes = vec![
        ("home".to_owned(), Payload::Home),
        ("rooms".into(), Payload::Control(ControlPayload::ListRooms)),
        (
            "scenarios".into(),
            Payload::Control(ControlPayload::ActionGroups),
        ),
        (
            "cameras".into(),
            Payload::Camera(CameraPayload::ListCameras),
        ),
        (
            "settings".into(),
            Payload::Personal(PersonalPayload::Settings),
        ),
        (
            "interface".into(),
            Payload::Personal(PersonalPayload::Interface),
        ),
        (
            "favorites".into(),
            Payload::Personal(PersonalPayload::Favorites { page: 0 }),
        ),
        ("help".into(), Payload::Help),
        ("admin".into(), Payload::Admin(AdminPayload::ListActions)),
    ];
    let rooms = db::rooms::get_rooms_for_user(user, true, &config.db).await?;
    let mut domains = BTreeMap::<String, usize>::new();
    let mut examples = BTreeMap::new();
    for room in &rooms {
        let devices =
            db::devices::get_devices_by_room_for_user(user, true, room.id, &config.db).await?;
        let mut visible: usize = 0;
        routes.push((
            format!("room-{}", room.id),
            Payload::Control(ControlPayload::RoomDetail { room: room.id }),
        ));
        for device in devices {
            if db::subscriptions::is_hidden(&device.entity_id, &config.db).await? {
                continue;
            }
            visible += 1;
            let domain = device.entity_id.split('.').next().unwrap().to_string();
            *domains.entry(domain.clone()).or_default() += 1;
            examples
                .entry(domain.clone())
                .or_insert((room.id, device.id));
            routes.push((
                format!("{domain}-{}", device.id),
                Payload::Control(ControlPayload::DeviceControl {
                    room: room.id,
                    device: device.id,
                }),
            ));
        }
        for page in 1..visible.div_ceil(crate::bot::screens::common::LIST_PAGE_SIZE) {
            routes.push((
                format!("room-{}-page-{page}", room.id),
                Payload::Control(ControlPayload::RoomPage {
                    room: room.id,
                    page: page as u16,
                }),
            ));
        }
    }
    for (domain, &(room, device)) in &examples {
        routes.push((
            format!("{domain}-details"),
            Payload::DeviceDetails { room, device },
        ));
        if matches!(domain.as_str(), "sensor" | "binary_sensor") {
            for (h, o) in [(12, 0), (72, 0), (168, 0), (24, -24), (3, -2)] {
                routes.push((
                    format!("{domain}-history-{h}-{o}"),
                    Payload::Control(ControlPayload::QuickAction {
                        room,
                        device,
                        cmd: DeviceCmd::ShowChart { h, o },
                    }),
                ));
            }
        }
    }
    let cameras = db::cameras::list_accessible_cameras(user, true, &config.db).await?;
    for camera in &cameras {
        routes.push((
            format!("camera-{}-details", camera.id),
            Payload::Camera(CameraPayload::CameraDetails { id: camera.id }),
        ));
        routes.push((
            format!("camera-{}-archive", camera.id),
            Payload::Camera(CameraPayload::RecordingArchive { camera: camera.id }),
        ));
    }
    let fake = FakeTelegram::new().await?;
    let mut fixtures = Vec::new();
    let mut failures = Vec::new();
    let mut checked = 0;
    let mut telegram_samples = BTreeMap::new();
    for lang in [crate::i18n::Language::Ru, crate::i18n::Language::En] {
        db::set_user_language(user, lang, &config.db).await?;
        for (label, payload) in &routes {
            let result: Result<()> = async {
                let view = router(payload.clone(), user, config.clone()).await?;
                let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
                if view.html_chart.is_some() {
                    // The approved graph contract replaces classic time presets
                    // with the calendar and PNG export. Check that contract, not
                    // parity with the deliberately superseded inline keyboard.
                    ensure!(prepared.pages.len() == 1 && prepared.pages[0].kind == "chart", "Graph must remain one screen");
                    ensure!(serde_json::to_value(&prepared.pages[0].graph)? == serde_json::to_value(&view.html_chart)?, "Displayed graph changed");
                    ensure!(prepared.actions.values().any(|p| matches!(p, Payload::Control(ControlPayload::QuickAction { cmd: DeviceCmd::OpenChartRange, .. }))), "Calendar missing");
                    ensure!(prepared.actions.values().any(|p| matches!(p, Payload::Control(ControlPayload::QuickAction { cmd: DeviceCmd::ExportChart, .. }))), "PNG export missing");
                    ensure!(prepared.actions.values().any(|p| matches!(p, Payload::DeviceDetails { .. })), "Device details missing");
                    ensure!(prepared.pages[0].back.as_ref().is_some_and(|key| prepared.actions.contains_key(key)), "Graph parent missing");
                } else {
                    assert_buttons_preserved(&view, &prepared)?;
                    ensure!(prepared.pages.iter().map(|p| p.body.as_str()).collect::<String>().contains(&view.text), "Body text lost");
                }
                ensure!(view.alert.is_none(), "Screen has an error alert");
                for (index, page) in prepared.pages.iter().enumerate() {
                    ensure!(serde_json::to_string(&json!({"page":page}))?.len() <= 1024, "Query reply exceeds budget");
                    fixtures.push(json!({"id":format!("{label}-{lang:?}-{index}"),"renderer":if view.html_chart.is_some() {"graph"} else {"release"},"page":page}));
                }
                ensure!(handlers::send_new_view_inner(&fake.bot, ChatId(user as i64), user, view.clone(), config.clone(), None).await?, "Delivery rejected");
                ensure!(config.sessions.get(&user).is_some_and(|s| s.ui_message_mode == UiMessageMode::Html), "HTML delivery fell back");
                let session = config.html_ui.sessions.get(&user).unwrap().clone();
                for index in 0..session.pages.len() {
                    ensure!(resolve(&fake.bot, &live_query(&session, user, &format!("p{index}")), &config).await?.is_none(), "Paging dispatched an action");
                    let updated = config.html_ui.sessions.get(&user).unwrap();
                    ensure!(updated.index == index && updated.pages[index].seq > session.pages[index].seq, "Paging was rejected");
                }
                ensure!(resolve(&fake.bot, &live_query(&session, user, "read"), &config).await? == Some(view.payload.for_refresh().to_string()), "Refresh lost its route");
                if let Some(back) = &session.pages.last().unwrap().back {
                    let actual = resolve(&fake.bot, &live_query(&session, user, back), &config).await?;
                    ensure!(actual == session.actions.get(back).map(|p| p.to_string()), "Back did not resolve to its issued route");
                }
                let renderer = if view.html_chart.is_some() { "graph" } else if view.html_kind.as_deref() == Some("climate") { "climate" } else { "home" };
                telegram_samples.entry(renderer).or_insert_with(|| (view.html_chart.is_some(), prepared.pages));
                checked += 1;
                Ok(())
            }.await;
            if let Err(error) = result {
                failures.push(format!("{label} {lang:?}: {error}"));
            }
        }
        println!(
            "Live UI {lang:?}: {checked} screens checked, {} failures",
            failures.len()
        );
    }
    let mut telegram_deliveries = 0;
    if std::env::var("HA_UI_TELEGRAM_SMOKE").as_deref() == Ok("1") {
        let bot = Bot::new(&options.bot_token);
        for (graph, pages) in telegram_samples.values() {
            // Test-owned, noninteractive messages; never consume the running bot's updates.
            let message = transport::call(&bot, "sendWidget", json!({"chat_id":user,"html":if *graph {GRAPH_HTML} else {HTML},"width":0,"allow_queries":false,"chat_data":pages[0].json()?,"caption":"Проверка HTML на реальном HA · без управления"})).await?;
            let id = MessageId(
                message["message_id"]
                    .as_i64()
                    .context("Widget message ID")? as i32,
            );
            let edited = transport::call(
                &bot,
                "editMessageWidgetData",
                json!({"chat_id":user,"message_id":id.0,"chat_data":pages.last().unwrap().json()?}),
            )
            .await;
            let deleted = bot.delete_message(ChatId(user as i64), id).await;
            edited?;
            deleted?;
            telegram_deliveries += 1;
        }
    }
    ensure!(
        ha.writes.load(Ordering::Relaxed) == 0,
        "UI attempted physical commands"
    );
    let report = json!({"screens":checked,"pages":fixtures.len(),"rooms":rooms.len(),"device_domains":domains,"cameras":cameras.len(),"ha_reads":ha.reads.load(Ordering::Relaxed),"physical_commands":0,"telegram_deliveries":telegram_deliveries,"failures":failures});
    std::fs::write(output.join("fixtures.json"), serde_json::to_vec(&fixtures)?)?;
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("Live UI report: {report}");
    println!("Live UI artifacts: {}", output.display());
    ensure!(
        failures.is_empty(),
        "Live UI screen failures; see report.json"
    );
    Ok(())
}
