mod models;

use teloxide::{
    dispatching::UpdateHandler,
    prelude::*,
};

pub use models::{State};

pub fn init(token: String) -> Bot {
    Bot::new(token)
}

pub fn schema() -> UpdateHandler<anyhow::Error>  {
    use teloxide::dispatching::dialogue::InMemStorage;
    use teloxide::types::Update;
    // use crate::models::AppConfig;
    use crate::bot::models::State;

    dptree::entry()
        .enter_dialogue::<Update, InMemStorage<State>, State>()
        .endpoint(|update: Update| async move {
            warn!("Необработанный апдейт: {:?}", update.id);
            Ok::<(), anyhow::Error>(())
        })
}