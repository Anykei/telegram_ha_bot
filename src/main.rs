use anyhow::{Context, Result};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::Dispatcher;
use teloxide::dptree;
use teloxide::error_handlers::LoggingErrorHandler;
use teloxide::update_listeners::Polling;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

extern crate pretty_env_logger;
#[macro_use]
extern crate log;

use crate::config::EnvPaths;
use crate::models::{AppConfig, RuntimeStatus};
use crate::options::AppOptions;

mod bot;
mod charts;
mod config;
mod core;
mod db;
mod ha;
mod i18n;
mod models;
mod options;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    pretty_env_logger::init();

    let cancel_token = CancellationToken::new();
    let main_cancel_token = cancel_token.clone();

    info!("🚀 Starting Homeassistant Telegram BOT.");

    let paths = EnvPaths::load()
        .validate()
        .context("Error checking env variables.")?;

    let options = AppOptions::load(&paths.options).context("Error load options.json.")?;

    let db_pool = db::init(
        &paths.db_url(),
        paths
            .migrations
            .to_str()
            .context("Путь к миграциям не валиден")?,
    )
    .await
    .context("Error initializing database pool.")?;

    let ha_client: Arc<dyn ha::HomeAssistantClient> =
        Arc::new(ha::init(paths.ha_url.clone(), paths.ha_token.clone()));

    let (recording_tx, recording_rx) = mpsc::channel::<core::camera_recording::RecordingJob>(32);

    let app_config = Arc::new(AppConfig {
        ha_client: ha_client.clone(),
        db: db_pool,
        root_user: options.root_user,

        delete_notification_messages_timeout_s: 5,
        ttl_notifications: 60 * 12,
        background_maintenance_interval_s: options.background_maintenance_interval_s,
        event_refresh_min_interval_s: options.event_refresh_min_interval_s,
        session_ttl_hours: options.session_ttl_hours,
        telegram_retry_after_extra_delay_s: options.telegram_retry_after_extra_delay_s,
        default_language: options.default_language,
        camera_default_clip_s: options.camera_default_clip_s,
        camera_clip_intervals_s: options.camera_clip_intervals_s.clone(),
        camera_recording_max_tail_seconds: options.camera_recording_max_tail_seconds,
        camera_recording_max_segment_seconds: options.camera_recording_max_segment_seconds,
        camera_recording_max_parallel_jobs: options.camera_recording_max_parallel_jobs,
        camera_recording_storage_root: options.camera_recording_storage_root.clone(),
        camera_recording_tx: recording_tx,

        sessions: DashMap::new(),
        ui_locks: DashMap::new(),
        recording_sends_in_progress: DashMap::new(),

        name_aliases: DashMap::new(),

        state_aliases: DashMap::new(),
        ui_background_cache: tokio::sync::Mutex::new(None),
        runtime_status: tokio::sync::RwLock::new(RuntimeStatus::default()),
    });

    info!("Load Backup sessions from database...");
    let active_sessions = db::get_all_active_sessions(&app_config.db).await?;

    for (uid, mid, context, last_seen_at) in active_sessions {
        app_config.sessions.insert(
            uid as u64,
            crate::models::UserSession {
                last_menu_id: mid,
                current_context: context,
                header_entities: std::collections::HashSet::new(), // Это можно тоже хранить в БД, если нужно
                last_ui_refresh_at: None,
                ui_refresh_blocked_until: None,
                last_seen_at,
            },
        );
    }
    info!("Restored {} action sessions.", app_config.sessions.len());

    match db::devices::get_all_display_names(&app_config.db).await {
        Ok(names) => {
            for (eid, name) in names {
                app_config.name_aliases.insert(eid, name);
            }
            info!(
                "Core: Cache primed with {} device names",
                app_config.name_aliases.len()
            );
        }
        Err(e) => error!("Core: Failed to prime name aliases: {}", e),
    }

    let states = db::get_state_aliases(&app_config.db).await;
    for (eid, state_map) in states {
        app_config.state_aliases.insert(eid, state_map);
    }

    let (tx, rx) = mpsc::channel::<ha::NotifyEvent>(100);
    ha::spawn_event_listener(
        paths.ha_url.clone(),
        paths.ha_token.clone(),
        cancel_token.clone(),
        tx,
    );

    info!("✅ Run Dispatcher...");

    tokio::spawn(async move {
        // Wait Ctrl+C or SIGTERM Docker/OS
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for ctrl+c");
        info!("Received SIGTERM");
        main_cancel_token.cancel();
    });

    let storage = InMemStorage::<bot::State>::new();
    let (mut dispatcher, _bot) = {
        let bot = bot::init(options.bot_token);
        let dispatcher = Dispatcher::builder(bot.clone(), bot::schema())
            .dependencies(dptree::deps![app_config.clone(), storage])
            .enable_ctrlc_handler()
            .build();
        (dispatcher, bot)
    };

    core::spawn_notification_processor(rx, _bot.clone(), app_config.clone(), cancel_token.clone());
    core::camera_recording::spawn_recording_worker(
        recording_rx,
        _bot.clone(),
        app_config.clone(),
        cancel_token.clone(),
    );
    core::spawn_background_maintenance(_bot.clone(), app_config.clone(), cancel_token.clone());

    let update_listener = Polling::builder(_bot.clone())
        .timeout(Duration::from_secs(30))
        .delete_webhook()
        .await
        .build();
    let bot_task = dispatcher.dispatch_with_listener(
        update_listener,
        LoggingErrorHandler::with_custom_text("An error from the update listener"),
    );

    tokio::select! {
        _ = bot_task => info!("Bot task completed successfully."),
        _ = cancel_token.cancelled() => info!("Bot task was canceled."),
    }

    info!("Graceful Shutdown...");

    app_config.db.close().await;

    info!("Database connection closed.");
    info!("Shutting down...");
    Ok(())
}
