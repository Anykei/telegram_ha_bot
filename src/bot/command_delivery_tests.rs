use super::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Notify};

#[derive(Default)]
struct CommandHa {
    writes: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::ha::HomeAssistantClient for CommandHa {
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn fetch_rooms(&self) -> Result<Vec<crate::ha::Room>> {
        Ok(Vec::new())
    }
    async fn fetch_action_entities(&self) -> Result<Vec<crate::ha::models::Entity>> {
        Ok(Vec::new())
    }
    async fn fetch_history(
        &self,
        _: &str,
        _: u32,
        _: i32,
    ) -> Result<crate::ha::client::HistoryResult> {
        anyhow::bail!("No history request expected")
    }
    async fn fetch_states_by_ids(&self, ids: &[String]) -> Result<Vec<crate::ha::models::Entity>> {
        Ok(ids
            .iter()
            .map(|id| crate::ha::models::Entity {
                entity_id: id.clone(),
                name: "Люстра".into(),
                state: "on".into(),
                device_class: None,
                attributes: Default::default(),
            })
            .collect())
    }
    async fn call_service(&self, domain: &str, service: &str, id: &str) -> Result<()> {
        assert_eq!((domain, service, id), ("light", "turn_off", "light.test"));
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn call_service_with_data(
        &self,
        domain: &str,
        service: &str,
        id: &str,
        _: Value,
    ) -> Result<()> {
        self.call_service(domain, service, id).await
    }
}

async fn fixture() -> Result<(Arc<AppConfig>, Arc<CommandHa>, i64)> {
    let mut config = crate::test_support::config().await;
    let ha = Arc::new(CommandHa::default());
    Arc::get_mut(&mut config).unwrap().ha_client = ha.clone();
    db::add_user(42, &config.db).await?;
    db::rooms::sync_rooms_from_ha("room", "Room", &config.db).await?;
    sqlx::query("INSERT INTO devices(id,room_id,entity_id,alias,critical) VALUES(1,1,'light.test','Люстра',1)")
        .execute(&config.db).await?;
    let CommandExecution::NeedsConfirmation { pending_id } = crate::core::commands::execute_text(
        42,
        false,
        CommandSource::Text,
        "выключи люстру",
        &config,
    )
    .await?
    else {
        anyhow::bail!("Expected a persisted critical confirmation")
    };
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Command(CommandPayload::Result { id: pending_id }).to_string(),
        UiMessageMode::Text,
    )
    .await;
    Ok((config, ha, pending_id))
}

// Read both JSON requests and streamed photo uploads. The fake API records the
// complete body so an accidental receipt send cannot pass as an unrelated edit.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Result<Value> {
    let mut bytes = Vec::new();
    let body_start = loop {
        read_more(socket, &mut bytes).await?;
        if let Some(index) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..body_start]);
    let method = headers.lines().next().unwrap().to_lowercase();
    let length = headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().unwrap())
    });
    let body = if let Some(length) = length {
        while bytes.len() < body_start + length {
            read_more(socket, &mut bytes).await?;
        }
        bytes[body_start..body_start + length].to_vec()
    } else {
        let mut body = Vec::new();
        let mut cursor = body_start;
        loop {
            let end = loop {
                if let Some(index) = bytes[cursor..].windows(2).position(|s| s == b"\r\n") {
                    break cursor + index;
                }
                read_more(socket, &mut bytes).await?;
            };
            let size = usize::from_str_radix(std::str::from_utf8(&bytes[cursor..end])?, 16)?;
            cursor = end + 2;
            if size == 0 {
                break;
            }
            while bytes.len() < cursor + size + 2 {
                read_more(socket, &mut bytes).await?;
            }
            body.extend_from_slice(&bytes[cursor..cursor + size]);
            cursor += size + 2;
        }
        body
    };
    let mut parsed = serde_json::from_slice::<Value>(&body)
        .unwrap_or_else(|_| json!({"multipart": String::from_utf8_lossy(&body)}));
    if let Some(body) = parsed.get("multipart").and_then(Value::as_str) {
        if let Some(id) = body
            .split("name=\"message_id\"")
            .nth(1)
            .and_then(|field| field.split("\r\n\r\n").nth(1))
            .and_then(|value| value.split("\r\n").next())
            .and_then(|value| value.parse::<i32>().ok())
        {
            parsed["message_id"] = id.into();
        }
    }
    parsed["method"] = method.into();
    Ok(parsed)
}

async fn read_more(socket: &mut tokio::net::TcpStream, bytes: &mut Vec<u8>) -> Result<()> {
    let mut buffer = [0; 4096];
    let size = socket.read(&mut buffer).await?;
    anyhow::ensure!(size > 0, "Incomplete HTTP request");
    bytes.extend_from_slice(&buffer[..size]);
    Ok(())
}

struct TelegramApi {
    bot: Bot,
    requests: mpsc::UnboundedReceiver<Value>,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Drop for TelegramApi {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn telegram_api(delete_release: Option<Arc<Notify>>) -> Result<TelegramApi> {
    telegram_api_paused(delete_release, None).await
}

async fn telegram_api_paused(
    delete_release: Option<Arc<Notify>>,
    send_release: Option<Arc<Notify>>,
) -> Result<TelegramApi> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?).parse()?;
    let (tx, requests) = mpsc::unbounded_channel();
    let pause_delete = Arc::new(AtomicBool::new(delete_release.is_some()));
    let pause_send = Arc::new(AtomicBool::new(send_release.is_some()));
    let next_message = Arc::new(AtomicI32::new(100));
    let deleted_messages = Arc::new(std::sync::Mutex::new(
        std::collections::HashSet::<i32>::new(),
    ));
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut socket, _) = accepted?;
                    let tx = tx.clone();
                    let release = delete_release.clone();
                    let pause = pause_delete.clone();
                    let send_release = send_release.clone();
                    let pause_send = pause_send.clone();
                    let next_message = next_message.clone();
                    let deleted_messages = deleted_messages.clone();
                    connections.spawn(async move {
                        let request = read_request(&mut socket).await?;
                        let method = request["method"].as_str().unwrap().to_owned();
                        let message_id = request["message_id"].as_i64().map(|id| id as i32);
                        tx.send(request)?;
                        let deleted = method.contains("deletemessage");
                        if deleted && pause.swap(false, Ordering::SeqCst) {
                            release.unwrap().notified().await;
                        }
                        if !deleted && pause_send.swap(false, Ordering::SeqCst) {
                            send_release.unwrap().notified().await;
                        }
                        let missing = method.contains("editmessage") && message_id.is_some_and(|id| deleted_messages.lock().unwrap().contains(&id));
                        let status = if missing { 400 } else { 200 };
                        let body = if deleted {
                            if let Some(id) = message_id {
                                deleted_messages.lock().unwrap().insert(id);
                            }
                            json!({"ok":true,"result":true})
                        } else if missing {
                            json!({"ok":false,"error_code":400,"description":"Bad Request: message to edit not found"})
                        } else {
                            let mut message = json!({
                                "message_id":next_message.fetch_add(1, Ordering::SeqCst),
                                "date":1_700_000_000,"chat":{"id":42,"type":"private"}
                            });
                            if method.contains("photo") {
                                message["photo"] = json!([{"file_id":"photo", "file_unique_id":"photo", "width":1,"height":1}]);
                            } else {
                                message["text"] = "ok".into();
                            }
                            json!({"ok":true,"result":message})
                        }.to_string();
                        // A cancelled HTTP waiter may have already closed its socket.
                        let _ = socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
                        Ok::<_, anyhow::Error>(())
                    });
                }
                finished = connections.join_next(), if !connections.is_empty() => {
                    finished.unwrap()??;
                }
            }
        }
    });
    let client = teloxide::net::default_reqwest_settings()
        .no_proxy()
        .build()?;
    Ok(TelegramApi {
        bot: Bot::with_client("123:test", client).set_api_url(url),
        requests,
        task,
    })
}

async fn next_request(api: &mut TelegramApi) -> Result<Value> {
    tokio::time::timeout(StdDuration::from_secs(5), api.requests.recv())
        .await?
        .context("Fake Telegram stopped")
}

async fn assert_no_more_requests(api: &mut TelegramApi) -> Result<()> {
    // A successful sentinel establishes ordering without sleeping: after the
    // callback has completed, every accidental request is already in this FIFO.
    api.bot.send_message(ChatId(42), "END_OF_TEST").await?;
    assert_eq!(next_request(api).await?["text"], "END_OF_TEST");
    Ok(())
}

async fn assert_receipt(config: &AppConfig, ha: &CommandHa, pending_id: i64) -> Result<()> {
    assert_eq!(ha.writes.load(Ordering::SeqCst), 1);
    let receipt = db::pending_commands::get_pending(pending_id, 42, &config.db)
        .await?
        .context("Receipt disappeared")?;
    assert_eq!(receipt.status, "executed");
    let result: crate::core::action_results::ActionResult =
        serde_json::from_str(receipt.result_json.as_deref().context("No saved result")?)?;
    assert_eq!(result.outcome, crate::core::action_results::Outcome::Sent);
    assert!(receipt.result_text.is_some());
    Ok(())
}

async fn assert_session(config: &AppConfig, payload: Payload, message_id: i32) -> Result<()> {
    let expected = payload.to_string();
    let session = config.sessions.get(&42).context("Memory session missing")?;
    assert_eq!(session.current_context, expected);
    assert_eq!(session.last_menu_id, message_id);
    drop(session);
    let persisted: (i32, String) =
        sqlx::query_as("SELECT last_menu_id,current_context FROM users WHERE id=42")
            .fetch_one(&config.db)
            .await?;
    assert_eq!(persisted, (message_id, expected));
    Ok(())
}

async fn wait_for_ui_lock_waiter(lock: &Arc<tokio::sync::Mutex<()>>) -> Result<()> {
    tokio::time::timeout(StdDuration::from_secs(5), async {
        // The registry and test own two Arcs. The delivery obtains a third Arc
        // immediately before awaiting this held lock. This observes the actual
        // boundary without choosing a scheduling delay.
        while Arc::strong_count(lock) < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn confirmed_command_receipt_cannot_replace_home_after_delete_message_wait() -> Result<()> {
    let (config, ha, pending_id) = fixture().await?;
    crate::core::update_user_state_with_mode(
        &config,
        42,
        7,
        &Payload::Command(CommandPayload::Result { id: pending_id }).to_string(),
        UiMessageMode::Photo,
    )
    .await;
    let release = Arc::new(Notify::new());
    let mut api = telegram_api(Some(release.clone())).await?;
    let original = config.begin_view_update(42);
    let task_token = original.clone();
    let task_config = config.clone();
    let bot = api.bot.clone();
    let callback = tokio::spawn(async move {
        handle_command_callback(
            &bot,
            ChatId(42),
            MessageId(7),
            42,
            CommandPayload::Confirm { id: pending_id },
            task_config,
            &task_token,
        )
        .await
    });
    assert!(next_request(&mut api).await?["method"]
        .as_str()
        .unwrap()
        .contains("deletemessage"));
    assert_receipt(&config, &ha, pending_id).await?;

    let home_token = config.begin_view_update(42);
    assert!(original.is_cancelled());
    let home = router(Payload::Home, 42, config.clone()).await?;
    let lock = config.ui_lock_for(42);
    let before_home = Arc::strong_count(&lock);
    let task_config = config.clone();
    let bot = api.bot.clone();
    let home_update = tokio::spawn(async move {
        update_view_cancellable(
            &bot,
            ChatId(42),
            MessageId(7),
            42,
            home,
            task_config,
            Some(&home_token),
        )
        .await
    });
    tokio::time::timeout(StdDuration::from_secs(5), async {
        while Arc::strong_count(&lock) <= before_home {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(matches!(
        api.requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    release.notify_one();
    tokio::time::timeout(StdDuration::from_secs(5), callback).await???;
    tokio::time::timeout(StdDuration::from_secs(5), home_update).await???;
    let attempted_edit = next_request(&mut api).await?;
    assert!(attempted_edit["method"]
        .as_str()
        .unwrap()
        .contains("editmessage"));
    assert_eq!(attempted_edit["message_id"], 7);
    // Fake Telegram has actually deleted anchor 7, so this Home edit receives
    // MessageToEditNotFound and must reanchor to a new message.
    let home_request = next_request(&mut api).await?;
    assert!(!home_request.to_string().contains("Команды отправлены"));
    let cleanup = next_request(&mut api).await?;
    assert!(cleanup["method"]
        .as_str()
        .unwrap()
        .contains("deletemessage"));
    assert_eq!(cleanup["message_id"], 7);
    assert_no_more_requests(&mut api).await?;
    assert_session(&config, Payload::Home, 100).await?;
    assert_receipt(&config, &ha, pending_id).await?;
    Ok(())
}

#[tokio::test]
async fn confirmed_command_receipt_cannot_replace_home_after_waiting_for_ui_lock() -> Result<()> {
    let (config, ha, pending_id) = fixture().await?;
    let release = Arc::new(Notify::new());
    let mut api = telegram_api(Some(release.clone())).await?;
    let lock = config.ui_lock_for(42);
    let original = config.begin_view_update(42);
    let task_token = original.clone();
    let task_config = config.clone();
    let bot = api.bot.clone();
    let callback = tokio::spawn(async move {
        handle_command_callback(
            &bot,
            ChatId(42),
            MessageId(7),
            42,
            CommandPayload::Confirm { id: pending_id },
            task_config,
            &task_token,
        )
        .await
    });
    assert!(next_request(&mut api).await?["method"]
        .as_str()
        .unwrap()
        .contains("deletemessage"));
    let waiting = lock.lock();
    tokio::pin!(waiting);
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    release.notify_one();
    let held = tokio::time::timeout(StdDuration::from_secs(5), waiting).await?;
    wait_for_ui_lock_waiter(&lock).await?;
    let _home_token = config.begin_view_update(42);
    assert!(original.is_cancelled());
    // Model the newer Home update that owns this same UI lock. The old receipt
    // is already past deleteMessage and may continue only after Home commits.
    crate::core::update_user_state_with_mode(
        &config,
        42,
        77,
        &Payload::Home.to_string(),
        UiMessageMode::Text,
    )
    .await;
    drop(held);
    tokio::time::timeout(StdDuration::from_secs(5), callback).await???;
    assert_no_more_requests(&mut api).await?;
    assert_session(&config, Payload::Home, 77).await?;
    assert_receipt(&config, &ha, pending_id).await?;
    Ok(())
}

#[tokio::test]
async fn confirmed_command_delivers_and_saves_receipt_when_view_is_current() -> Result<()> {
    let (config, ha, pending_id) = fixture().await?;
    let mut api = telegram_api(None).await?;
    let token = config.begin_view_update(42);
    handle_command_callback(
        &api.bot,
        ChatId(42),
        MessageId(7),
        42,
        CommandPayload::Confirm { id: pending_id },
        config.clone(),
        &token,
    )
    .await?;
    assert!(next_request(&mut api).await?["method"]
        .as_str()
        .unwrap()
        .contains("deletemessage"));
    let receipt = next_request(&mut api).await?;
    assert!(receipt.to_string().contains("Команды отправлены"));
    assert_session(
        &config,
        Payload::Command(CommandPayload::Result { id: pending_id }),
        100,
    )
    .await?;
    assert_receipt(&config, &ha, pending_id).await?;
    assert_no_more_requests(&mut api).await?;
    Ok(())
}

#[tokio::test]
async fn chosen_target_confirmation_respects_cancellation_while_waiting_for_ui_lock() -> Result<()>
{
    for cancelled in [false, true] {
        let (config, ha, _) = fixture().await?;
        let context = crate::core::commands::ChoiceContext {
            action: crate::core::commands::ChoiceAction::Device {
                action: crate::core::commands::SerializableDeviceAction::TurnOff,
                resolved: Vec::new(),
                remaining: Vec::new(),
            },
            candidates: vec![1],
            text: "выключи люстру".into(),
            source: CommandSource::Text,
            engine: "local".into(),
            back: Payload::Home,
        };
        let choice_id =
            db::command_choices::create(42, &serde_json::to_string(&context)?, &config.db).await?;
        let mut api = telegram_api(None).await?;
        let lock = config.ui_lock_for(42);
        let held = lock.lock().await;
        let token = config.begin_view_update(42);
        let task_token = token.clone();
        let task_config = config.clone();
        let bot = api.bot.clone();
        let callback = tokio::spawn(async move {
            handle_command_callback(
                &bot,
                ChatId(42),
                MessageId(7),
                42,
                CommandPayload::Choose {
                    id: choice_id,
                    target: 1,
                },
                task_config,
                &task_token,
            )
            .await
        });
        wait_for_ui_lock_waiter(&lock).await?;
        let pending_id: i64 =
            sqlx::query_scalar("SELECT MAX(id) FROM pending_commands WHERE user_id=42")
                .fetch_one(&config.db)
                .await?;
        if cancelled {
            let _home_token = config.begin_view_update(42);
            assert!(token.is_cancelled());
            crate::core::update_user_state_with_mode(
                &config,
                42,
                77,
                &Payload::Home.to_string(),
                UiMessageMode::Text,
            )
            .await;
        }
        drop(held);
        tokio::time::timeout(StdDuration::from_secs(5), callback).await???;
        assert_eq!(ha.writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            db::command_choices::get(choice_id, 42, &config.db)
                .await?
                .unwrap()
                .status,
            "selected"
        );
        assert_eq!(
            db::pending_commands::get_pending(pending_id, 42, &config.db)
                .await?
                .unwrap()
                .status,
            "pending"
        );
        if cancelled {
            assert_session(&config, Payload::Home, 77).await?;
        } else {
            assert!(next_request(&mut api).await?.to_string().contains("Люстра"));
            assert_session(
                &config,
                Payload::Command(CommandPayload::Result { id: pending_id }),
                100,
            )
            .await?;
        }
        assert_no_more_requests(&mut api).await?;
    }
    Ok(())
}

#[tokio::test]
async fn cancellation_during_view_database_preparation_prevents_text_and_photo_send() -> Result<()>
{
    for text_mode in [false, true] {
        let (config, ha, pending_id) = fixture().await?;
        let mut api = telegram_api(None).await?;
        let token = config.begin_view_update(42);
        let mut view = router(Payload::Home, 42, config.clone()).await?;
        assert!(view.admission_generation.is_some());
        if text_mode {
            view.text = "Long prepared screen content. ".repeat(60);
        }
        assert_eq!(
            view_mode(&view.get_plain_text()) == UiMessageMode::Text,
            text_mode
        );
        let connection = config.db.acquire().await?;
        let bot = api.bot.clone();
        let sending = send_new_view_inner(&bot, ChatId(42), 42, view, config.clone(), Some(&token));
        tokio::pin!(sending);
        assert!(!token.is_cancelled());
        // The initial token check passes; the single-connection pool then
        // suspends the already-rendered view's admission/preparation query.
        assert!(futures_util::poll!(sending.as_mut()).is_pending());
        token.cancel();
        drop(connection);
        assert!(!tokio::time::timeout(StdDuration::from_secs(5), sending).await??);
        assert_no_more_requests(&mut api).await?;
        assert_session(
            &config,
            Payload::Command(CommandPayload::Result { id: pending_id }),
            7,
        )
        .await?;
        assert_eq!(ha.writes.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn cancelled_inflight_receipt_is_deleted_before_queued_home_persists() -> Result<()> {
    for text_mode in [false, true] {
        let (config, ha, pending_id) = fixture().await?;
        crate::core::commands::confirm_pending(42, false, pending_id, &config).await?;
        assert_receipt(&config, &ha, pending_id).await?;
        let mut receipt = router(
            Payload::Command(CommandPayload::Result { id: pending_id }),
            42,
            config.clone(),
        )
        .await?;
        if text_mode {
            receipt
                .text
                .push_str(&"\nAdditional completed command details".repeat(50));
        }
        assert_eq!(
            view_mode(&receipt.get_plain_text()) == UiMessageMode::Text,
            text_mode
        );
        let release = Arc::new(Notify::new());
        let mut api = telegram_api_paused(None, Some(release.clone())).await?;
        let old_token = config.begin_view_update(42);
        let task_token = old_token.clone();
        let task_config = config.clone();
        let old_bot = api.bot.clone();
        let old_send = tokio::spawn(async move {
            send_new_view_cancellable(
                &old_bot,
                ChatId(42),
                42,
                receipt,
                task_config,
                Some(&task_token),
            )
            .await
        });
        let submitted = next_request(&mut api).await?;
        assert!(submitted.to_string().contains("Команды отправлены"));
        let home_token = config.begin_view_update(42);
        assert!(old_token.is_cancelled());
        let home = router(Payload::Home, 42, config.clone()).await?;
        let lock = config.ui_lock_for(42);
        let before_home = Arc::strong_count(&lock);
        let task_config = config.clone();
        let home_bot = api.bot.clone();
        let home_send = tokio::spawn(async move {
            send_new_view_cancellable(
                &home_bot,
                ChatId(42),
                42,
                home,
                task_config,
                Some(&home_token),
            )
            .await
        });
        tokio::time::timeout(StdDuration::from_secs(5), async {
            while Arc::strong_count(&lock) <= before_home {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        // The newer screen is queued behind the still-in-flight old send.
        assert!(matches!(
            api.requests.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        release.notify_one();
        tokio::time::timeout(StdDuration::from_secs(5), old_send).await???;
        tokio::time::timeout(StdDuration::from_secs(5), home_send).await???;
        let cleanup = next_request(&mut api).await?;
        assert!(cleanup["method"]
            .as_str()
            .unwrap()
            .contains("deletemessage"));
        assert_eq!(cleanup["message_id"], 100);
        let home_request = next_request(&mut api).await?;
        assert!(!home_request.to_string().contains("Команды отправлены"));
        assert_session(&config, Payload::Home, 101).await?;
        assert_receipt(&config, &ha, pending_id).await?;
        assert_no_more_requests(&mut api).await?;
    }
    Ok(())
}
