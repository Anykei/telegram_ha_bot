pub(crate) mod handlers;
mod utils;
pub(crate) mod notification;
pub(crate) mod models;
mod screens;
pub(crate) mod router;

use std::sync::Arc;
use teloxide::{
    dispatching::UpdateHandler,
    prelude::*,
};

pub use models::State;

pub fn init(token: String) -> Bot {
    Bot::new(token)
}

pub fn schema() -> UpdateHandler<anyhow::Error>  {
    use teloxide::dispatching::dialogue::InMemStorage;
    use teloxide::types::Update;
    use crate::models::AppConfig;
    use crate::bot::models::State;
    use crate::db;

    let auth_filter = dptree::filter_async(|update: Update, config: Arc<AppConfig>| async move {
        let Some(user) = update.from() else {
            return false;
        };

        let user_id = user.id.0;

        if config.root_user == user_id {
            return true;
        }
        db::user_exists(&config.db, user_id).await
    });

    let command_handler = Update::filter_message()
        .filter_command::<handlers::Command>()
        .endpoint(handlers::handle_command);

    let callback_handler = Update::filter_callback_query()
        .endpoint(handlers::handle_callback);

    dptree::entry()
        .enter_dialogue::<Update, InMemStorage<State>, State>()
        .chain(auth_filter)
        .branch(command_handler)
        .branch(callback_handler)
        .endpoint(|update: Update| async move {
            warn!("No catch message: {:?}", update.id);
            Ok::<(), anyhow::Error>(())
        })
}