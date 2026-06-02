use anyhow::{anyhow, Context, Result};
use reqwest::{header, Client};
use serde_json::{json, Value};
use std::time::Duration;

pub async fn process(
    ha_url: &str,
    ha_token: &str,
    text: &str,
    language: &str,
    timeout_s: u64,
) -> Result<String> {
    let url = format!("{}/api/conversation/process", ha_url.trim_end_matches('/'));
    let client = Client::builder()
        .timeout(Duration::from_secs(timeout_s.max(5)))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("Не удалось создать HA Conversation HTTP client")?;

    let response = client
        .post(&url)
        .bearer_auth(ha_token)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&json!({
            "text": text,
            "language": language,
        }))
        .send()
        .await
        .with_context(|| format!("Не удалось отправить команду в HA Conversation: {}", url))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("Не удалось прочитать HA Conversation response")?;

    if !status.is_success() {
        return Err(anyhow!("HA Conversation вернул {}: {}", status, body));
    }

    let value: Value =
        serde_json::from_str(&body).context("Не удалось разобрать HA Conversation JSON")?;
    ensure_not_error_response(&value)?;

    extract_speech(&value).ok_or_else(|| anyhow!("HA Conversation не вернул текст ответа"))
}

fn ensure_not_error_response(value: &Value) -> Result<()> {
    let response_type = value["response"]["response_type"]
        .as_str()
        .or_else(|| value["result"]["response"]["response_type"].as_str());

    if response_type != Some("error") {
        return Ok(());
    }

    let code = value["response"]["data"]["code"]
        .as_str()
        .or_else(|| value["result"]["response"]["data"]["code"].as_str())
        .unwrap_or("unknown");
    let speech = extract_speech(value).unwrap_or_else(|| "без текста ошибки".to_string());

    Err(anyhow!("HA Conversation error {}: {}", code, speech))
}

fn extract_speech(value: &Value) -> Option<String> {
    [
        &value["response"]["speech"]["plain"]["speech"],
        &value["speech"]["plain"]["speech"],
        &value["result"]["response"]["speech"]["plain"]["speech"],
        &value["result"]["speech"]["plain"]["speech"],
    ]
    .into_iter()
    .find_map(|value| value.as_str())
    .map(str::trim)
    .filter(|text| !text.is_empty())
    .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_conversation_speech() {
        let value = json!({
            "response": {
                "speech": {
                    "plain": {
                        "speech": "Готово"
                    }
                }
            }
        });

        assert_eq!(extract_speech(&value).as_deref(), Some("Готово"));
    }

    #[test]
    fn conversation_error_response_is_error() {
        let value = json!({
            "response": {
                "response_type": "error",
                "data": {
                    "code": "no_valid_targets"
                },
                "speech": {
                    "plain": {
                        "speech": "Нет зоны всех комнатах"
                    }
                }
            }
        });

        let error = ensure_not_error_response(&value).expect_err("HA error response should fail");
        assert!(error.to_string().contains("no_valid_targets"));
        assert!(error.to_string().contains("Нет зоны всех комнатах"));
    }
}
