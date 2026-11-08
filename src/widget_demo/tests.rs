use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

type Calls = Arc<Mutex<Vec<(String, Value)>>>;
struct FakeTelegram {
    api: Telegram,
    calls: Calls,
    fail: Arc<Mutex<Option<String>>>,
    task: tokio::task::JoinHandle<()>,
    directory: PathBuf,
}
impl Drop for FakeTelegram {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
impl FakeTelegram {
    async fn new() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let calls: Calls = Arc::default();
        let recorded = calls.clone();
        let fail: Arc<Mutex<Option<String>>> = Arc::default();
        let failing = fail.clone();
        let task = tokio::spawn(async move {
            let mut id = 100;
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
                let mut body = vec![0; length];
                reader.read_exact(&mut body).await.unwrap();
                recorded
                    .lock()
                    .unwrap()
                    .push((method.clone(), serde_json::from_slice(&body).unwrap()));
                let fail = {
                    let mut next = failing.lock().unwrap();
                    if next.as_deref() == Some(&method) {
                        next.take();
                        true
                    } else {
                        false
                    }
                };
                let result = if method == "sendMessage" || method == "sendWidget" {
                    id += 1;
                    json!({"message_id": id})
                } else {
                    json!(true)
                };
                let response = if fail {
                    json!({"ok": false, "description": "simulated failure"})
                } else {
                    json!({"ok": true, "result": result})
                }
                .to_string();
                reader.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
            }
        });
        let directory = std::env::temp_dir().join(format!(
            "ha-widget-{}-{}",
            std::process::id(),
            address.port()
        ));
        tokio::fs::create_dir_all(&directory).await?;
        Ok(Self {
            api: Telegram {
                client: reqwest::Client::new(),
                token: "test".into(),
                endpoint: format!("http://{address}"),
            },
            calls,
            fail,
            task,
            directory,
        })
    }
    fn session(&self) -> Session {
        Session {
            user: 7,
            message: 12,
            preference: preferences::path(&self.directory.join("options.json"), 99, 7),
            model: Model {
                rev: 1,
                ..Default::default()
            },
        }
    }
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }
    fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
    fn fail_next(&self, method: &str) {
        *self.fail.lock().unwrap() = Some(method.into());
    }
}
fn widget(session: &Session, action: &str) -> Value {
    json!({"id": "query", "from": {"id": session.user}, "chat": {"id": session.user,"type": "private"}, "message_id": session.message, "data": format!("p1:{}:{action}", session.model.rev)})
}
fn callback(session: &Session, action: &str) -> Value {
    json!({"id": "callback", "from": {"id": session.user}, "message": {"chat": {"id": session.user,"type": "private"}, "message_id": session.message}, "data": format!("p1:{}:{action}", session.model.rev)})
}
fn command(text: &str) -> Value {
    json!({"from": {"id": 7}, "chat": {"id": 7,"type": "private"}, "text": text})
}
fn answer(body: &Value) -> Value {
    serde_json::from_str(body["data"].as_str().unwrap()).unwrap()
}

#[test]
fn routes_only_offer_valid_actions_and_stale_requests_never_execute() -> Result<()> {
    let mut model = Model {
        rev: 1,
        ..Default::default()
    };
    assert!(model.action("p1:1:on").is_err());
    for action in [
        "rooms", "room", "device", "on", "settings", "classic", "html", "device",
    ] {
        let input = format!("p1:{}:{action}", model.rev);
        assert_eq!(model.action(&input)?, Some(action));
        let old = input.clone();
        model.apply(action)?;
        assert_eq!(model.action(&old)?, None);
        assert!(model.page().items.len() <= 4);
    }
    assert!(
        model.on,
        "navigation and switching presentation must preserve device state"
    );
    assert_eq!(model.screen, Screen::Device);
    assert!(model.action("p1:9:on:extra").is_err());
    assert!(model.action(&"x".repeat(65)).is_err());
    Ok(())
}

#[test]
fn every_transport_requires_owner_private_chat_and_current_message() {
    let session = Session {
        user: 7,
        message: 12,
        preference: PathBuf::new(),
        model: Model::default(),
    };
    let q = widget(&session, "read");
    assert!(owns_widget(&q, 7, 12));
    for (field, value) in [
        ("from", json!({"id": 8})),
        ("chat", json!({"id": 7, "type": "group"})),
        ("message_id", json!(13)),
        ("inline_message_id", json!("copy")),
    ] {
        let mut invalid = q.clone();
        invalid[field] = value;
        assert!(!owns_widget(&invalid, 7, 12));
    }
    assert!(owns_callback(&callback(&session, "read"), 7, 12));
    assert!(!owns_callback(&callback(&session, "read"), 7, 13));
    assert!(!owns_callback(&callback(&session, "read"), 8, 12));
    let mut message = command("/classic");
    message["chat"]["type"] = json!("group");
    assert!(!owns_message(&message, 7));
}

#[tokio::test]
async fn html_and_classic_share_state_and_persist_mode_across_restarts() -> Result<()> {
    let fake = FakeTelegram::new().await?;
    let mut session = fake.session();
    for action in ["rooms", "room", "device"] {
        session.widget(&fake.api, &widget(&session, action)).await?;
    }
    let original = widget(&session, "on");
    session.widget(&fake.api, &original).await?;
    let rev = session.model.rev;
    session.widget(&fake.api, &original).await?;
    assert_eq!(session.model.rev, rev);
    assert!(session.model.on);
    session.command(&fake.api, &command("/classic")).await?;
    assert!(session.model.on);
    assert_eq!(session.model.screen, Screen::Device);
    assert_eq!(
        preferences::load(&session.preference).await?,
        InterfaceMode::Classic
    );
    session
        .callback(&fake.api, &callback(&session, "off"))
        .await?;
    assert!(!session.model.on);
    session.command(&fake.api, &command("/html")).await?;
    assert_eq!(
        preferences::load(&session.preference).await?,
        InterfaceMode::Html
    );
    assert!(!session.model.on);
    fake.clear();
    session.widget(&fake.api, &original).await?;
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(answer(&fake.calls()[0].1), json!({"denied": true}));
    Ok(())
}

#[tokio::test]
async fn widget_answers_before_edit_and_keeps_acknowledged_state_when_edit_fails() -> Result<()> {
    let fake = FakeTelegram::new().await?;
    let mut session = fake.session();
    session.model.screen = Screen::Device;
    let stale_refresh = widget(&session, "read");
    fake.fail_next("editMessageWidgetData");
    assert!(session
        .widget(&fake.api, &widget(&session, "on"))
        .await
        .is_err());
    let calls = fake.calls();
    assert_eq!(calls[0].0, "answerWidgetQuery");
    assert_eq!(answer(&calls[0].1)["page"]["hero"]["value"], "Включён");
    assert_eq!(calls[1].0, "editMessageWidgetData");
    assert!(session.model.on);
    session.widget(&fake.api, &stale_refresh).await?;
    assert!(session.model.on);
    Ok(())
}

#[tokio::test]
async fn failed_switch_keeps_old_message_state_and_preference() -> Result<()> {
    let fake = FakeTelegram::new().await?;
    let mut session = fake.session();
    session.model.on = true;
    preferences::save(&session.preference, InterfaceMode::Html).await?;
    fake.fail_next("sendMessage");
    assert!(session
        .switch(&fake.api, InterfaceMode::Classic)
        .await
        .is_err());
    assert_eq!(
        (session.message, session.model.mode, session.model.on),
        (12, InterfaceMode::Html, true)
    );
    assert_eq!(
        preferences::load(&session.preference).await?,
        InterfaceMode::Html
    );
    assert!(!fake
        .calls()
        .iter()
        .any(|(method, _)| method == "deleteMessage"));
    fake.clear();
    session.preference = fake.directory.join("missing/interface.json");
    assert!(session
        .switch(&fake.api, InterfaceMode::Classic)
        .await
        .is_err());
    assert_eq!(session.message, 12);
    assert!(fake
        .calls()
        .iter()
        .all(|(method, body)| method != "deleteMessage" || body["message_id"] != 12));
    Ok(())
}

#[tokio::test]
async fn mode_buttons_reply_before_replacement_and_block_old_callbacks() -> Result<()> {
    let fake = FakeTelegram::new().await?;
    let mut session = fake.session();
    session.model.screen = Screen::Settings;
    session
        .widget(&fake.api, &widget(&session, "classic"))
        .await?;
    assert_eq!(fake.calls()[0].0, "answerWidgetQuery");
    assert_eq!(answer(&fake.calls()[0].1), json!({"transition": true}));
    let old = callback(&session, "html");
    session.callback(&fake.api, &old).await?;
    assert_eq!(session.model.mode, InterfaceMode::Html);
    fake.clear();
    session.callback(&fake.api, &old).await?;
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(fake.calls()[0].0, "answerCallbackQuery");
    assert_eq!(session.model.mode, InterfaceMode::Html);
    let mut other = command("/classic");
    other["from"]["id"] = json!(8);
    fake.clear();
    session.command(&fake.api, &other).await?;
    assert!(fake.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_preferences_are_not_silently_overwritten() -> Result<()> {
    let fake = FakeTelegram::new().await?;
    let path = fake.session().preference;
    assert_eq!(preferences::load(&path).await?, InterfaceMode::Html);
    tokio::fs::write(&path, b"{\"version\":99,\"mode\":\"html\"}").await?;
    assert!(preferences::load(&path).await.is_err());
    tokio::fs::write(&path, b"broken").await?;
    assert!(preferences::load(&path).await.is_err());
    Ok(())
}

#[test]
fn demo_is_opt_in_and_embedded_panel_has_no_simulator() -> Result<()> {
    let options: AppOptions = serde_json::from_value(json!({"bot_token": "test", "root_user": 7}))?;
    assert!(!options.widget_demo_enabled);
    assert!(HTML.contains("TelegramWidget"));
    assert!(!HTML.contains("__panelTest"));
    assert!(HTML.len() <= 32768);
    Ok(())
}
