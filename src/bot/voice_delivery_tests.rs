use super::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Notify};

struct Reply {
    status: u16,
    body: String,
    release: Option<Arc<Notify>>,
}

impl Reply {
    fn json(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            release: None,
        }
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Result<Value> {
    let mut bytes = Vec::new();
    let body_start = loop {
        let mut chunk = [0; 4096];
        let size = socket.read(&mut chunk).await?;
        anyhow::ensure!(size != 0, "Incomplete HTTP request");
        bytes.extend_from_slice(&chunk[..size]);
        if let Some(index) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..body_start]);
    let method = headers.lines().next().unwrap().to_owned();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < body_start + length {
        let mut chunk = [0; 4096];
        let size = socket.read(&mut chunk).await?;
        anyhow::ensure!(size != 0, "Incomplete HTTP body");
        bytes.extend_from_slice(&chunk[..size]);
    }
    let mut body = serde_json::from_slice::<Value>(&bytes[body_start..body_start + length])
        .unwrap_or(json!({}));
    body["method"] = method.into();
    Ok(body)
}

async fn respond(socket: &mut tokio::net::TcpStream, status: u16, body: &str) -> Result<()> {
    socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await?;
    Ok(())
}

fn message_reply(user: u64) -> Value {
    json!({"ok":true,"result":{"message_id":7,"date":1_700_000_000,"chat":{"id":user,"type":"private"},"text":"ok"}})
}

async fn api(
    user: u64,
    replies: Vec<Reply>,
) -> Result<(
    Bot,
    mpsc::UnboundedReceiver<Value>,
    tokio::task::JoinHandle<Result<()>>,
)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?).parse()?;
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        let mut replies = replies.into_iter();
        loop {
            let (mut socket, _) = listener.accept().await?;
            tx.send(read_request(&mut socket).await?)?;
            let reply = replies
                .next()
                .unwrap_or_else(|| Reply::json(message_reply(user)));
            if let Some(release) = reply.release {
                release.notified().await;
            }
            // Cancellation can close the client connection while a reply waits.
            let _ = respond(&mut socket, reply.status, &reply.body).await;
        }
    });
    let client = teloxide::net::default_reqwest_settings()
        .no_proxy()
        .build()?;
    Ok((
        Bot::with_client("123:test", client).set_api_url(url),
        rx,
        task,
    ))
}

async fn request(rx: &mut mpsc::UnboundedReceiver<Value>) -> Result<Value> {
    tokio::time::timeout(StdDuration::from_secs(5), rx.recv())
        .await?
        .context("API stopped")
}

async fn readonly_execution(config: &mut Arc<AppConfig>, user: u64) -> Result<CommandExecution> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let cfg = Arc::get_mut(config).unwrap();
    cfg.voice_command_engine = crate::options::VoiceCommandEngine::HaConversationReadonly;
    cfg.ha_url = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        let (socket, _) = listener.accept().await?;
        let mut ws = tokio_tungstenite::accept_async(socket).await?;
        ws.send(WsMessage::Text(
            json!({"type":"auth_required"}).to_string().into(),
        ))
        .await?;
        ws.next().await.context("No auth")??;
        ws.send(WsMessage::Text(
            json!({"type":"auth_ok","ha_version":"2026.9.0"})
                .to_string()
                .into(),
        ))
        .await?;
        ws.next().await.context("No recognition request")??;
        ws.send(WsMessage::Text(
            json!({"type":"result","id":1,"success":true,"result":{"results":[{
                "match":true,"intent":{"name":"HassGetState"},"details":{},
                "targets":{"light.private":{"matched":true}},"source":"builtin"
            }]}})
            .to_string()
            .into(),
        ))
        .await?;
        drop(ws);
        let (mut socket, _) = listener.accept().await?;
        let req = read_request(&mut socket).await?;
        assert!(req["method"]
            .as_str()
            .unwrap()
            .contains("/api/states/light.private"));
        respond(
            &mut socket,
            200,
            &json!({"entity_id":"light.private","state":"PRIVATEVOICESTATE","attributes":{}})
                .to_string(),
        )
        .await?;
        Ok::<_, anyhow::Error>(())
    });
    let execution =
        crate::core::commands::execute_voice_text(user, false, "какое состояние света", config)
            .await?;
    server.await??;
    assert!(
        matches!(&execution, CommandExecution::Done { message, displayed_resources, .. }
        if message.contains("PRIVATEVOICESTATE") && displayed_resources.len() == 1)
    );
    Ok(execution)
}

#[tokio::test]
async fn voice_answer_rechecks_sources_after_progress_delete_wait() -> Result<()> {
    for revoke in [
        "none",
        "room",
        "device",
        "admission",
        "voice",
        "cancel",
        "cancel_db",
    ] {
        let mut config = crate::test_support::config().await;
        db::add_user(42, &config.db).await?;
        db::rooms::sync_rooms_from_ha("room", "Room", &config.db).await?;
        sqlx::query(
            "INSERT INTO devices(id,room_id,entity_id,alias) VALUES(1,1,'light.private','Private')",
        )
        .execute(&config.db)
        .await?;
        let execution = readonly_execution(&mut config, 42).await?;
        let release = Arc::new(Notify::new());
        let (bot, mut requests, server) = api(
            42,
            vec![Reply {
                status: 200,
                body: json!({"ok":true,"result":true}).to_string(),
                release: Some(release.clone()),
            }],
        )
        .await?;
        let task_bot = bot.clone();
        let task_config = config.clone();
        let token = tokio_util::sync::CancellationToken::new();
        let task_token = token.clone();
        let task = tokio::spawn(async move {
            finish_voice_execution(
                &task_bot,
                ChatId(42),
                42,
                MessageId(7),
                execution,
                task_config,
                &task_token,
            )
            .await
        });
        assert!(request(&mut requests).await?["method"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("deletemessage"));
        match revoke {
            "room" => db::access::set_room_access(42, 1, false, false, &config.db).await?,
            "device" => {
                db::access::set_device_access(42, "light.private", false, false, &config.db).await?
            }
            "admission" => {
                db::delete_user(42, &config.db).await?;
                db::add_user(42, &config.db).await?;
            }
            "voice" => {
                db::access::toggle_user_voice_access(42, &config.db).await?;
            }
            "cancel" => token.cancel(),
            _ => {}
        }
        let held_connection = if revoke == "cancel_db" {
            Some(config.db.acquire().await?)
        } else {
            None
        };
        release.notify_one();
        if revoke == "cancel_db" {
            tokio::time::sleep(StdDuration::from_millis(20)).await;
            assert!(!task.is_finished());
            token.cancel();
        }
        tokio::time::timeout(StdDuration::from_secs(5), task).await???;
        drop(held_connection);
        if revoke == "none" {
            assert!(request(&mut requests).await?["text"]
                .as_str()
                .unwrap()
                .contains("PRIVATEVOICESTATE"));
        }
        bot.send_message(ChatId(42), "END_OF_TEST").await?;
        assert_eq!(
            request(&mut requests).await?["text"],
            "END_OF_TEST",
            "leak after {revoke}"
        );
        server.abort();
    }
    Ok(())
}

fn voice_directories(user: u64) -> Result<Vec<std::path::PathBuf>> {
    let prefix = format!("telegram_ha_bot_voice_{user}_");
    Ok(std::fs::read_dir(std::env::temp_dir())?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
        .collect())
}

#[tokio::test]
async fn downloaded_voice_is_removed_on_progress_error_and_cancellation() -> Result<()> {
    for (user, cancel) in [(8_700_001, false), (8_700_002, true)] {
        let mut config = crate::test_support::config().await;
        Arc::get_mut(&mut config).unwrap().voice_enabled = true;
        db::add_user(user, &config.db).await?;
        let release = Arc::new(Notify::new());
        let replies = vec![
            Reply::json(json!({"ok":true,"result":{"file_id":"voice","file_unique_id":"voice_unique","file_size":3,"file_path":"voice/test.oga"}})),
            Reply { status:200, body:"oga".into(), release:None },
            Reply { status:429, body:json!({"ok":false,"error_code":429,"description":"Too Many Requests","parameters":{"retry_after":1}}).to_string(), release:cancel.then(|| release.clone()) },
        ];
        let (bot, mut requests, server) = api(user, replies).await?;
        let msg: Message = serde_json::from_value(
            json!({"message_id":1,"date":1_700_000_000,"chat":{"id":user,"type":"private"},"voice":{"file_id":"voice","file_unique_id":"voice_unique","file_size":3,"duration":1,"mime_type":"audio/ogg"}}),
        )?;
        assert!(msg.voice().is_some());
        let task = tokio::spawn(async move {
            handle_voice_message_inner(
                &bot,
                &msg,
                user,
                ChatId(user as i64),
                config,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
        });
        assert!(request(&mut requests).await?["method"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("getfile"));
        assert!(request(&mut requests).await?["method"]
            .as_str()
            .unwrap()
            .contains("voice%2Ftest.oga"));
        assert!(request(&mut requests).await?["method"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("sendmessage"));
        if cancel {
            assert_eq!(voice_directories(user)?.len(), 1);
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            release.notify_one();
        } else {
            assert!(tokio::time::timeout(StdDuration::from_secs(5), task)
                .await??
                .is_err());
        }
        assert!(
            voice_directories(user)?.is_empty(),
            "temporary audio leaked"
        );
        server.abort();
    }
    Ok(())
}

#[tokio::test]
async fn confirmation_delivery_uses_fresh_text_after_device_revocation() -> Result<()> {
    let config = crate::test_support::config().await;
    db::add_user(42, &config.db).await?;
    db::rooms::sync_rooms_from_ha("room", "Room", &config.db).await?;
    sqlx::query("INSERT INTO devices(id,room_id,entity_id,alias,critical) VALUES(1,1,'light.private','PRIVATECONFIRMNAME',1)").execute(&config.db).await?;
    let execution = crate::core::commands::execute_text(
        42,
        false,
        CommandSource::Text,
        "выключи PRIVATECONFIRMNAME",
        &config,
    )
    .await?;
    let CommandExecution::NeedsConfirmation { pending_id } = &execution else {
        panic!("Expected confirmation")
    };
    assert!(
        crate::core::commands::pending_result_text(42, *pending_id, &config)
            .await?
            .contains("PRIVATECONFIRMNAME")
    );
    db::access::set_device_access(42, "light.private", false, false, &config.db).await?;
    let (bot, mut requests, server) = api(42, vec![]).await?;
    apply_command_execution(&bot, ChatId(42), 42, execution, config).await?;
    let sent = request(&mut requests).await?;
    assert!(!sent.to_string().contains("PRIVATECONFIRMNAME"));
    server.abort();
    Ok(())
}
