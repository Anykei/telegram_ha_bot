use super::super::personal::PersonalPayload;
use super::super::router::{router, ControlPayload, DeviceCmd};
use super::*;
use serde_json::{json, Value};
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[path = "release_tests.rs"]
mod release_tests;

#[derive(Default)]
struct FakeHa {
    calls: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl crate::ha::HomeAssistantClient for FakeHa {
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn fetch_rooms(&self) -> Result<Vec<crate::ha::Room>> {
        Ok(vec![])
    }
    async fn fetch_history(
        &self,
        _: &str,
        _: u32,
        _: i32,
    ) -> Result<crate::ha::client::HistoryResult> {
        anyhow::bail!("No history")
    }
    async fn fetch_states_by_ids(&self, ids: &[String]) -> Result<Vec<crate::ha::models::Entity>> {
        Ok(ids
            .iter()
            .map(|id| crate::ha::models::Entity {
                entity_id: id.clone(),
                name: "Свет в гостиной".into(),
                state: "off".into(),
                device_class: None,
                attributes: Default::default(),
            })
            .collect())
    }
    async fn fetch_action_entities(&self) -> Result<Vec<crate::ha::models::Entity>> {
        Ok(vec![])
    }
    async fn call_service(&self, domain: &str, service: &str, entity: &str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{domain}:{service}:{entity}"));
        Ok(())
    }
    async fn call_service_with_data(
        &self,
        domain: &str,
        service: &str,
        entity: &str,
        _: Value,
    ) -> Result<()> {
        self.call_service(domain, service, entity).await
    }
}
async fn config() -> Result<(Arc<AppConfig>, Arc<FakeHa>)> {
    let mut config = crate::test_support::config().await;
    let ha = Arc::new(FakeHa::default());
    let value = Arc::get_mut(&mut config).unwrap();
    value.html_ui.enabled = true;
    value.ha_client = ha.clone();
    db::add_user(1, &config.db).await?;
    let generation = db::admission_generation(1, &config.db).await?.unwrap();
    db::record_user_activity(1, &generation, chrono::Utc::now(), &config.db).await?;
    db::rooms::sync_rooms_from_ha("living", "Гостиная", &config.db).await?;
    db::devices::sync_device(
        "light.living",
        "living",
        "Свет в гостиной",
        "light",
        &config.db,
    )
    .await?;
    sqlx::query("DELETE FROM hidden_entities")
        .execute(&config.db)
        .await?;
    db::interface::set(1, db::interface::InterfaceMode::Html, &config.db).await?;
    Ok((config, ha))
}

struct FakeTelegram {
    bot: Bot,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    fail: Arc<Mutex<Option<String>>>,
    update_batches: Arc<Mutex<std::collections::VecDeque<Vec<Value>>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for FakeTelegram {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl FakeTelegram {
    async fn new() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let fail = Arc::new(Mutex::new(None::<String>));
        let failing = fail.clone();
        let update_batches = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let pending_updates = update_batches.clone();
        let task = tokio::spawn(async move {
            let mut id = 10;
            while let Ok((stream, _)) = listener.accept().await {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_err() {
                    break;
                }
                let method = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_string();
                let mut length = 0;
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
                recorded.lock().unwrap().push((method.clone(), body));
                let fail = {
                    let mut method_to_fail = failing.lock().unwrap();
                    if method_to_fail.as_deref() == Some(&method) {
                        method_to_fail.take();
                        true
                    } else {
                        false
                    }
                };
                id += 1;
                let result = if method == "getUpdates" {
                    json!(pending_updates
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or_default())
                } else if method.to_ascii_lowercase().starts_with("send")
                    || method.to_ascii_lowercase().starts_with("editmessage")
                {
                    json!({"message_id":id,"date":1,"chat":{"id":1,"type":"private"},"text":"Test"})
                } else {
                    json!(true)
                };
                let body = if fail {
                    json!({"ok":false,"description":"simulated failure"})
                } else {
                    json!({"ok":true,"result":result})
                }
                .to_string();
                reader.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
        });
        let bot = Bot::with_client(
            "123:test",
            teloxide::net::default_reqwest_settings()
                .no_proxy()
                .build()?,
        )
        .set_api_url(format!("http://{address}").parse()?);
        Ok(Self {
            bot,
            calls,
            fail,
            update_batches,
            task,
        })
    }
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
    fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
}
fn query(session: &WidgetSession, key: &str) -> CallbackQuery {
    let raw = json!({"update_id":1,"widget_query":{"id":"42","from":{"id":1,"is_bot":false,"first_name":"Test"},"chat":{"id":1,"type":"private"},"message_id":session.message,"data":format!("r1:{}:{key}",session.pages[0].rev)}});
    let teloxide::types::UpdateKind::CallbackQuery(q) = transport::normalize(&raw).unwrap().kind
    else {
        panic!()
    };
    q
}
async fn publish(
    fake: &FakeTelegram,
    config: &Arc<AppConfig>,
    payload: Payload,
) -> Result<WidgetSession> {
    let view = router(payload, 1, config.clone()).await?;
    assert!(
        handlers::send_new_view_inner(&fake.bot, ChatId(1), 1, view, config.clone(), None).await?
    );
    Ok(config
        .html_ui
        .sessions
        .get(&1)
        .context("No HTML session")?
        .clone())
}

#[test]
fn normalization_preserves_original_sender_and_chat_for_admission_filters() -> Result<()> {
    let raw = json!({"update_id":3,"widget_query":{"id":"q","from":{"id":7,"is_bot":false,"first_name":"U"},"chat":{"id":8,"type":"private"},"message_id":12,"data":"x"}});
    let update = transport::normalize(&raw)?;
    assert!(!crate::bot::is_verified_private_update(&update));
    let mut raw = raw;
    raw["widget_query"]["chat"]["id"] = json!(7);
    assert!(crate::bot::is_verified_private_update(
        &transport::normalize(&raw)?
    ));
    raw["widget_query"]["chat"]["type"] = json!("group");
    raw["widget_query"]["chat"]["title"] = json!("G");
    assert!(!crate::bot::is_verified_private_update(
        &transport::normalize(&raw)?
    ));
    raw["widget_query"]["inline_message_id"] = json!("inline");
    assert!(transport::normalize(&raw).is_err());
    Ok(())
}

#[tokio::test]
async fn polling_delivers_both_interfaces_and_advances_past_invalid_updates() -> Result<()> {
    use futures_util::StreamExt;
    use teloxide::{
        types::UpdateKind,
        update_listeners::{AsUpdateStream, UpdateListener},
    };

    let fake = FakeTelegram::new().await?;
    let user = json!({"id":7,"is_bot":false,"first_name":"Test"});
    let chat = json!({"id":7,"type":"private"});
    let message = json!({"message_id":12,"date":1,"chat":chat,"from":user,"text":"/start"});
    fake.update_batches.lock().unwrap().extend([
        vec![
            json!({"update_id":10,"future_update":{}}),
            json!({"update_id":11,"message":{"text":"malformed"}}),
            json!({"update_id":12,"callback_query":{"id":"classic","from":user,
                "chat_instance":"chat","data":"route","message":message}}),
            json!({"update_id":13,"widget_query":{"id":"widget","from":user,
                "chat":chat,"message_id":12,"data":"opaque-key"}}),
            json!({"update_id":14,"message":message}),
            json!({"update_id":15,"widget_query":{"id":"invalid","message_id":12}}),
            json!({"update_id":16,"future_update":{}}),
        ],
        vec![json!({"update_id":17,"message":message})],
    ]);
    let mut listener = transport::listener(fake.bot.clone());
    let stop = listener.stop_token();
    let updates = listener.as_stream();
    futures_util::pin_mut!(updates);
    let mut received = Vec::new();
    for id in [12, 13, 14, 17] {
        let update = tokio::time::timeout(std::time::Duration::from_secs(5), updates.next())
            .await?
            .context("Polling stopped unexpectedly")??;
        assert_eq!(update.id.0, id);
        assert!(crate::bot::is_verified_private_update(&update));
        received.push(update.kind);
    }
    assert!(
        matches!(&received[0], UpdateKind::CallbackQuery(q) if !is_widget(q) && q.data.as_deref() == Some("route"))
    );
    assert!(
        matches!(&received[1], UpdateKind::CallbackQuery(q) if is_widget(q) && q.data.as_deref() == Some("opaque-key"))
    );
    assert!(matches!(&received[2], UpdateKind::Message(m) if m.text() == Some("/start")));
    stop.stop();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), updates.next())
            .await?
            .is_none()
    );

    let calls = fake.calls();
    let polls = calls
        .iter()
        .filter(|(method, _)| method == "getUpdates")
        .collect::<Vec<_>>();
    assert_eq!(polls.len(), 2);
    assert_eq!(polls[0].1["offset"], 0);
    assert_eq!(polls[1].1["offset"], 17);
    assert_eq!(
        polls[1].1["allowed_updates"],
        json!(["message", "callback_query", "widget_query"])
    );
    assert!(calls
        .iter()
        .any(|(method, body)| method == "answerWidgetQuery"
            && body == &json!({"widget_query_id":"invalid","data":"{\"denied\":true}"})));
    Ok(())
}

#[tokio::test]
async fn real_routes_keep_every_action_and_every_text_byte_within_telegram_limits() -> Result<()> {
    let (config, ha) = config().await?;
    for payload in [
        Payload::Home,
        Payload::Control(ControlPayload::ListRooms),
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
        Payload::Personal(PersonalPayload::Settings),
        Payload::Personal(PersonalPayload::Interface),
        Payload::Control(ControlPayload::DeviceControl { room: 1, device: 1 }),
    ] {
        let view = router(payload, 1, config.clone()).await?;
        let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
        assert_eq!(
            prepared.actions.len(),
            view.kb.inline_keyboard.iter().map(Vec::len).sum::<usize>()
        );
        let mut visible = prepared
            .pages
            .iter()
            .flat_map(|p| p.rows.iter().flatten())
            .map(|b| b.text.clone())
            .collect::<Vec<_>>();
        let mut original = view
            .kb
            .inline_keyboard
            .iter()
            .flatten()
            .map(|b| b.text.clone())
            .collect::<Vec<_>>();
        visible.sort();
        original.sort();
        assert_eq!(visible, original);
        let body = prepared
            .pages
            .iter()
            .map(|p| p.body.as_str())
            .collect::<String>();
        assert!(body.contains(&view.text));
        for page in &prepared.pages {
            assert!(page.json()?.len() <= 1000);
            assert!(serde_json::to_string(&json!({"page":page}))?.len() <= 1024);
            assert!(page.rows.len() <= 4);
        }
    }
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn opaque_keys_are_bound_to_current_message_and_claimed_once() -> Result<()> {
    let (config, _) = config().await?;
    let fake = FakeTelegram::new().await?;
    let session = publish(&fake, &config, Payload::Home).await?;
    let (key, payload) = session.actions.iter().next().unwrap();
    let q = query(&session, key);
    assert_eq!(
        resolve(&fake.bot, &q, &config).await?,
        Some(payload.to_string())
    );
    assert_eq!(resolve(&fake.bot, &q, &config).await?, None);
    let stale = query(&session, "forged");
    assert_eq!(resolve(&fake.bot, &stale, &config).await?, None);
    publish(&fake, &config, Payload::Home).await?;
    assert_eq!(resolve(&fake.bot, &q, &config).await?, None);
    Ok(())
}

#[tokio::test]
async fn pagination_is_read_only_and_read_does_not_repeat_an_action() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let session = publish(&fake, &config, Payload::Personal(PersonalPayload::Settings)).await?;
    assert!(session.pages.len() > 1);
    fake.clear();
    assert_eq!(
        resolve(&fake.bot, &query(&session, "p1"), &config).await?,
        None
    );
    assert_eq!(fake.calls()[0].0, "answerWidgetQuery");
    assert_eq!(fake.calls()[1].0, "editMessageWidgetData");
    assert_eq!(config.html_ui.sessions.get(&1).unwrap().index, 1);
    fake.clear();
    assert_eq!(
        resolve(&fake.bot, &query(&session, "read"), &config).await?,
        Some(session.view.payload.for_refresh().to_string())
    );
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(fake.calls()[0].0, "answerWidgetQuery");
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn regrant_and_changed_resource_identity_invalidate_old_cards() -> Result<()> {
    let (config, _) = config().await?;
    let fake = FakeTelegram::new().await?;
    let session = publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
    )
    .await?;
    sqlx::query("UPDATE devices SET entity_id='light.changed' WHERE id=1")
        .execute(&config.db)
        .await?;
    assert_eq!(
        resolve(&fake.bot, &query(&session, "read"), &config).await?,
        None
    );
    let session = publish(&fake, &config, Payload::Home).await?;
    db::delete_user(1, &config.db).await?;
    db::add_user(1, &config.db).await?;
    assert_eq!(
        resolve(&fake.bot, &query(&session, "read"), &config).await?,
        None
    );
    let answer = fake.calls().last().unwrap().1["data"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        serde_json::from_str::<Value>(&answer)?,
        json!({"denied":true})
    );
    Ok(())
}

#[tokio::test]
async fn original_confirmation_routes_are_used_for_widget_device_actions() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let session = publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::RoomDetail { room: 1 }),
    )
    .await?;
    let (key, payload) = session
        .actions
        .iter()
        .find(|(_, payload)| {
            matches!(
                payload,
                Payload::Control(
                    ControlPayload::QuickAction { .. } | ControlPayload::ConfirmAction { .. }
                )
            )
        })
        .context("Missing light action")?;
    fake.clear();
    let data = resolve(&fake.bot, &query(&session, key), &config)
        .await?
        .unwrap();
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(fake.calls()[0].0, "answerWidgetQuery");
    assert_eq!(&Payload::from_string(&data)?, payload);
    assert!(
        ha.calls.lock().unwrap().is_empty(),
        "The adapter cannot execute a physical command"
    );
    // Only the existing routing/confirmation engine may act after resolution.
    let original = crate::bot::confirmations::prepare_legacy_callback(
        Payload::from_string(&data)?,
        1,
        1,
        session.message,
        config.clone(),
    )
    .await?;
    router(original, 1, config.clone()).await?;
    assert!(ha.calls.lock().unwrap().len() <= 1);
    assert!(resolve(&fake.bot, &query(&session, key), &config)
        .await?
        .is_none());
    Ok(())
}

#[tokio::test]
async fn widget_callback_is_acknowledged_once_before_result_delivery() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let session = publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::DeviceControl { room: 1, device: 1 }),
    )
    .await?;
    fake.clear();
    let storage = teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new();
    let dialogue = Dialogue::new(storage, ChatId(1));
    dialogue.update(crate::bot::State::default()).await?;
    handlers::handle_callback(
        fake.bot.clone(),
        query(&session, "read"),
        config.clone(),
        dialogue,
    )
    .await?;
    assert_eq!(fake.calls()[0].0, "answerWidgetQuery");
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if config
                .html_ui
                .sessions
                .get(&1)
                .is_some_and(|current| current.pages[0].seq > session.pages[0].seq)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    let calls = fake.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|(method, _)| method == "answerWidgetQuery")
            .count(),
        1
    );
    assert!(!calls
        .iter()
        .any(|(method, _)| method.eq_ignore_ascii_case("answerCallbackQuery")));
    assert!(calls
        .iter()
        .any(|(method, _)| method == "editMessageWidgetData"));
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn delivery_failure_and_unsupported_views_fall_back_without_changing_preference() -> Result<()>
{
    let (config, _) = config().await?;
    let fake = FakeTelegram::new().await?;
    *fake.fail.lock().unwrap() = Some("sendWidget".into());
    let view = router(Payload::Home, 1, config.clone()).await?;
    assert!(
        handlers::send_new_view_inner(&fake.bot, ChatId(1), 1, view, config.clone(), None).await?
    );
    assert_ne!(
        config.sessions.get(&1).unwrap().ui_message_mode,
        UiMessageMode::Html
    );
    assert_eq!(
        db::interface::get(1, &config.db).await?,
        db::interface::InterfaceMode::Html
    );
    let mut view = router(Payload::Home, 1, config.clone()).await?;
    view.next_state = Some(crate::bot::State::PersonalQuietHours);
    assert!(eligible(&view));
    view.next_state = None;
    view.image = Some(vec![1]);
    assert!(!eligible(&view));
    Ok(())
}

#[tokio::test]
async fn mode_commands_preserve_safe_route_and_never_replay_quick_action() -> Result<()> {
    let (config, ha) = config().await?;
    let fake = FakeTelegram::new().await?;
    let session = publish(
        &fake,
        &config,
        Payload::Control(ControlPayload::DeviceControl { room: 1, device: 1 }),
    )
    .await?;
    config.sessions.get_mut(&1).unwrap().current_context =
        Payload::Control(ControlPayload::QuickAction {
            room: 1,
            device: 1,
            cmd: DeviceCmd::TurnOn,
        })
        .to_string();
    let message: teloxide::types::Message = serde_json::from_value(
        json!({"message_id":99,"date":1,"from":{"id":1,"is_bot":false,"first_name":"U"},"chat":{"id":1,"type":"private"},"text":"/classic"}),
    )?;
    let storage = teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new();
    let dialogue = Dialogue::new(storage, ChatId(1));
    handlers::handle_command(
        fake.bot.clone(),
        message,
        handlers::Command::Classic,
        config.clone(),
        dialogue,
    )
    .await?;
    assert_eq!(
        db::interface::get(1, &config.db).await?,
        db::interface::InterfaceMode::Classic
    );
    assert_ne!(
        config.sessions.get(&1).unwrap().ui_message_mode,
        UiMessageMode::Html
    );
    assert_ne!(
        config.sessions.get(&1).unwrap().last_menu_id,
        session.message
    );
    assert!(ha.calls.lock().unwrap().is_empty());
    Ok(())
}
