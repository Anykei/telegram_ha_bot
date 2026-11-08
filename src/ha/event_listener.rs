use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use serde_json::{json, Value};
use std::cmp::min;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_util::sync::CancellationToken;
use tungstenite::Utf8Bytes;

pub fn spawn_event_listener(
    url: String,
    token: String,
    cancel_token: CancellationToken,
    tx: mpsc::Sender<super::models::NotifyEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tokio::select! {
            _ = start_event_listener(url, token, cancel_token.clone(), tx) => {
                info!("Event listener finished.");
            }
            _ = cancel_token.cancelled() => {
                info!("Event listener cancelled.");
            }
        }
    })
}

async fn start_event_listener(
    ha_url: String,
    ha_token: String,
    cancel_token: CancellationToken,
    tx: mpsc::Sender<super::models::NotifyEvent>,
) {
    let ws_url = ha_url
        .replace("http", "ws")
        .trim_end_matches('/')
        .to_string()
        + "/api/websocket";
    let mut backoff = Duration::from_millis(500);
    let max_backoff = Duration::from_secs(30);

    loop {
        if cancel_token.is_cancelled() {
            return;
        }

        info!("Connect WebSocket HA: {}", ws_url);

        let (ws_stream, _) = match connect_async(&ws_url).await {
            Ok(s) => {
                backoff = Duration::from_millis(500); // Reset on successful connection
                s
            }
            Err(e) => {
                warn!(
                    "Connection to WS failed: {}. Retrying in {:?}...",
                    e, backoff
                );
                let sleep_for = backoff;
                backoff = min(backoff.saturating_mul(2), max_backoff);

                tokio::select! {
                    _ = tokio::time::sleep(sleep_for) => continue,
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
                    let received_at = chrono::Utc::now();

                    let text = msg.to_text().unwrap_or("");

                    // Skip empty messages (heartbeat/ping frames)
                    if text.is_empty() {
                        debug!("Received empty WebSocket frame (heartbeat)");
                        continue;
                    }

                    let v: Value = match serde_json::from_str(text) {
                        Ok(val) => val,
                        Err(e) => {
                            debug!("Failed to parse WebSocket message: {}. Payload: {}", e, text);
                            continue;
                        }
                    };

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
                        Some("event") if v["event"]["event_type"] == "state_changed" => {
                            debug!(
                                "Event HA: {}, Data: {}",
                                v["event"]["event_type"], v["event"]["data"]
                            );

                            let data = &v["event"]["data"];
                            let event = super::models::NotifyEvent {
                                received_at,
                                entity_id: data["entity_id"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                old_state: data["old_state"]["state"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                new_state: data["new_state"]["state"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                friendly_name: data["new_state"]["attributes"]["friendly_name"]
                                    .as_str()
                                    .unwrap_or("Устройство")
                                    .to_string(),
                                device_class: data["new_state"]["attributes"]["device_class"]
                                    .as_str()
                                    .map(String::from),
                            };

                            if !forward_event(&tx, event, &cancel_token).await {
                                return;
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

        let sleep_for = backoff;
        backoff = min(backoff.saturating_mul(2), max_backoff);

        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {},
            _ = cancel_token.cancelled() => return,
        }
    }
}

pub(crate) async fn forward_event(
    tx: &mpsc::Sender<super::models::NotifyEvent>,
    event: super::models::NotifyEvent,
    cancel: &CancellationToken,
) -> bool {
    if tx.capacity() == 0 {
        warn!(
            "operation=ha_event stage=enqueue result=backpressure capacity={}",
            tx.max_capacity()
        );
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        result = tx.send(event) => result.is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn full_input_preserves_all_transitions_in_order() {
        let (tx, mut rx) = mpsc::channel(100);
        let cancel = CancellationToken::new();
        let sender = tokio::spawn(async move {
            for index in 0..1000 {
                assert!(
                    forward_event(
                        &tx,
                        super::super::models::NotifyEvent {
                            new_state: index.to_string(),
                            ..Default::default()
                        },
                        &cancel
                    )
                    .await
                );
            }
        });
        for index in 0..1000 {
            assert_eq!(rx.recv().await.unwrap().new_state, index.to_string());
        }
        sender.await.unwrap();
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn backpressure_send_can_be_cancelled_and_closed() {
        let (tx, rx) = mpsc::channel(1);
        tx.send(Default::default()).await.unwrap();
        let cancel = CancellationToken::new();
        let pending = forward_event(&tx, Default::default(), &cancel);
        tokio::pin!(pending);
        assert!(futures_util::poll!(&mut pending).is_pending());
        cancel.cancel();
        assert!(!pending.await);
        drop(rx);
        assert!(!forward_event(&tx, Default::default(), &CancellationToken::new()).await);
    }

    #[tokio::test]
    async fn backpressure_preserves_original_received_time() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(Default::default()).await.unwrap();
        let cancel = CancellationToken::new();
        let received_at = chrono::Utc::now() - chrono::Duration::seconds(30);
        let event = crate::ha::NotifyEvent {
            entity_id: "binary_sensor.door".into(),
            old_state: "off".into(),
            new_state: "on".into(),
            received_at,
            ..Default::default()
        };
        let pending = forward_event(&tx, event, &cancel);
        tokio::pin!(pending);
        assert!(futures_util::poll!(pending.as_mut()).is_pending());
        rx.recv().await.unwrap();
        assert!(pending.await);
        let forwarded = rx.recv().await.unwrap();
        assert_eq!(forwarded.received_at, received_at);
        assert_eq!(forwarded.entity_id, "binary_sensor.door");
        assert_eq!(forwarded.new_state, "on");
    }
}
