use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use log::warn;
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tungstenite::{Bytes, Utf8Bytes};

const CHUNK_SIZE: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const PIPELINE_LIST_TIMEOUT: Duration = Duration::from_secs(10);
const START_TIMEOUT: Duration = Duration::from_secs(15);
const TRANSIENT_RETRY_DELAY: Duration = Duration::from_millis(1200);

pub async fn transcribe_pcm(
    ha_url: &str,
    ha_token: &str,
    pipeline_id: Option<&str>,
    sample_rate: u32,
    timeout_s: u64,
    pcm: &[u8],
) -> Result<String> {
    let mut last_error = None;
    for attempt in 1..=2 {
        match transcribe_pcm_once(ha_url, ha_token, pipeline_id, sample_rate, timeout_s, pcm).await
        {
            Ok(text) => return Ok(text),
            Err(error) if attempt == 1 && is_retryable_assist_error(&error) => {
                warn!(
                    "HA Assist STT transient error, retrying once in {:?}: {}",
                    TRANSIENT_RETRY_DELAY, error
                );
                last_error = Some(error);
                tokio::time::sleep(TRANSIENT_RETRY_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("HA Assist STT не вернул результат")))
}

async fn transcribe_pcm_once(
    ha_url: &str,
    ha_token: &str,
    pipeline_id: Option<&str>,
    sample_rate: u32,
    timeout_s: u64,
    pcm: &[u8],
) -> Result<String> {
    let ws_url = ha_url
        .replace("http", "ws")
        .trim_end_matches('/')
        .to_string()
        + "/api/websocket";

    let (ws_stream, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(&ws_url))
        .await
        .with_context(|| {
            format!(
                "Timeout подключения к HA Assist WebSocket после {}с: {}",
                CONNECT_TIMEOUT.as_secs(),
                ws_url
            )
        })?
        .with_context(|| format!("Не удалось подключиться к HA Assist WebSocket: {}", ws_url))?;
    let (mut write, mut read) = ws_stream.split();

    authenticate(&mut write, &mut read, ha_token).await?;
    let pipeline_id = select_pipeline(&mut write, &mut read, pipeline_id).await?;
    let handler_id = start_stt_run(
        &mut write,
        &mut read,
        pipeline_id.as_deref(),
        sample_rate,
        timeout_s,
    )
    .await?;

    send_pcm(&mut write, handler_id, pcm).await?;
    wait_for_stt_result(&mut read, timeout_s).await
}

async fn authenticate<W, R>(write: &mut W, read: &mut R, ha_token: &str) -> Result<()>
where
    W: SinkExt<Message> + Unpin,
    W::Error: std::error::Error + Send + Sync + 'static,
    R: StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let msg = next_json_with_timeout(read, AUTH_TIMEOUT, "ожидание auth_required").await?;
    if msg["type"].as_str() != Some("auth_required") {
        return Err(anyhow!("HA WebSocket не запросил авторизацию"));
    }

    write
        .send(Message::Text(Utf8Bytes::from(
            json!({"type": "auth", "access_token": ha_token}).to_string(),
        )))
        .await?;

    let msg = next_json_with_timeout(read, AUTH_TIMEOUT, "ожидание auth_ok").await?;
    match msg["type"].as_str() {
        Some("auth_ok") => Ok(()),
        Some("auth_invalid") => Err(anyhow!("HA WebSocket отклонил токен")),
        other => Err(anyhow!("Неожиданный ответ авторизации HA: {:?}", other)),
    }
}

async fn select_pipeline<W, R>(
    write: &mut W,
    read: &mut R,
    configured_pipeline: Option<&str>,
) -> Result<Option<String>>
where
    W: SinkExt<Message> + Unpin,
    W::Error: std::error::Error + Send + Sync + 'static,
    R: StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    if let Some(pipeline) = configured_pipeline.filter(|value| !value.trim().is_empty()) {
        return Ok(Some(pipeline.to_string()));
    }

    write
        .send(Message::Text(Utf8Bytes::from(
            json!({"id": 1, "type": "assist_pipeline/pipeline/list"}).to_string(),
        )))
        .await?;

    let response = next_json_with_timeout(
        read,
        PIPELINE_LIST_TIMEOUT,
        "получение списка Assist pipeline",
    )
    .await?;
    if response["success"].as_bool() != Some(true) {
        return Err(anyhow!(
            "Home Assistant не вернул список Assist pipeline: {}",
            format_ws_result_error(&response)
        ));
    }

    let result = &response["result"];
    if let Some(id) = result["preferred_pipeline"].as_str() {
        return Ok(Some(id.to_string()));
    }
    if let Some(id) = result["preferred_pipeline_id"].as_str() {
        return Ok(Some(id.to_string()));
    }
    if let Some(id) = result["default_pipeline"].as_str() {
        return Ok(Some(id.to_string()));
    }
    if let Some(id) = result["default_pipeline_id"].as_str() {
        return Ok(Some(id.to_string()));
    }

    let first = result["pipelines"]
        .as_array()
        .and_then(|pipelines| pipelines.first())
        .and_then(|pipeline| pipeline["id"].as_str())
        .map(ToString::to_string);

    first
        .map(Some)
        .ok_or_else(|| anyhow!("В Home Assistant не найден ни один Assist pipeline"))
}

async fn start_stt_run<W, R>(
    write: &mut W,
    read: &mut R,
    pipeline_id: Option<&str>,
    sample_rate: u32,
    timeout_s: u64,
) -> Result<u8>
where
    W: SinkExt<Message> + Unpin,
    W::Error: std::error::Error + Send + Sync + 'static,
    R: StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let mut body = json!({
        "id": 2,
        "type": "assist_pipeline/run",
        "start_stage": "stt",
        "end_stage": "stt",
        "input": {
            "sample_rate": sample_rate
        },
        "timeout": timeout_s
    });
    if let Some(pipeline_id) = pipeline_id {
        body["pipeline"] = json!(pipeline_id);
    }

    write
        .send(Message::Text(Utf8Bytes::from(body.to_string())))
        .await?;

    let mut handler_id = None;
    let mut stt_started = false;
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;

    while tokio::time::Instant::now() < deadline {
        let msg = next_json_until(read, deadline, "запуск HA Assist STT pipeline").await?;
        if msg["type"].as_str() == Some("result") && msg["success"].as_bool() == Some(false) {
            return Err(anyhow!(
                "Не удалось запустить HA Assist STT pipeline: {}",
                format_ws_result_error(&msg)
            ));
        }

        if msg["type"].as_str() == Some("event") {
            let event = &msg["event"];
            match event["type"].as_str() {
                Some("run-start") => {
                    handler_id = event["data"]["runner_data"]["stt_binary_handler_id"]
                        .as_u64()
                        .and_then(|value| u8::try_from(value).ok());
                }
                Some("stt-start") => {
                    stt_started = true;
                    if let Some(handler_id) = handler_id {
                        return Ok(handler_id);
                    }
                }
                Some("error") | Some("stt-failed") => {
                    return Err(anyhow!(
                        "HA Assist STT вернул ошибку: {}",
                        format_pipeline_error(event)
                    ));
                }
                _ => {}
            }
        }

        if stt_started {
            if let Some(handler_id) = handler_id {
                return Ok(handler_id);
            }
        }
    }

    Err(anyhow!(
        "HA Assist STT pipeline не прислал stt_binary_handler_id"
    ))
}

async fn send_pcm<W>(write: &mut W, handler_id: u8, pcm: &[u8]) -> Result<()>
where
    W: SinkExt<Message> + Unpin,
    W::Error: std::error::Error + Send + Sync + 'static,
{
    for chunk in pcm.chunks(CHUNK_SIZE) {
        let mut payload = Vec::with_capacity(chunk.len() + 1);
        payload.push(handler_id);
        payload.extend_from_slice(chunk);
        write.send(Message::Binary(Bytes::from(payload))).await?;
    }

    write
        .send(Message::Binary(Bytes::from(vec![handler_id])))
        .await?;
    Ok(())
}

async fn wait_for_stt_result<R>(read: &mut R, timeout_s: u64) -> Result<String>
where
    R: StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_s);

    while tokio::time::Instant::now() < deadline {
        let msg = next_json_until(read, deadline, "ожидание результата HA Assist STT").await?;
        if msg["type"].as_str() != Some("event") {
            continue;
        }

        let event = &msg["event"];
        match event["type"].as_str() {
            Some("stt-end") => {
                if let Some(text) = find_text(event) {
                    let text = text.trim();
                    if !text.is_empty() {
                        return Ok(text.to_string());
                    }
                }
                return Err(anyhow!("HA Assist STT вернул пустой текст"));
            }
            Some("stt-failed") | Some("error") | Some("run-end") => {
                let detail = format_pipeline_error(event);
                if detail != "без подробностей" {
                    return Err(anyhow!("HA Assist STT ошибка: {}", detail));
                }
                return Err(anyhow!("HA Assist STT не смог распознать речь"));
            }
            _ => {}
        }
    }

    Err(anyhow!("HA Assist STT timeout после {}с", timeout_s))
}

async fn next_json<R>(read: &mut R) -> Result<Value>
where
    R: StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    loop {
        let msg = read
            .next()
            .await
            .context("HA WebSocket закрыл соединение")??;
        if !msg.is_text() {
            continue;
        }

        return serde_json::from_str(msg.to_text()?).context("Не удалось разобрать HA WS JSON");
    }
}

async fn next_json_with_timeout<R>(
    read: &mut R,
    timeout: Duration,
    stage: &'static str,
) -> Result<Value>
where
    R: StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    tokio::time::timeout(timeout, next_json(read))
        .await
        .with_context(|| format!("HA Assist WebSocket timeout: {}", stage))?
}

async fn next_json_until<R>(
    read: &mut R,
    deadline: tokio::time::Instant,
    stage: &'static str,
) -> Result<Value>
where
    R: StreamExt<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let now = tokio::time::Instant::now();
    if now >= deadline {
        return Err(anyhow!("HA Assist WebSocket timeout: {}", stage));
    }

    next_json_with_timeout(read, deadline - now, stage).await
}

fn find_text(value: &Value) -> Option<&str> {
    value["data"]["stt_output"]["text"]
        .as_str()
        .or_else(|| value["data"]["text"].as_str())
        .or_else(|| value["data"]["message"].as_str())
        .or_else(|| value["data"]["error"].as_str())
}

fn format_ws_result_error(value: &Value) -> String {
    let error = &value["error"];
    let code = error["code"].as_str();
    let message = error["message"].as_str();

    match (code, message) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => compact_json(value),
    }
}

fn format_pipeline_error(event: &Value) -> String {
    let code = event["data"]["code"]
        .as_str()
        .or_else(|| event["code"].as_str());
    let message = event["data"]["message"]
        .as_str()
        .or_else(|| event["data"]["error"].as_str())
        .or_else(|| event["message"].as_str())
        .or_else(|| find_text(event));

    match (code, message) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => "без подробностей".to_string(),
    }
}

fn compact_json(value: &Value) -> String {
    let text = value.to_string();
    const MAX_LEN: usize = 400;
    if text.len() <= MAX_LEN {
        text
    } else {
        format!("{}...", &text[..MAX_LEN])
    }
}

fn is_retryable_assist_error(error: &anyhow::Error) -> bool {
    let text = error.to_string().to_lowercase();
    [
        "timeout",
        "timed out",
        "закрыл соединение",
        "connection reset",
        "connection refused",
        "connection closed",
        "connection aborted",
        "unexpected eof",
        "broken pipe",
        "failed to lookup address",
        "dns",
        "temporarily unavailable",
        "не удалось подключиться",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_assist_errors_are_network_only() {
        assert!(is_retryable_assist_error(&anyhow!(
            "HA WebSocket закрыл соединение"
        )));
        assert!(is_retryable_assist_error(&anyhow!(
            "Timeout подключения к HA Assist WebSocket"
        )));
        assert!(!is_retryable_assist_error(&anyhow!(
            "validation-error: the pipeline does not support speech-to-text"
        )));
        assert!(!is_retryable_assist_error(&anyhow!(
            "HA Assist STT вернул пустой текст"
        )));
    }
}
