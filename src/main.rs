use anyhow::{Context, Result};
use chrono::Utc;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::Dispatcher;
use teloxide::dptree;
use teloxide::error_handlers::LoggingErrorHandler;
use teloxide::types::{ChatId, MessageId};
use teloxide::update_listeners::Polling;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

extern crate pretty_env_logger;
#[macro_use]
extern crate log;

use crate::config::EnvPaths;
use crate::models::{AppConfig, RuntimeStatus, UiMessageMode};
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

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const BACKGROUND_TASK_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(12);
const BACKGROUND_TASK_ABORT_TIMEOUT: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    pretty_env_logger::init();

    let cancel_token = CancellationToken::new();
    let main_cancel_token = cancel_token.clone();

    info!("🚀 Starting Home Assistant Telegram Bot v{}.", APP_VERSION);

    let paths = EnvPaths::load()
        .validate()
        .context("Error checking env variables.")?;

    let options = AppOptions::load(&paths.options).context("Error load options.json.")?;
    info!(
        "Startup config: db={}, migrations={}, ha_url={}, default_language={:?}, voice_enabled={}, recording_storage={}, recording_parallel_jobs={}",
        paths.db_url(),
        paths.migrations.display(),
        paths.ha_url,
        options.default_language,
        options.voice_enabled,
        options.camera_recording_storage_root,
        options.camera_recording_max_parallel_jobs
    );

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
        ha_url: paths.ha_url.clone(),
        ha_token: paths.ha_token.clone(),
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
        voice_enabled: options.voice_enabled,
        voice_stt_provider: options.voice_stt_provider,
        voice_command_engine: options.voice_command_engine,
        voice_ha_pipeline_id: options.voice_ha_pipeline_id.clone(),
        voice_stt_sample_rate: options.voice_stt_sample_rate,
        voice_confirm_dangerous: options.voice_confirm_dangerous,
        voice_pending_ttl_s: options.voice_pending_ttl_s,
        voice_max_audio_size_mb: options.voice_max_audio_size_mb,
        voice_max_audio_duration_s: options.voice_max_audio_duration_s,
        voice_stt_timeout_s: options.voice_stt_timeout_s,
        voice_show_recognized_text: options.voice_show_recognized_text,
        voice_response_format: options.voice_response_format,

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
                ui_message_mode: UiMessageMode::Photo,
                header_entities: std::collections::HashSet::new(), // Это можно тоже хранить в БД, если нужно
                recording_rule_wizard: None,
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
    let event_listener_handle = ha::spawn_event_listener(
        paths.ha_url.clone(),
        paths.ha_token.clone(),
        cancel_token.clone(),
        tx,
    );

    info!("✅ Run Dispatcher...");

    let storage = InMemStorage::<bot::State>::new();
    let (mut dispatcher, _bot) = {
        let bot = bot::init(options.bot_token);
        let dispatcher = Dispatcher::builder(bot.clone(), bot::schema())
            .dependencies(dptree::deps![app_config.clone(), storage])
            .build();
        (dispatcher, bot)
    };

    let notification_handle = core::spawn_notification_processor(
        rx,
        _bot.clone(),
        app_config.clone(),
        cancel_token.clone(),
    );
    let recording_handle = core::camera_recording::spawn_recording_worker(
        recording_rx,
        _bot.clone(),
        app_config.clone(),
        cancel_token.clone(),
    );
    let maintenance_handle =
        core::spawn_background_maintenance(_bot.clone(), app_config.clone(), cancel_token.clone());
    spawn_shutdown_signal_handler(main_cancel_token, _bot.clone(), app_config.clone());

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
    cancel_token.cancel();

    wait_for_background_tasks(vec![
        ("ha_event_listener", event_listener_handle),
        ("notification_processor", notification_handle),
        ("camera_recording_worker", recording_handle),
        ("background_maintenance", maintenance_handle),
    ])
    .await;

    app_config.db.close().await;

    info!("Database connection closed.");
    info!("Shutting down...");
    Ok(())
}

async fn wait_for_background_tasks(handles: Vec<(&'static str, JoinHandle<()>)>) {
    let mut waits = JoinSet::new();
    for (name, handle) in handles {
        waits.spawn(wait_for_background_task(name, handle));
    }

    while let Some(result) = waits.join_next().await {
        if let Err(error) = result {
            error!("Background task waiter failed: {}", error);
        }
    }
}

async fn wait_for_background_task(name: &'static str, mut handle: JoinHandle<()>) {
    tokio::select! {
        result = &mut handle => log_background_task_result(name, result),
        _ = tokio::time::sleep(BACKGROUND_TASK_SHUTDOWN_TIMEOUT) => {
            warn!(
                "Background task {} did not stop within {:?}; aborting before database shutdown",
                name,
                BACKGROUND_TASK_SHUTDOWN_TIMEOUT
            );
            handle.abort();

            match tokio::time::timeout(BACKGROUND_TASK_ABORT_TIMEOUT, &mut handle).await {
                Ok(result) => log_background_task_result(name, result),
                Err(_) => error!(
                    "Background task {} did not finish after abort within {:?}",
                    name,
                    BACKGROUND_TASK_ABORT_TIMEOUT
                ),
            }
        }
    }
}

fn log_background_task_result(name: &str, result: std::result::Result<(), tokio::task::JoinError>) {
    match result {
        Ok(()) => info!("Background task {} stopped.", name),
        Err(error) if error.is_cancelled() => warn!("Background task {} was aborted.", name),
        Err(error) => error!("Background task {} failed: {}", name, error),
    }
}

fn spawn_shutdown_signal_handler(
    cancel_token: CancellationToken,
    bot: teloxide::Bot,
    config: Arc<AppConfig>,
) {
    tokio::spawn(async move {
        let reason = wait_for_shutdown_signal().await;
        info!("Received {}, preparing graceful shutdown", reason);

        mark_shutdown_status(&config, reason).await;
        refresh_shutdown_status_headers(&bot, &config).await;

        cancel_token.cancel();
    });
}

async fn mark_shutdown_status(config: &Arc<AppConfig>, reason: &str) {
    let mut status = config.runtime_status.write().await;
    status.shutdown_requested_at = Some(Utc::now());
    status.shutdown_reason = Some(reason.to_string());
}

async fn refresh_shutdown_status_headers(bot: &teloxide::Bot, config: &Arc<AppConfig>) {
    let sessions = config
        .sessions
        .iter()
        .map(|entry| {
            (
                *entry.key(),
                entry.value().last_menu_id,
                entry.value().current_context.clone(),
            )
        })
        .collect::<Vec<_>>();

    if sessions.is_empty() {
        return;
    }

    let refresh = async {
        for (user_id, message_id, context) in sessions {
            if let Err(error) = crate::bot::handlers::refresh_current_view(
                bot,
                config,
                user_id,
                ChatId(user_id as i64),
                MessageId(message_id),
                &context,
            )
            .await
            {
                debug!(
                    "Failed to refresh shutdown status for user {}: {}",
                    user_id, error
                );
            }
        }
    };

    if tokio::time::timeout(Duration::from_secs(6), refresh)
        .await
        .is_err()
    {
        warn!("Shutdown status refresh timed out");
    }
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() -> &'static str {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to listen for SIGTERM");

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if let Err(error) = result {
                error!("Failed to listen for Ctrl+C: {}", error);
            }
            "SIGINT"
        }
        _ = sigterm.recv() => "SIGTERM",
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() -> &'static str {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!("Failed to listen for Ctrl+C: {}", error);
    }
    "SIGINT"
}
