pub(crate) mod assist_pipeline;
pub(crate) mod client;
pub(crate) mod conversation;
mod event_listener;
pub(crate) mod intent_recognition;
pub(crate) mod models;
mod templates;

pub use client::{HAClient, HomeAssistantClient};

pub use event_listener::spawn_event_listener;

pub use models::{NotifyEvent, Room};

pub fn init(url: String, token: String) -> HAClient {
    HAClient::new(url, token, 10, 5)
}
