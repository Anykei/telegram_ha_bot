use super::*;
use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

struct Telegram {
    bot: Bot,
    requests: mpsc::UnboundedReceiver<(String, Value)>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Telegram {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Telegram {
    async fn new() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let bot =
            Bot::new("123:TEST").set_api_url(format!("http://{}", listener.local_addr()?).parse()?);
        let (tx, requests) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut next = 100;
            while let Ok((socket, _)) = listener.accept().await {
                let mut socket = BufReader::new(socket);
                let mut line = String::new();
                socket.read_line(&mut line).await.unwrap();
                let method = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_owned();
                let mut length = 0;
                loop {
                    line.clear();
                    socket.read_line(&mut line).await.unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                let result = match method.as_str() {
                    "SendMessage" | "sendMessage" => {
                        next += 1;
                        json!({"message_id": next, "date": 1700000000, "chat": {"id": request["chat_id"], "type":"private"}, "text": request["text"]})
                    }
                    "DeleteMessage" | "deleteMessage" => json!(true),
                    _ => panic!("Unexpected Telegram method: {method}"),
                };
                tx.send((method.to_lowercase(), request)).unwrap();
                let response = json!({"ok":true,"result":result}).to_string();
                socket.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).as_bytes()).await.unwrap();
            }
        });
        Ok(Self {
            bot,
            requests,
            task,
        })
    }

    async fn request(&mut self) -> Result<(String, Value)> {
        Ok(timeout(Duration::from_secs(3), self.requests.recv())
            .await?
            .unwrap())
    }
}

#[tokio::test]
async fn notifications_expire_without_holding_up_the_next_delivery() -> Result<()> {
    let mut config = crate::test_support::config().await;
    Arc::get_mut(&mut config)
        .unwrap()
        .delete_notification_messages_timeout_s = 1;
    let mut api = Telegram::new().await?;
    timeout(Duration::from_millis(700), async {
        for text in ["ВКЛ", "ВЫКЛ", "ВКЛ"] {
            send_notification_text_to_recipient(api.bot.clone(), config.clone(), 42, text.into())
                .await?;
        }
        anyhow::Ok(())
    })
    .await??;
    for text in ["ВКЛ", "ВЫКЛ", "ВКЛ"] {
        let (method, body) = api.request().await?;
        assert_eq!(method, "sendmessage");
        assert_eq!(body["text"], text);
        assert_eq!(body["parse_mode"], "MarkdownV2");
    }
    assert!(timeout(Duration::from_millis(50), api.requests.recv())
        .await
        .is_err());
    let mut deleted = Vec::new();
    for _ in 0..3 {
        let (method, body) = api.request().await?;
        assert_eq!(method, "deletemessage");
        assert_eq!(body["chat_id"], 42);
        deleted.push(body["message_id"].as_i64().unwrap());
    }
    deleted.sort_unstable();
    assert_eq!(deleted, [101, 102, 103]);
    Ok(())
}

#[tokio::test]
async fn explicit_zero_keeps_notification_messages() -> Result<()> {
    let config = crate::test_support::config().await;
    let mut api = Telegram::new().await?;
    send_notification_text_to_recipient(api.bot.clone(), config, 42, "ВКЛ".into()).await?;
    assert_eq!(api.request().await?.0, "sendmessage");
    assert!(timeout(Duration::from_millis(50), api.requests.recv())
        .await
        .is_err());
    Ok(())
}

async fn subscriber() -> Result<Arc<AppConfig>> {
    let mut config = crate::test_support::config().await;
    Arc::get_mut(&mut config).unwrap().html_ui.enabled = true;
    crate::db::add_user(42, &config.db).await?;
    crate::db::rooms::sync_rooms_from_ha("hall", "Коридор", &config.db).await?;
    crate::db::devices::sync_device("light.hall", "hall", "Свет", "light", &config.db).await?;
    crate::db::subscriptions::toggle_hidden("light.hall", &config.db).await?;
    crate::db::subscriptions::toggle_subscription(42, "light.hall", &config.db).await?;
    assert_eq!(
        crate::db::subscriptions::get_subscribers("light.hall", &config.db).await?,
        vec![42]
    );
    Ok(config)
}

#[tokio::test]
async fn delivery_rechecks_html_preference_and_preserves_admission_and_subscription_checks(
) -> Result<()> {
    use crate::db::interface::{set, InterfaceMode};
    use crate::models::NotificationScope;
    let config = subscriber().await?;
    let mut api = Telegram::new().await?;
    let generation = crate::db::admission_generation(42, &config.db)
        .await?
        .unwrap();
    let scope = NotificationScope::Device("light.hall".into());

    // Capture the delivery context in classic mode, then change the preference
    // before dispatch. Suppression works without an active widget/session too.
    set(42, InterfaceMode::Html, &config.db).await?;
    for critical in [false, true] {
        deliver_notification(
            api.bot.clone(),
            config.clone(),
            42,
            generation.clone(),
            "ВКЛ".into(),
            critical,
            scope.clone(),
        )
        .await?;
        assert!(api.requests.try_recv().is_err());
    }

    set(42, InterfaceMode::Classic, &config.db).await?;
    deliver_notification(
        api.bot.clone(),
        config.clone(),
        42,
        generation.clone(),
        "ВЫКЛ".into(),
        false,
        scope.clone(),
    )
    .await?;
    assert_eq!(api.request().await?.1["text"], "ВЫКЛ");

    deliver_notification(
        api.bot.clone(),
        config.clone(),
        42,
        "revoked".into(),
        "ВКЛ".into(),
        false,
        scope.clone(),
    )
    .await?;
    assert!(api.requests.try_recv().is_err());
    crate::db::subscriptions::toggle_subscription(42, "light.hall", &config.db).await?;
    deliver_notification(
        api.bot.clone(),
        config,
        42,
        generation,
        "ВКЛ".into(),
        false,
        scope,
    )
    .await?;
    assert!(api.requests.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn html_state_messages_do_not_occupy_delivery_queue() -> Result<()> {
    use crate::core::notification_delivery::{enqueue, hold_delivery};
    use crate::db::interface::{set, InterfaceMode};
    use crate::models::NotificationScope;
    let mut config = subscriber().await?;
    let _held = hold_delivery(&config, 4);
    set(42, InterfaceMode::Html, &config.db).await?;
    let mut data = NotificationData {
        recipients: vec![42],
        human_state: "ВКЛ".into(),
        critical: false,
        scope: NotificationScope::Device("light.hall".into()),
    };
    enqueue(&config, data.clone()).await?;
    data.critical = true;
    enqueue(&config, data.clone()).await?;
    assert_eq!(config.runtime_status.read().await.notifications_queued, 0);
    assert_eq!(config.runtime_status.read().await.notification_overflow, 0);

    data.scope = NotificationScope::Camera(1);
    enqueue(&config, data.clone()).await?;
    assert_eq!(config.runtime_status.read().await.notifications_queued, 1);

    data.scope = NotificationScope::Device("light.hall".into());
    set(42, InterfaceMode::Classic, &config.db).await?;
    enqueue(&config, data.clone()).await?;
    assert_eq!(config.runtime_status.read().await.notifications_queued, 2);

    // A stored HTML preference must not silence classic-only deployments.
    set(42, InterfaceMode::Html, &config.db).await?;
    Arc::get_mut(&mut config).unwrap().html_ui.enabled = false;
    enqueue(&config, data).await?;
    assert_eq!(config.runtime_status.read().await.notifications_queued, 3);
    Ok(())
}
