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
