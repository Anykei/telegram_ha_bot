pub(crate) mod client;
pub(crate) mod models;
mod templates;
mod event_listener;

pub use client::HAClient;

pub use event_listener::spawn_event_listener;

pub use models::{Room, NotifyEvent};

pub fn init(url:String, token: String) -> HAClient {
    HAClient::new(url, token, 10, 5)
}