//! Beta methods and raw polling. Teloxide 0.17 drops unknown widget_query fields.
use anyhow::{ensure, Context, Result};
use futures_util::{stream, Stream};
use serde_json::{json, Value};
use std::{collections::VecDeque, pin::Pin, time::Duration};
use teloxide::{
    prelude::*,
    stop::{mk_stop_token, StopToken},
    types::{Update, UpdateId, UpdateKind},
    update_listeners::{StatefulListener, UpdateListener},
};

pub async fn call(bot: &Bot, method: &str, body: Value) -> Result<Value> {
    let url = bot.api_url();
    let base = if url.host_str() == Some("api.telegram.org") {
        "https://api.telegram.org/beta".to_owned()
    } else {
        url.as_str().trim_end_matches('/').to_owned()
    };
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
    let result: Value = response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("Telegram {method}: invalid response"))?;
    if result["ok"] != true {
        if let Some(seconds) = result["parameters"]["retry_after"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
        {
            return Err(teloxide::RequestError::RetryAfter(
                teloxide::types::Seconds::from_seconds(seconds),
            )
            .into());
        }
        let reason = result["description"]
            .as_str()
            .unwrap_or("request rejected")
            .replace(bot.token(), "<redacted>");
        anyhow::bail!("Telegram {method}: {reason}");
    }
    Ok(result["result"].clone())
}

pub fn normalize(raw: &Value) -> Result<Update> {
    let Some(q) = raw.get("widget_query") else {
        return serde_json::from_value(raw.clone()).context("Invalid Telegram update");
    };
    let id = q["id"].as_str().context("Missing query ID")?;
    ensure!(
        q["inline_message_id"].is_null() && q["message_id"].as_i64().is_some_and(|id| id > 0),
        "Unsupported inline/copy widget query"
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
pub fn listener(bot: Bot) -> impl UpdateListener<Err = anyhow::Error> {
    let (stop, flag) = mk_stop_token();
    let updates = stream::unfold(
        (bot, 0_i64, VecDeque::<Value>::new(), flag),
        move |(bot, mut offset, mut pending, mut stop_flag)| async move {
            loop {
                if stop_flag.is_stopped() {
                    return None;
                }
                if let Some(raw) = pending.pop_front() {
                    if let Some(id) = raw["update_id"].as_i64() {
                        offset = id + 1;
                    }
                    match normalize(&raw) {
                        Ok(update) => return Some((Ok(update), (bot, offset, pending, stop_flag))),
                        Err(_) => {
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
                let result = tokio::select! {
                    _=&mut stop_flag=>return None,
                    result=call(&bot,"getUpdates",json!({"offset":offset,"timeout":25,"allowed_updates":["message","callback_query","widget_query"]}))=>result,
                };
                match result {
                    Ok(updates) => {
                        if let Some(values) = updates.as_array() {
                            pending.extend(values.iter().cloned());
                        } else {
                            return Some((
                                Err(anyhow::anyhow!("Invalid Telegram updates")),
                                (bot, offset, pending, stop_flag),
                            ));
                        }
                    }
                    Err(error) => {
                        tokio::select! {_=&mut stop_flag=>return None,_=tokio::time::sleep(Duration::from_secs(2))=>{}}
                        return Some((Err(error), (bot, offset, pending, stop_flag)));
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
