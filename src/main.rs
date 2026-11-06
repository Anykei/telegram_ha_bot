use anyhow::{Context, Result};
use chrono::Utc;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::Dispatcher;
use teloxide::dptree;
use teloxide::error_handlers::LoggingErrorHandler;
use teloxide::prelude::Requester;
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
#[cfg(test)]
mod migration_tests;
mod models;
mod options;
#[cfg(test)]
mod test_support;
mod widget_demo;

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

    let paths = EnvPaths::load();
    let mut options = AppOptions::load(&paths.options).context("Error load options.json.")?;
    anyhow::ensure!(
        !(options.widget_demo_enabled && options.ha_mock_enabled),
        "Use widget_demo_enabled=false for the regular bot with ha_mock_enabled=true"
    );
    if options.widget_demo_enabled {
        return widget_demo::run(&options, &paths.options).await;
    }
    let paths = if options.ha_mock_enabled {
        let paths = paths.with_mock_ha()?;
        // Voice recognition needs a real HA pipeline; typed commands still work.
        options.voice_enabled = false;
        options.voice_command_engine = options::VoiceCommandEngine::LocalParser;
        options.camera_recording_storage_root = paths
            .database
            .with_extension("recordings")
            .to_string_lossy()
            .into_owned();
        info!("HA MOCK: regular bot, virtual devices, no connection to Home Assistant");
        paths
    } else {
        paths
    }
    .validate()
    .context("Error checking env variables.")?;
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
    let new_root = !db::user_exists(options.root_user, &db_pool).await;
    db::add_user(options.root_user, &db_pool)
        .await
        .context("Error initializing root admission")?;

    if options.ha_mock_enabled && options.html_widgets_enabled && new_root {
        db::interface::set(
            options.root_user,
            db::interface::InterfaceMode::Html,
            &db_pool,
        )
        .await?;
    }
    let (tx, rx) = mpsc::channel::<ha::NotifyEvent>(100);
    let ha_client: Arc<dyn ha::HomeAssistantClient> = if options.ha_mock_enabled {
        let mock = ha::mock::MockHa::open(db_pool.clone(), tx.clone()).await?;
        mock.seed_inventory().await?;
        Arc::new(mock)
    } else {
        Arc::new(ha::init(paths.ha_url.clone(), paths.ha_token.clone()))
    };

    let (recording_tx, recording_rx) = mpsc::channel::<core::camera_recording::RecordingJob>(32);

    let app_config = Arc::new(AppConfig {
        html_ui: crate::bot::html_ui::HtmlUi::new(options.html_widgets_enabled),
        ha_client: ha_client.clone(),
        ha_url: paths.ha_url.clone(),
        ha_token: paths.ha_token.clone(),
        db: db_pool,
        root_user: options.root_user,

        delete_notification_messages_timeout_s: options.delete_notification_messages_timeout_s,
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
        admission_lifecycle: tokio::sync::Mutex::new(()),
        dialogue_generations: DashMap::new(),
        active_dialogues: DashMap::new(),
        ui_locks: DashMap::new(),
        view_updates: DashMap::new(),
        climate_controls: Default::default(),
        recording_sends_in_progress: DashMap::new(),

        name_aliases: DashMap::new(),

        state_aliases: DashMap::new(),
        ui_background_cache: tokio::sync::Mutex::new(None),
        camera_snapshot_cache: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        camera_pre_roll_registry: Arc::new(core::camera_pre_roll::CameraPreRollRegistry::new()),
        runtime_status: tokio::sync::RwLock::new(RuntimeStatus::default()),
        notification_delivery: Default::default(),
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
                last_persisted_at: Some(last_seen_at),
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

    let recording_storage_ready =
        match core::camera_recording::initialize_recording_storage(&app_config).await {
            Ok(()) => true,
            Err(error) => {
                error!("operation=recording stage=startup result=disabled reason={error}");
                false
            }
        };

    let event_listener_handle = if options.ha_mock_enabled {
        // Mock mutations feed the same notification/live-refresh pipeline directly.
        drop(tx);
        tokio::spawn(async {})
    } else {
        ha::spawn_event_listener(
            paths.ha_url.clone(),
            paths.ha_token.clone(),
            cancel_token.clone(),
            tx,
        )
    };

    info!("✅ Run Dispatcher...");

    let storage = InMemStorage::<bot::State>::new();
    let (mut dispatcher, _bot) = {
        let bot = bot::init(options.bot_token);
        let dispatcher = Dispatcher::builder(bot.clone(), bot::schema())
            .dependencies(dptree::deps![app_config.clone(), storage])
            .build();
        (dispatcher, bot)
    };

    let notification_delivery_handle = core::notification_delivery::spawn_worker(
        _bot.clone(),
        app_config.clone(),
        cancel_token.clone(),
    );
    let notification_handle = core::spawn_notification_processor(
        rx,
        _bot.clone(),
        app_config.clone(),
        cancel_token.clone(),
    );
    let recording_handle = if recording_storage_ready {
        core::camera_recording::spawn_recording_worker(
            recording_rx,
            _bot.clone(),
            app_config.clone(),
            cancel_token.clone(),
        )
    } else {
        drop(recording_rx);
        tokio::spawn(async {})
    };
    let camera_pre_roll_handle = core::camera_pre_roll::spawn_camera_pre_roll_worker(
        app_config.clone(),
        cancel_token.clone(),
    );
    let action_schedule_handle = core::action_schedules::spawn_action_schedule_worker(
        app_config.clone(),
        cancel_token.clone(),
    );
    let camera_health_handle = core::camera_health::spawn_camera_health_worker(
        _bot.clone(),
        app_config.clone(),
        cancel_token.clone(),
    );
    let maintenance_handle =
        core::spawn_background_maintenance(_bot.clone(), app_config.clone(), cancel_token.clone());
    let shutdown_signal_handle =
        spawn_shutdown_signal_handler(main_cancel_token, _bot.clone(), app_config.clone());

    let bot_task = async {
        if app_config.html_ui.enabled {
            // One polling stream handles both transports; Teloxide cannot decode widget_query.
            if let Err(error) = _bot.delete_webhook().await {
                log::warn!("Cannot clear Telegram webhook: {error}");
            }
            dispatcher
                .dispatch_with_listener(
                    bot::html_ui::transport::listener(_bot.clone()),
                    LoggingErrorHandler::with_custom_text("HTML update listener error"),
                )
                .await;
        } else {
            let update_listener = Polling::builder(_bot.clone())
                .timeout(Duration::from_secs(30))
                .delete_webhook()
                .await
                .build();
            dispatcher
                .dispatch_with_listener(
                    update_listener,
                    LoggingErrorHandler::with_custom_text("An error from the update listener"),
                )
                .await;
        }
    };

    tokio::select! {
        _ = bot_task => info!("Bot task completed successfully."),
        _ = cancel_token.cancelled() => info!("Bot task was canceled."),
    }

    info!("Graceful Shutdown...");
    if app_config
        .runtime_status
        .read()
        .await
        .shutdown_requested_at
        .is_none()
    {
        mark_shutdown_status(&app_config, "dispatcher stopped").await;
    }
    cancel_token.cancel();
    core::cameras::cancel_native_jobs();

    let shutdown_started = tokio::time::Instant::now();
    let climate_config = app_config.clone();
    let climate_shutdown = tokio::spawn(async move {
        if let Err(error) = climate_config.climate_controls.shutdown().await {
            error!("operation=climate stage=shutdown result=failed reason={error}");
        }
    });
    let admission_shutdown = tokio::spawn(async {
        // If the outer 12-second waiter aborts this task, cancel the remaining
        // UI/media sends; their guards and native owners are joined below.
        struct AbortOnDrop;
        impl Drop for AbortOnDrop {
            fn drop(&mut self) {
                db::abort_admission_tasks();
            }
        }
        let _abort = AbortOnDrop;
        db::wait_admission_tasks().await;
    });
    let handles = vec![
        ("ha_event_listener", event_listener_handle),
        ("notification_processor", notification_handle),
        ("notification_delivery", notification_delivery_handle),
        ("camera_recording_worker", recording_handle),
        ("camera_pre_roll_worker", camera_pre_roll_handle),
        ("action_schedule_worker", action_schedule_handle),
        ("camera_health_worker", camera_health_handle),
        ("background_maintenance", maintenance_handle),
        ("shutdown_signal", shutdown_signal_handle),
        ("climate_controls", climate_shutdown),
        ("admission_tasks", admission_shutdown),
    ];
    let deadline =
        shutdown_started + BACKGROUND_TASK_SHUTDOWN_TIMEOUT + BACKGROUND_TASK_ABORT_TIMEOUT;
    let shutdown = async {
        wait_for_background_tasks(handles).await;
        db::wait_admission_tasks().await;
        core::cameras::wait_native_jobs().await;
    };
    if tokio::time::timeout_at(deadline, shutdown).await.is_err() {
        error!("operation=shutdown result=emergency_exit reason=workers_or_native_io_exceeded_15s");
        // A non-interruptible native call cannot safely be killed as a thread.
        // Leave durable ownership records for startup recovery; do not claim a clean DB close.
        std::process::exit(1);
    }

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
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let reason = tokio::select! {
            reason = wait_for_shutdown_signal() => reason,
            _ = cancel_token.cancelled() => return,
        };
        info!("Received {}, preparing graceful shutdown", reason);

        mark_shutdown_status(&config, reason).await;
        cancel_token.cancel();
        core::cameras::cancel_native_jobs();
        refresh_shutdown_status_headers(&bot, &config).await;
    })
}

async fn mark_shutdown_status(config: &Arc<AppConfig>, reason: &str) {
    let mut status = config.runtime_status.write().await;
    status.shutdown_requested_at = Some(Utc::now());
    status.shutdown_reason = Some(reason.to_string());
    drop(status);
    config.climate_controls.stop_accepting();
    db::stop_admission_tasks();
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
