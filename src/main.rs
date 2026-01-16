use anyhow::{Context, Result};
use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::Dispatcher;
use teloxide::dptree;

extern crate pretty_env_logger;
#[macro_use] extern crate log;

use crate::config::EnvPaths;
use crate::options::AppOptions;
use crate::models::AppConfig;

mod db;
mod models;
mod ha;
mod config;
mod options;
mod bot;
mod core;

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

    let options = AppOptions::load(&paths.options)
        .context("Error load options.json.")?;

    let db_pool = db::init(&paths.db_url(), paths.migrations.to_str().context("Путь к миграциям не валиден")?)
        .await
        .context("Error initializing database pool.")?;

    let ha_client = Arc::new(ha::init(paths.ha_url.clone(), paths.ha_token.clone()));

    let app_config = Arc::new(AppConfig {
        ha_client: ha_client.clone(),
        db: db_pool,
        root_user: options.root_user,

        delete_chart_timeout_s: 600,
        delete_help_messages_timeout_s: 30,
        delete_notification_messages_timeout_s: 5,
        delete_error_messages_timeout_s: 5,
        leak_time_notification_m: 100,
        background_maintenance_interval_s:5,

        sessions: DashMap::new(),

        name_aliases: DashMap::new(),

        state_aliases: DashMap::new(),
    });

    info!("Load Backup sessions from database...");
    let active_sessions = db::get_all_active_sessions(&app_config.db).await?;

    for (uid, mid, context) in active_sessions {
        app_config.sessions.insert(uid as u64, crate::models::UserSession {
            last_menu_id: mid,
            current_context: context,
            header_entities: std::collections::HashSet::new(), // Это можно тоже хранить в БД, если нужно
        });
    }
    info!("Restored {} action sessions.", app_config.sessions.len());

    let names = db::get_aliases_map(&app_config.db).await;
    for (eid, name) in names {
        app_config.name_aliases.insert(eid, name);
    }

    let states = db::get_state_aliases(&app_config.db).await;
    for (eid, state_map) in states {
        app_config.state_aliases.insert(eid, state_map);
    }

    let (tx, rx) = mpsc::channel::<ha::NotifyEvent>(100);
    ha::spawn_event_listener(paths.ha_url.clone(), paths.ha_token.clone(), cancel_token.clone(), tx);

    info!("✅ Run Dispatcher...");

    tokio::spawn(async move {
        // Wait Ctrl+C or SIGTERM Docker/OS
        tokio::signal::ctrl_c().await.expect("Failed to listen for ctrl+c");
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
    core::spawn_background_maintenance(_bot.clone(), app_config.clone(), cancel_token.clone());

    let bot_task = dispatcher.dispatch();

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