//! Isolated HTML-widget pilot. All state is synthetic; this module has no HA/DB access.
use crate::options::AppOptions;
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

const HTML: &str = include_str!("../ui/widgets/dist/switch.html");
const API: &str = "https://api.telegram.org/beta";

struct Telegram {
    client: reqwest::Client,
    token: String,
}

impl Telegram {
    async fn call(&self, method: &str, body: Value) -> Result<Value> {
        let response = self
            .client
            .post(format!("{API}/bot{}/{method}", self.token))
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

#[derive(Clone, Debug, Serialize)]
struct DemoState {
    v: u8,
    id: &'static str,
    rev: u64,
    name: &'static str,
    room: &'static str,
    state: &'static str,
    access: &'static str,
    critical: bool,
}

impl Default for DemoState {
    fn default() -> Self {
        Self {
            v: 1,
            id: "42",
            rev: 1,
            name: "Торшер · демо",
            room: "Гостиная",
            state: "off",
            access: "control",
            critical: false,
        }
    }
}

impl DemoState {
    /// Explicit desired states + a revision make double taps and stale requests harmless.
    fn apply(&mut self, request: &str) -> Result<()> {
        ensure!(request.len() <= 64, "Widget request is too long");
        let parts: Vec<_> = request.split(':').collect();
        ensure!(
            parts.len() == 4 && parts[0] == "w1" && parts[1] == self.id,
            "Invalid demo target"
        );
        let revision = parts[2].parse::<u64>().context("Invalid revision")?;
        ensure!(
            matches!(parts[3], "on" | "off" | "read"),
            "Unsupported action"
        );
        if parts[3] == "read" || revision != self.rev {
            return Ok(());
        }
        self.state = if parts[3] == "on" { "on" } else { "off" };
        self.rev += 1;
        Ok(())
    }
}

fn owns_query(query: &Value, user: u64, message: i64) -> bool {
    query["from"]["id"].as_u64() == Some(user)
        && query["chat"]["id"].as_u64() == Some(user)
        && query["chat"]["type"] == "private"
        && query["message_id"].as_i64() == Some(message)
        && query["inline_message_id"].is_null()
}

async fn answer(api: &Telegram, query: &Value, state: &DemoState) -> Result<()> {
    api.call(
        "answerWidgetQuery",
        json!({ "widget_query_id": query["id"], "data": serde_json::to_string(&json!({"state": state}))? }),
    )
    .await?;
    Ok(())
}

async fn handle(
    api: &Telegram,
    query: &Value,
    user: u64,
    message: i64,
    state: &mut DemoState,
) -> Result<()> {
    if !owns_query(query, user, message) {
        let denied = DemoState {
            access: "denied",
            name: "Устройство",
            room: "",
            ..state.clone()
        };
        return answer(api, query, &denied).await;
    }
    let request = query["data"].as_str().context("Missing widget data")?;
    let previous = state.rev;
    if state.apply(request).is_err() {
        // Malformed/stale inputs never become device operations.
        return answer(api, query, state).await;
    }
    // Answer even if the state edit fails: an accepted command must not time out silently.
    if state.rev != previous {
        if let Err(error) = api
            .call(
                "editMessageWidgetData",
                json!({
                    "chat_id": user, "message_id": message,
                    "chat_data": serde_json::to_string(state)?
                }),
            )
            .await
        {
            log::warn!("Widget demo state edit: {error}");
        }
    }
    answer(api, query, state).await?;
    log::info!(
        "Widget demo: action={}, state={}, revision={}",
        request.rsplit(':').next().unwrap_or("?"),
        state.state,
        state.rev
    );
    Ok(())
}

pub async fn run(options: &AppOptions) -> Result<()> {
    println!(
        "HTML widget DEMO: виртуальное устройство; Home Assistant и основная БД не подключаются."
    );
    let api = Telegram {
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(40))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        token: options.bot_token.clone(),
    };
    let identity = api.call("getMe", json!({})).await?;
    let webhook = api.call("getWebhookInfo", json!({})).await?;
    ensure!(
        webhook["url"].as_str().unwrap_or("").is_empty(),
        "Demo cannot run alongside a webhook; its configuration was not changed"
    );
    let allowed_before = webhook.get("allowed_updates").cloned().unwrap_or(json!([]));
    let user = options.root_user;
    let mut state = DemoState::default();
    let sent = api.call("sendWidget", json!({
        "chat_id": user,
        "html": HTML,
        "width": 360,
        "allow_queries": true,
        "chat_data": serde_json::to_string(&state)?,
        "caption": "Демонстрационный пульт. Торшер виртуальный; Home Assistant не подключён. Работает, пока запущен cargo run."
    })).await.context("Cannot send HTML widget through Telegram beta API")?;
    let message = sent["message_id"]
        .as_i64()
        .context("sendWidget returned no message_id")?;
    println!("HTML demo sent: @{}, chat={}, message={}. Keep cargo run running to use the switch. Ctrl+C stops the demo.", identity["username"].as_str().unwrap_or("?"), user, message);
    let stop = crate::wait_for_shutdown_signal();
    tokio::pin!(stop);
    let mut offset = 0_i64;
    let outcome: Result<()> = loop {
        let result = tokio::select! {
            _ = &mut stop => break Ok(()),
            result = api.call("getUpdates", json!({ "offset": offset, "timeout": 25, "allowed_updates": ["widget_query"] })) => result,
        };
        let updates = match result {
            Ok(updates) => updates,
            Err(error) => {
                break Err(error.context("Demo polling stopped; run only one bot instance"))
            }
        };
        for update in updates.as_array().context("Invalid getUpdates result")? {
            offset = update["update_id"].as_i64().context("Missing update_id")? + 1;
            if let Some(query) = update.get("widget_query") {
                if let Err(error) = handle(&api, query, user, message, &mut state).await {
                    log::warn!("Widget demo request: {error}");
                }
            }
        }
    };
    // Restore update selection so the regular Teloxide bot can be started afterwards.
    if let Err(error) = api
        .call(
            "getUpdates",
            json!({ "offset": offset, "timeout": 0, "allowed_updates": allowed_before }),
        )
        .await
    {
        log::warn!("Could not restore Telegram update selection: {error}");
    }
    log::info!("HTML widget demo stopped.");
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_and_old_commands_do_not_repeat_or_reverse_state() -> Result<()> {
        let mut state = DemoState::default();
        state.apply("w1:42:1:on")?;
        state.apply("w1:42:1:on")?;
        state.apply("w1:42:1:off")?;
        assert_eq!((state.state, state.rev), ("on", 2));
        state.apply("w1:42:2:off")?;
        assert_eq!((state.state, state.rev), ("off", 3));
        Ok(())
    }

    #[test]
    fn reads_and_invalid_targets_do_not_mutate() -> Result<()> {
        let mut state = DemoState::default();
        state.apply("w1:42:1:read")?;
        for value in [
            "w1:43:1:on",
            "w1:42:1:toggle",
            "w1:42:no:on",
            "w1:42:1:on:extra",
        ] {
            assert!(state.apply(value).is_err());
        }
        assert_eq!((state.state, state.rev), ("off", 1));
        Ok(())
    }

    #[test]
    fn only_owner_in_the_original_private_message_is_accepted() {
        let original =
            json!({ "from": {"id": 7}, "chat": {"id": 7, "type": "private"}, "message_id": 12 });
        assert!(owns_query(&original, 7, 12));
        assert!(!owns_query(&original, 8, 12));
        assert!(!owns_query(&original, 7, 13));
        assert!(!owns_query(
            &json!({"from": {"id": 7}, "widget_id": 1}),
            7,
            12
        ));
        let mut group = original;
        group["chat"]["type"] = json!("group");
        assert!(!owns_query(&group, 7, 12));
    }

    #[test]
    fn demo_is_opt_in_and_payload_is_embedded() -> Result<()> {
        let options: AppOptions =
            serde_json::from_value(json!({"bot_token": "test", "root_user": 7}))?;
        assert!(!options.widget_demo_enabled);
        assert!(HTML.contains("TelegramWidget"));
        assert!(!HTML.contains("mock-telegram"));
        assert!(HTML.len() <= 32768);
        Ok(())
    }
}
