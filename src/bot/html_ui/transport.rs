//! Bot API widget methods and raw polling. Teloxide 0.17 drops widget_query updates.
use anyhow::{ensure, Context, Result};
use futures_util::{stream, Stream};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::VecDeque, pin::Pin, time::Duration};
use teloxide::{
    prelude::*,
    stop::{mk_stop_token, StopToken},
    types::{Update, UpdateId, UpdateKind},
    update_listeners::{StatefulListener, UpdateListener},
    ApiError, RequestError,
};

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;

pub async fn call(bot: &Bot, method: &str, body: Value) -> Result<Value> {
    let url = bot.api_url();
    let base = url.as_str().trim_end_matches('/');
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("HTTP client")
    });
    let response = client
        .post(format!("{base}/bot{}/{method}", bot.token()))
        .timeout(Duration::from_secs(if method == "getUpdates" {
            35
        } else {
            8
        }))
        .json(&body)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Telegram {method}: network error"))?;
    let status = response.status();
    let result: Value = response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("Telegram {method}: invalid response"))?;
    if result["ok"] != true {
        if let Some(seconds) = result["parameters"]["retry_after"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
        {
            return Err(
                RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(seconds)).into(),
            );
        }
        let reason = result["description"]
            .as_str()
            .unwrap_or("request rejected")
            .replace(bot.token(), "<redacted>");
        let code = result["error_code"]
            .as_u64()
            .unwrap_or(u64::from(status.as_u16()));
        if method == "getUpdates" && rejects_widget_updates(code, &reason) {
            return Err(WidgetUpdatesUnsupported.into());
        }
        if method == "editMessageWidgetData" && code == 400 {
            if let Some(error) = widget_edit_error(&reason) {
                return Err(RequestError::Api(error).into());
            }
        }
        anyhow::bail!("Telegram {method}: {reason}");
    }
    Ok(result["result"].clone())
}

fn widget_edit_error(reason: &str) -> Option<ApiError> {
    let reason = reason.to_ascii_lowercase();
    let reason = reason.strip_prefix("bad request: ").unwrap_or(&reason);
    match reason {
        "message to edit not found" | "message_to_edit_not_found" => {
            Some(ApiError::MessageToEditNotFound)
        }
        "message_id_invalid" => Some(ApiError::MessageIdInvalid),
        "message is not modified" | "message_not_modified" => Some(ApiError::MessageNotModified),
        reason if reason.starts_with("message is not modified:") => {
            Some(ApiError::MessageNotModified)
        }
        _ => None,
    }
}

#[derive(Debug)]
struct WidgetUpdatesUnsupported;

impl std::fmt::Display for WidgetUpdatesUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Telegram rejected widget query updates")
    }
}

impl std::error::Error for WidgetUpdatesUnsupported {}

fn rejects_widget_updates(code: u64, reason: &str) -> bool {
    if code != 400 {
        return false;
    }
    let reason = reason.to_ascii_lowercase();
    (reason.contains("widget_query")
        && [
            "unsupported",
            "not supported",
            "unknown",
            "invalid",
            "not allowed",
        ]
        .iter()
        .any(|word| reason.contains(word)))
        || [
            "unsupported update type",
            "unknown update type",
            "invalid update type",
        ]
        .iter()
        .any(|phrase| reason.contains(phrase))
}

pub fn normalize(raw: &Value) -> Result<Update> {
    let Some(q) = raw.get("widget_query") else {
        // UpdateKind probes a borrowed &str key first. An owned Value consumes
        // that key on the failed probe, silently producing Error({}) even for
        // ordinary messages/callbacks. Borrow the JSON so the probe succeeds.
        let update = Update::deserialize(raw).context("Invalid Telegram update")?;
        ensure!(
            !matches!(update.kind, UpdateKind::Error(_)),
            "Unsupported or malformed Telegram update"
        );
        return Ok(update);
    };
    let id = q["id"].as_str().context("Missing query ID")?;
    ensure!(
        q["inline_message_id"].is_null() && q["message_id"].as_i64().is_some_and(|id| id > 0),
        "Unsupported inline/copy widget query"
    );
    ensure!(
        q.get("type").is_none_or(|kind| kind["type"] == "data"),
        "Unsupported widget query type"
    );
    let callback=serde_json::from_value(json!({
        "id":format!("html:{id}"),"from":q["from"],"chat_instance":"html-transport",
        "data":q["data"],"message":{"message_id":q["message_id"],"date":1,"chat":q["chat"],"text":"HTML"}
    })).context("Invalid widget query")?;
    Ok(Update {
        id: UpdateId(
            raw["update_id"]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .context("Missing update ID")?,
        ),
        kind: UpdateKind::CallbackQuery(callback),
    })
}

type Updates = Pin<Box<dyn Stream<Item = Result<Update>> + Send>>;
struct ListenerState {
    updates: Updates,
    stop: StopToken,
}
fn as_stream(state: &mut ListenerState) -> &mut Updates {
    &mut state.updates
}
pub fn listener(
    bot: Bot,
    on_widget_updates_rejected: impl FnOnce() + Send + 'static,
) -> impl UpdateListener<Err = anyhow::Error> {
    let (stop, flag) = mk_stop_token();
    let updates = stream::unfold(
        (
            bot,
            0_i64,
            VecDeque::<Value>::new(),
            flag,
            Some(on_widget_updates_rejected),
        ),
        |(bot, mut offset, mut pending, mut stop_flag, mut on_rejection)| async move {
            loop {
                if stop_flag.is_stopped() {
                    return None;
                }
                if let Some(raw) = pending.pop_front() {
                    if let Some(id) = raw["update_id"]
                        .as_u64()
                        .and_then(|id| u32::try_from(id).ok())
                    {
                        offset = i64::from(id) + 1;
                    }
                    match normalize(&raw) {
                        Ok(update) => {
                            return Some((
                                Ok(update),
                                (bot, offset, pending, stop_flag, on_rejection),
                            ))
                        }
                        Err(_) => {
                            // Log envelope fields only: message bodies, callback data,
                            // and user details must not end up in diagnostics.
                            let fields = raw
                                .as_object()
                                .map(|object| object.keys().map(String::as_str).collect::<Vec<_>>())
                                .unwrap_or_default();
                            log::warn!(
                                "Skipping unsupported or malformed Telegram update: id={:?}, fields={:?}",
                                raw["update_id"].as_u64(),
                                fields
                            );
                            if let Some(id) = raw["widget_query"]["id"].as_str() {
                                let _ = call(
                                    &bot,
                                    "answerWidgetQuery",
                                    json!({"widget_query_id":id,"data":"{\"denied\":true}"}),
                                )
                                .await;
                            }
                            continue;
                        }
                    }
                }
                let allowed_updates: &[&str] = if on_rejection.is_some() {
                    &["message", "callback_query", "widget_query"]
                } else {
                    &["message", "callback_query"]
                };
                let result = tokio::select! {
                    _=&mut stop_flag=>return None,
                    result=call(&bot,"getUpdates",json!({"offset":offset,"timeout":25,"allowed_updates":allowed_updates}))=>result,
                };
                match result {
                    Ok(updates) => {
                        if let Some(values) = updates.as_array() {
                            pending.extend(values.iter().cloned());
                        } else {
                            return Some((
                                Err(anyhow::anyhow!("Invalid Telegram updates")),
                                (bot, offset, pending, stop_flag, on_rejection),
                            ));
                        }
                    }
                    Err(error) => {
                        if let Some(on_rejection) =
                            on_rejection.take_if(|_| error.is::<WidgetUpdatesUnsupported>())
                        {
                            log::warn!("{error}; continuing polling with Telegram buttons");
                            on_rejection();
                            // The failed request did not yield updates. Keep the last offset.
                            continue;
                        }
                        tokio::select! {_=&mut stop_flag=>return None,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
                        return Some((Err(error), (bot, offset, pending, stop_flag, on_rejection)));
                    }
                }
            }
        },
    );
    // The raw list above must survive Dispatcher hints, which know no widget_query.
    StatefulListener::new(
        ListenerState {
            updates: Box::pin(updates),
            stop,
        },
        as_stream,
        |state: &mut ListenerState| state.stop.clone(),
    )
}
