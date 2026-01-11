use futures_util::{SinkExt, StreamExt};
use log::{error, info};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_util::sync::CancellationToken;
use tungstenite::Utf8Bytes;


pub fn spawn_event_listener(
    url: String,
    token: String,
    cancel_token: CancellationToken,
    tx: mpsc::Sender<super::models::NotifyEvent>) {

    tokio::spawn(async move {
        tokio::select! {
            _ = start_event_listener(url, token, cancel_token.clone(), tx) => {
                info!("Event listener finished.");
            }
            _ = cancel_token.cancelled() => {
                info!("Event listener cancelled.");
            }
        }
    });
}

async fn start_event_listener(
    ha_url: String,
    ha_token: String,
    cancel_token: CancellationToken,
    tx: mpsc::Sender<super::models::NotifyEvent>
) {
    let ws_url = ha_url.replace("http", "ws").trim_end_matches('/').to_string() + "/api/websocket";

    loop {
        if cancel_token.is_cancelled() { return; }

        info!("Connect WebSocket HA: {}", ws_url);

        let (ws_stream, _) = match connect_async(&ws_url).await {
            Ok(s) => s,
            Err(e) => {
                error!("Connection to WS failed: {}. throw 5 sec...", e);
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                    _ = cancel_token.cancelled() => return,
                }
            }
        };

        let (mut write, mut read) = ws_stream.split();
        let mut id_counter = 1;

        loop {
            tokio::select! {
                msg = read.next() => {
                    let Some(Ok(msg)) = msg else { break; }; // Если ошибка коннекта - идем на реконнект

                    let text = msg.to_text().unwrap_or("");
                    let v: Value = serde_json::from_str(text).unwrap_or_default();

                    match v["type"].as_str() {
                        Some("auth_required") => {
                            let _ = write.send(Message::Text(Utf8Bytes::from(json!({"type": "auth", "access_token": ha_token}).to_string()))).await;
                        }
                        Some("auth_ok") => {
                            info!("WebSocket HA: Auth complete.");
                            id_counter += 1;
                            let _ = write.send(Message::Text(Utf8Bytes::from(json!({
                                "id": id_counter,
                                "type": "subscribe_events",
                                "event_type": "state_changed"
                            }).to_string()))).await;
                        }
                        Some("event") => {
                            if v["event"]["event_type"] == "state_changed" {
                                let data = &v["event"]["data"];
                                let event = super::models::NotifyEvent {
                                    entity_id: data["entity_id"].as_str().unwrap_or_default().to_string(),
                                    old_state: data["old_state"]["state"].as_str().unwrap_or_default().to_string(),
                                    new_state: data["new_state"]["state"].as_str().unwrap_or_default().to_string(),
                                    friendly_name: data["new_state"]["attributes"]["friendly_name"].as_str().unwrap_or("Устройство").to_string(),
                                };

                                let _ = tx.send(event).await;
                            }
                        }
                        _ => {}
                    }
                }
                _ = cancel_token.cancelled() => {
                    info!("Close WebSocket connection...");
                    return;
                }
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {},
            _ = cancel_token.cancelled() => return,
        }
    }
}