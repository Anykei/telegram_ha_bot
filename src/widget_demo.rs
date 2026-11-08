//! Isolated migration playground. Both presentations use synthetic data only.
mod model;
mod preferences;
#[cfg(test)]
mod tests;

use crate::options::AppOptions;
use anyhow::{bail, ensure, Context, Result};
use model::{InterfaceMode, Model, Page, Screen};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const HTML: &str = include_str!("../ui/widgets/dist/panel.html");
const CAPTION: &str = "Демо · виртуальный дом. Home Assistant не подключён. /ui — выбор вида; /classic — обычные кнопки. Работает, пока запущен cargo run.";

struct Telegram {
    client: reqwest::Client,
    token: String,
    endpoint: String,
}
impl Telegram {
    async fn call(&self, method: &str, body: Value) -> Result<Value> {
        let timeout = if method == "getUpdates" { 35 } else { 8 };
        let response = self
            .client
            .post(format!("{}/bot{}/{method}", self.endpoint, self.token))
            .timeout(Duration::from_secs(timeout))
            .json(&body)
            .send()
            .await
            // reqwest errors contain the URL, including the bot token.
            .map_err(|_| anyhow::anyhow!("Telegram {method}: network request failed"))?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("Telegram {method}: invalid JSON ({status})"))?;
        if value["ok"] != true {
            let reason = value["description"]
                .as_str()
                .unwrap_or("request rejected")
                .replace(&self.token, "<redacted>");
            bail!("Telegram {method}: {status} {reason}");
        }
        Ok(value["result"].clone())
    }
}

struct Session {
    user: u64,
    message: i64,
    preference: PathBuf,
    model: Model,
}
fn owns_widget(query: &Value, user: u64, message: i64) -> bool {
    query["from"]["id"].as_u64() == Some(user)
        && query["chat"]["id"].as_u64() == Some(user)
        && query["chat"]["type"] == "private"
        && query["message_id"].as_i64() == Some(message)
        && query["inline_message_id"].is_null()
}
fn owns_message(message: &Value, user: u64) -> bool {
    message["from"]["id"].as_u64() == Some(user)
        && message["chat"]["id"].as_u64() == Some(user)
        && message["chat"]["type"] == "private"
}
fn owns_callback(query: &Value, user: u64, message: i64) -> bool {
    query["from"]["id"].as_u64() == Some(user)
        && query["message"]["chat"]["id"].as_u64() == Some(user)
        && query["message"]["chat"]["type"] == "private"
        && query["message"]["message_id"].as_i64() == Some(message)
        && query["inline_message_id"].is_null()
}
fn classic(page: &Page) -> Value {
    let mut lines = vec![page.eyebrow.to_string(), page.title.to_string()];
    if !page.description.is_empty() {
        lines.push(page.description.to_string());
    }
    if let Some(hero) = &page.hero {
        lines.push(format!("{}: {}", hero.label, hero.value));
    }
    for item in &page.items {
        if !item.detail.is_empty() {
            lines.push(format!("{} · {}", item.label, item.detail));
        }
    }
    lines.push(CAPTION.into());
    let button = |label: String, action: &str| {
        json!({
            "text": label, "callback_data": format!("p1:{}:{action}", page.rev)
        })
    };
    let mut rows: Vec<Vec<Value>> = page
        .items
        .iter()
        .map(|item| {
            vec![button(
                format!("{}{}", if item.selected { "✓ " } else { "" }, item.label),
                item.action,
            )]
        })
        .collect();
    rows.push(vec![button("Обновить".into(), "read")]);
    if !page.nav.is_empty() {
        rows.push(
            page.nav
                .iter()
                .map(|item| button(item.label.into(), item.action))
                .collect(),
        );
    }
    json!({"text": lines.join("\n\n"), "reply_markup": {"inline_keyboard": rows}})
}
async fn send(api: &Telegram, user: u64, model: &Model) -> Result<i64> {
    let page = model.page();
    let (method, mut body) = match model.mode {
        InterfaceMode::Classic => ("sendMessage", classic(&page)),
        InterfaceMode::Html => (
            "sendWidget",
            json!({
                "html": HTML, "width": 360, "allow_queries": true,
                "chat_data": serde_json::to_string(&page)?, "caption": CAPTION,
            }),
        ),
    };
    body["chat_id"] = json!(user);
    api.call(method, body).await?["message_id"]
        .as_i64()
        .context("Telegram returned no message_id")
}
async fn answer_widget(api: &Telegram, query: &Value, data: Value) -> Result<()> {
    api.call(
        "answerWidgetQuery",
        json!({
            "widget_query_id": query["id"], "data": serde_json::to_string(&data)?
        }),
    )
    .await?;
    Ok(())
}
impl Session {
    async fn edit(&self, api: &Telegram) -> Result<()> {
        let (method, mut body) = match self.model.mode {
            InterfaceMode::Classic => ("editMessageText", classic(&self.model.page())),
            InterfaceMode::Html => (
                "editMessageWidgetData",
                json!({
                    "chat_data": serde_json::to_string(&self.model.page())?
                }),
            ),
        };
        body["chat_id"] = json!(self.user);
        body["message_id"] = json!(self.message);
        api.call(method, body).await?;
        Ok(())
    }
    async fn switch(&mut self, api: &Telegram, mode: InterfaceMode) -> Result<()> {
        let mut next = self.model.clone();
        next.mode = mode;
        next.rev += 1;
        // Keep the old controls and preference until the new message is delivered AND saved.
        let message = send(api, self.user, &next).await?;
        if let Err(error) = preferences::save(&self.preference, mode).await {
            let _ = api
                .call(
                    "deleteMessage",
                    json!({"chat_id": self.user, "message_id": message}),
                )
                .await;
            return Err(error);
        }
        let old = self.message;
        self.message = message;
        self.model = next;
        // Reject old message IDs even if Telegram cannot delete them.
        if let Err(error) = api
            .call(
                "deleteMessage",
                json!({"chat_id": self.user, "message_id": old}),
            )
            .await
        {
            log::warn!("Could not remove previous demo view: {error}");
        }
        Ok(())
    }
    async fn apply(&mut self, api: &Telegram, action: &str) -> Result<()> {
        let mode = match action {
            "classic" => Some(InterfaceMode::Classic),
            "html" => Some(InterfaceMode::Html),
            _ => None,
        };
        if let Some(mode) = mode {
            if mode != self.model.mode {
                return self.switch(api, mode).await;
            }
            preferences::save(&self.preference, mode).await?;
        }
        self.model.apply(action)?;
        self.edit(api).await
    }
    async fn widget(&mut self, api: &Telegram, query: &Value) -> Result<()> {
        if self.model.mode != InterfaceMode::Html || !owns_widget(query, self.user, self.message) {
            return answer_widget(api, query, json!({"denied": true})).await;
        }
        let action = match self.model.action(query["data"].as_str().unwrap_or("")) {
            Ok(Some(action)) => action,
            _ => return answer_widget(api, query, json!({"page": self.model.page()})).await,
        };
        if action == "classic" {
            // Answer before extra API calls: a widget has a short response deadline.
            answer_widget(api, query, json!({"transition": true})).await?;
            if let Err(error) = self.apply(api, action).await {
                self.model.rev += 1;
                let _ = self.edit(api).await;
                return Err(error);
            }
            return Ok(());
        }
        if action == "html" {
            preferences::save(&self.preference, InterfaceMode::Html).await?;
        }
        self.model.apply(action)?;
        let answered = answer_widget(api, query, json!({"page": self.model.page()})).await;
        // Persist confirmed state even if answering timed out.
        let edited = self.edit(api).await;
        answered.and(edited)
    }
    async fn callback(&mut self, api: &Telegram, query: &Value) -> Result<()> {
        let valid = self.model.mode == InterfaceMode::Classic
            && owns_callback(query, self.user, self.message);
        let action = valid
            .then(|| self.model.action(query["data"].as_str().unwrap_or("")))
            .and_then(Result::ok)
            .flatten();
        api.call(
            "answerCallbackQuery",
            json!({
                "callback_query_id": query["id"],
                "text": if action.is_some() { "" } else { "Откройте актуальный пульт: /start" }
            }),
        )
        .await?;
        if let Some(action) = action {
            self.apply(api, action).await?;
        }
        Ok(())
    }
    async fn notice(&self, api: &Telegram, text: &str) {
        let _ = api
            .call("sendMessage", json!({"chat_id": self.user, "text": text}))
            .await;
    }
    async fn command(&mut self, api: &Telegram, message: &Value) -> Result<()> {
        if !owns_message(message, self.user) {
            return Ok(());
        }
        let command = message["text"]
            .as_str()
            .unwrap_or("")
            .split_whitespace()
            .next()
            .unwrap_or("")
            .split('@')
            .next()
            .unwrap_or("");
        match command {
            "/classic" => self.switch(api, InterfaceMode::Classic).await,
            "/html" => self.switch(api, InterfaceMode::Html).await,
            "/ui" | "/start" | "/demo" => {
                // Commands recover a deleted card and never replay its action.
                let mut next = self.model.clone();
                next.screen = if command == "/ui" {
                    Screen::Settings
                } else {
                    Screen::Home
                };
                next.rev += 1;
                let message = send(api, self.user, &next).await?;
                let old = self.message;
                self.message = message;
                self.model = next;
                let _ = api
                    .call(
                        "deleteMessage",
                        json!({"chat_id": self.user, "message_id": old}),
                    )
                    .await;
                Ok(())
            }
            _ => {
                self.notice(api, "Демонстрационный пульт: /start — дом, /ui — выбор вида, /classic — кнопки Telegram, /html — HTML. Home Assistant не подключён.").await;
                Ok(())
            }
        }
    }
    async fn update(&mut self, api: &Telegram, update: &Value) -> Result<()> {
        if let Some(query) = update.get("widget_query") {
            return self.widget(api, query).await;
        }
        if let Some(query) = update.get("callback_query") {
            return self.callback(api, query).await;
        }
        if let Some(message) = update.get("message") {
            return self.command(api, message).await;
        }
        Ok(())
    }
}

pub async fn run(options: &AppOptions, options_path: &Path) -> Result<()> {
    println!("HTML migration DEMO: виртуальный дом; Home Assistant и основная БД не подключаются.");
    let api = Telegram {
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        token: options.bot_token.clone(),
        endpoint: "https://api.telegram.org/beta".into(),
    };
    let identity = api.call("getMe", json!({})).await?;
    let webhook = api.call("getWebhookInfo", json!({})).await?;
    ensure!(
        webhook["url"].as_str().unwrap_or("").is_empty(),
        "Demo cannot run alongside a webhook; its configuration was not changed"
    );
    let allowed_before = webhook.get("allowed_updates").cloned().unwrap_or(json!([]));
    let user = options.root_user;
    let preference = preferences::path(
        options_path,
        identity["id"].as_u64().context("Missing bot ID")?,
        user,
    );
    let mode = preferences::load(&preference).await?;
    let mut model = Model {
        mode,
        rev: 1,
        ..Default::default()
    };
    let message = match send(&api, user, &model).await {
        Ok(message) => message,
        Err(error) if mode == InterfaceMode::Html => {
            log::warn!("HTML unavailable, opening classic demo: {error}");
            model.mode = InterfaceMode::Classic;
            // Temporary fallback preserves the user's saved preference.
            send(&api, user, &model).await?
        }
        Err(error) => return Err(error),
    };
    let mut session = Session {
        user,
        message,
        preference,
        model,
    };
    println!("Demo sent: @{}, chat={}, message={}. /ui — выбор вида; /classic — обычный; /html — новый. Keep cargo run running. Ctrl+C stops the demo.", identity["username"].as_str().unwrap_or("?"), user, message);
    let stop = crate::wait_for_shutdown_signal();
    tokio::pin!(stop);
    let mut offset = 0_i64;
    let outcome: Result<()> = loop {
        let result = tokio::select! {
            _ = &mut stop => break Ok(()),
            result = api.call("getUpdates", json!({ "offset": offset, "timeout": 25, "allowed_updates": ["widget_query", "callback_query", "message"] })) => result,
        };
        let updates = match result {
            Ok(updates) => updates,
            Err(error) => {
                break Err(error.context("Demo polling stopped; run only one bot instance"))
            }
        };
        let Some(updates) = updates.as_array() else {
            break Err(anyhow::anyhow!("Invalid getUpdates result"));
        };
        for update in updates {
            if let Some(id) = update["update_id"].as_i64() {
                offset = id + 1;
            }
            if let Err(error) = session.update(&api, update).await {
                log::warn!("Demo request: {error}");
                session.notice(&api, "Не удалось обновить пульт. /start — открыть заново; /classic — обычный вид.").await;
            }
        }
    };
    if let Err(error) = api
        .call(
            "getUpdates",
            json!({ "offset": offset, "timeout": 0, "allowed_updates": allowed_before }),
        )
        .await
    {
        log::warn!("Could not restore Telegram update selection: {error}");
    }
    println!("Демо остановлено. Запустите cargo run, чтобы снова пользоваться пультом.");
    outcome
}
