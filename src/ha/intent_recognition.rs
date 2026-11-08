//! Recognition and reads only. This client deliberately has no execution API.
use anyhow::{anyhow, ensure, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Clone, Deserialize)]
pub struct Slot {
    pub value: Value,
}

#[derive(Clone, Deserialize)]
pub struct Target {
    // Recognition-time predicate, never an authorization decision or live state.
    #[allow(dead_code)] // Required and type-checked by serde; deliberately not used for reads.
    pub matched: bool,
}

#[derive(Clone, Deserialize)]
pub struct Intent {
    pub name: String,
}

#[derive(Clone, Deserialize)]
pub struct Recognition {
    #[serde(rename = "match")]
    pub matched: bool,
    pub intent: Intent,
    pub details: BTreeMap<String, Slot>,
    pub targets: BTreeMap<String, Target>,
    pub source: Option<String>,
    #[serde(skip)]
    pub floor_targets_supported: bool,
}

#[derive(Clone, Deserialize)]
pub struct ReadState {
    pub entity_id: String,
    pub state: String,
    pub attributes: BTreeMap<String, Value>,
}

#[async_trait::async_trait]
pub trait ReadonlyHa: Send + Sync {
    async fn recognize(&self, text: &str, language: &str) -> Result<Recognition>;
    async fn read_state(&self, entity_id: &str) -> Result<Option<ReadState>>;
    async fn temperature_unit(&self) -> Result<Option<String>>;
    async fn weather_entities(&self) -> Result<Vec<String>>;
}

pub struct RecognitionClient<'a> {
    url: &'a str,
    token: &'a str,
    timeout: Duration,
    http: reqwest::Client,
}

impl<'a> RecognitionClient<'a> {
    pub fn new(url: &'a str, token: &'a str, timeout_s: u64) -> Result<Self> {
        let timeout = Duration::from_secs(timeout_s.max(1));
        Ok(Self {
            url,
            token,
            timeout,
            http: reqwest::Client::builder()
                .timeout(timeout)
                .connect_timeout(Duration::from_secs(10).min(timeout))
                .build()?,
        })
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response> {
        self.http
            .get(format!("{}{}", self.url.trim_end_matches('/'), path))
            .bearer_auth(self.token)
            .send()
            .await
            .map_err(|_| anyhow!("HA read request failed"))
    }
}

fn websocket_url(base: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(base).context("Invalid HA URL")?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => anyhow::bail!("Unsupported HA URL scheme"),
    };
    url.set_scheme(scheme)
        .map_err(|_| anyhow!("Invalid HA URL scheme"))?;
    url.set_path(&format!(
        "{}/api/websocket",
        url.path().trim_end_matches('/')
    ));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

pub fn valid_entity_id(id: &str) -> bool {
    id.split_once('.').is_some_and(|(domain, name)| {
        !domain.is_empty()
            && !name.is_empty()
            && [domain, name].iter().all(|s| {
                s.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            })
    })
}

pub fn parse_result(value: Value) -> Result<Recognition> {
    ensure!(
        value["type"] == "result" && value["id"] == 1 && value["success"] == true,
        "HA recognition unavailable or incompatible"
    );
    let results = value["result"]["results"]
        .as_array()
        .context("Incompatible HA recognition response")?;
    ensure!(results.len() == 1, "Expected one recognition result");
    let recognition: Recognition = serde_json::from_value(results[0].clone())
        .map_err(|_| anyhow!("Unrecognized question or incompatible HA response"))?;
    ensure!(
        recognition.matched && recognition.source.as_deref() != Some("trigger"),
        "Question not recognized"
    );
    ensure!(
        recognition.targets.keys().all(|id| valid_entity_id(id)),
        "Invalid recognition targets"
    );
    Ok(recognition)
}

#[async_trait::async_trait]
impl ReadonlyHa for RecognitionClient<'_> {
    async fn recognize(&self, text: &str, language: &str) -> Result<Recognition> {
        tokio::time::timeout(self.timeout, async {
            let (mut socket, _) = tokio::time::timeout(
                Duration::from_secs(10),
                connect_async(websocket_url(self.url)?),
            )
            .await
            .context("HA recognition connection timeout")?
            .map_err(|_| anyhow!("HA recognition connection failed"))?;
            async fn receive<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Result<Value>
            where
                S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
            {
                loop {
                    match socket.next().await {
                        Some(Ok(Message::Text(text))) => {
                            return serde_json::from_str(&text)
                                .map_err(|_| anyhow!("Invalid HA message"))
                        }
                        Some(Ok(Message::Ping(bytes))) => socket.send(Message::Pong(bytes)).await?,
                        Some(Ok(Message::Pong(_))) => {}
                        _ => anyhow::bail!("HA recognition connection closed"),
                    }
                }
            }
            let version = tokio::time::timeout(Duration::from_secs(10), async {
                ensure!(
                    receive(&mut socket).await?["type"] == "auth_required",
                    "Invalid HA authentication response"
                );
                socket
                    .send(Message::Text(
                        json!({"type":"auth", "access_token":self.token})
                            .to_string()
                            .into(),
                    ))
                    .await?;
                let authenticated = receive(&mut socket).await?;
                ensure!(
                    authenticated["type"] == "auth_ok",
                    "HA authentication failed"
                );
                Ok::<_, anyhow::Error>(
                    authenticated["ha_version"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )
            })
            .await
            .context("HA authentication timeout")??;
            socket
                .send(Message::Text(
                    json!({"id":1,"type":"conversation/agent/homeassistant/debug",
                "sentences":[text],"language":language})
                    .to_string()
                    .into(),
                ))
                .await?;
            let mut result = parse_result(receive(&mut socket).await?)?;
            // 2026.6 debug ignores the floor slot; 2026.9 includes it. Fail
            // closed for unverified versions rather than answer for every floor.
            result.floor_targets_supported = version.starts_with("2026.9.");
            Ok(result)
        })
        .await
        .context("HA recognition deadline exceeded")?
    }

    async fn read_state(&self, entity_id: &str) -> Result<Option<ReadState>> {
        ensure!(valid_entity_id(entity_id), "Invalid entity ID");
        let response = self.get(&format!("/api/states/{entity_id}")).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        ensure!(response.status().is_success(), "HA state read failed");
        let state: ReadState = response
            .json()
            .await
            .map_err(|_| anyhow!("Invalid HA state"))?;
        ensure!(
            state.entity_id == entity_id,
            "HA returned a different entity"
        );
        Ok(Some(state))
    }

    async fn temperature_unit(&self) -> Result<Option<String>> {
        let response = self.get("/api/config").await?;
        ensure!(
            response.status().is_success(),
            "HA configuration read failed"
        );
        let config: Value = response
            .json()
            .await
            .map_err(|_| anyhow!("Invalid HA configuration"))?;
        Ok(config["unit_system"]["temperature"]
            .as_str()
            .map(str::to_owned))
    }

    async fn weather_entities(&self) -> Result<Vec<String>> {
        // Debug recognition omits targets for a weather question without slots.
        // Read only IDs; the caller must authorize each ID before reading state.
        // This constant contains no user text, slot values or executable intent.
        let response = self.http.post(format!("{}/api/template", self.url.trim_end_matches('/')))
            .bearer_auth(self.token)
            .json(&json!({"template":"{{ states.weather | map(attribute='entity_id') | list | to_json }}"}))
            .send().await.map_err(|_| anyhow!("HA weather catalog read failed"))?;
        ensure!(
            response.status().is_success(),
            "HA weather catalog unavailable"
        );
        let ids: Vec<String> = response
            .json()
            .await
            .map_err(|_| anyhow!("Invalid weather catalog"))?;
        ensure!(
            ids.iter()
                .all(|id| valid_entity_id(id) && id.starts_with("weather.")),
            "Invalid weather target"
        );
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires configured Home Assistant; sends diagnostic recognition and GET requests only"]
    async fn live_readonly_recognition_contract() -> Result<()> {
        let paths = crate::config::EnvPaths::load();
        let client = RecognitionClient::new(&paths.ha_url, &paths.ha_token, 15)?;
        let config: Value = client.get("/api/config").await?.json().await?;
        println!(
            "HA version: {}",
            config["version"].as_str().unwrap_or("unknown")
        );
        let mut successes = 0;
        for (language, text) in [
            ("ru", "какая температура"),
            ("en", "what is the temperature"),
            ("ru", "сколько ламп включено"),
            ("en", "how many lights are on"),
            ("ru", "какие окна открыты"),
            ("en", "which windows are open"),
            ("ru", "все лампы выключены"),
            ("en", "are all lights off"),
            ("ru", "какая погода"),
            ("en", "what is the weather"),
        ] {
            match client.recognize(text, language).await {
                Ok(result) => {
                    successes += 1;
                    println!(
                        "{language}: {text}: intent={}, slots={:?}, targets={}",
                        result.intent.name,
                        result.details.keys().collect::<Vec<_>>(),
                        result.targets.len()
                    );
                }
                Err(error) => println!("{language}: {text}: {error}"),
            }
        }
        ensure!(
            successes > 0,
            "No live query recognized; compatibility unconfirmed"
        );
        let weather = client.weather_entities().await?;
        println!("Default weather catalog: {} targets", weather.len());
        if let Some(id) = weather.first() {
            ensure!(
                client.read_state(id).await?.is_some(),
                "Weather entity disappeared"
            );
            println!("Default weather state read: ok (value omitted)");
        }
        // Read catalog metadata only to build installation-specific safe queries.
        // No entity names, IDs or state values are printed by this probe.
        let catalog_response = client.http.post(format!("{}/api/template", client.url.trim_end_matches('/')))
            .bearer_auth(client.token)
            .json(&json!({"template": "[{% for s in states.sensor if s.attributes.device_class | default('') == 'temperature' %}{% if not loop.first %},{% endif %}{\"id\":{{ s.entity_id | to_json }},\"name\":{{ s.name | to_json }},\"area\":{{ area_name(s.entity_id) | to_json }}}{% endfor %}]"}))
            .send().await?;
        ensure!(
            catalog_response.status().is_success(),
            "Temperature metadata unavailable"
        );
        let catalog: Vec<Value> = catalog_response.json().await?;
        let exposed_ids = tokio::time::timeout(Duration::from_secs(15), async {
            let (mut ws, _) = connect_async(websocket_url(client.url)?).await?;
            let greeting: Value = serde_json::from_str(
                ws.next()
                    .await
                    .context("Missing auth greeting")??
                    .to_text()?,
            )?;
            ensure!(
                greeting["type"] == "auth_required",
                "Expected authentication"
            );
            ws.send(Message::Text(
                json!({"type":"auth","access_token":client.token})
                    .to_string()
                    .into(),
            ))
            .await?;
            let auth: Value =
                serde_json::from_str(ws.next().await.context("Missing auth result")??.to_text()?)?;
            ensure!(auth["type"] == "auth_ok", "Authentication failed");
            ws.send(Message::Text(
                json!({"id":1,"type":"homeassistant/expose_entity/list"})
                    .to_string()
                    .into(),
            ))
            .await?;
            let response: Value = serde_json::from_str(
                ws.next()
                    .await
                    .context("Missing exposure list")??
                    .to_text()?,
            )?;
            ensure!(
                response["success"] == true,
                "Exposure metadata unavailable with this token"
            );
            let exposed = response["result"]["exposed_entities"]
                .as_object()
                .context("Invalid exposure metadata")?;
            Ok::<_, anyhow::Error>(
                exposed
                    .iter()
                    .filter(|(_, settings)| settings["conversation"] == true)
                    .map(|(id, _)| id.to_owned())
                    .collect::<std::collections::HashSet<_>>(),
            )
        })
        .await??;
        println!("Entities exposed to Assist: {}", exposed_ids.len());
        println!(
            "Temperature sensors exposed to Assist: {}",
            catalog
                .iter()
                .filter(|entry| exposed_ids.contains(entry["id"].as_str().unwrap_or_default()))
                .count()
        );
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let mut named_successes = 0;
        for (index, entry) in catalog
            .iter()
            .filter(|entry| exposed_ids.contains(entry["id"].as_str().unwrap_or_default()))
            .take(3)
            .enumerate()
        {
            let Some(name) = entry["name"].as_str() else {
                continue;
            };
            let area = entry["area"].as_str();
            println!(
                "Named query {}: same-name sensors={}, has area={}",
                index + 1,
                catalog
                    .iter()
                    .filter(|other| other["name"] == entry["name"])
                    .count(),
                area.is_some()
            );
            let ru_question = area
                .map(|area| format!("какая температура {name} в {area}"))
                .unwrap_or_else(|| format!("какая температура {name}"));
            let en_question = area
                .map(|area| format!("what is {name} in {area}"))
                .unwrap_or_else(|| format!("what is {name}"));
            for (language, question) in [
                (crate::i18n::Language::Ru, ru_question),
                (crate::i18n::Language::En, en_question),
            ] {
                let recognized = client.recognize(&question, language.code()).await;
                if recognized.as_ref().is_ok_and(|r| {
                    r.intent.name == "HassGetState"
                        && r.targets
                            .contains_key(entry["id"].as_str().unwrap_or_default())
                }) {
                    let response = crate::core::readonly::answer(
                        &client, 1, true, &question, language, &pool, 15,
                    )
                    .await?;
                    ensure!(!response.is_empty(), "Empty named state answer");
                    named_successes += 1;
                    println!(
                        "Named temperature query {} {}: recognized and answered (contents omitted)",
                        index + 1,
                        language.code()
                    );
                } else {
                    println!(
                        "Named temperature query {} {}: no matching target",
                        index + 1,
                        language.code()
                    );
                    match recognized {
                        Ok(r) => {
                            println!(
                                "Recognition details: intent={}, targets={}, slots={:?}",
                                r.intent.name,
                                r.targets.len(),
                                r.details.keys().collect::<Vec<_>>()
                            );
                            if let Some(slot) = r.details.get("name") {
                                println!("Canonical name equals entity ID: {}; equals HA display name: {}; valid entity ID: {}", slot.value == entry["id"], slot.value == entry["name"], slot.value.as_str().is_some_and(valid_entity_id));
                            }
                        }
                        Err(error) => println!("Recognition rejection: {error}"),
                    }
                }
            }
        }
        println!("Named temperature queries answered: {named_successes}");
        Ok(())
    }
    fn response() -> Value {
        json!({"id":1,"type":"result","success":true,"result":{"results":[{
            "match":true,"intent":{"name":"HassGetState"},"details":{},
            "targets":{"light.test":{"matched":false}},"source":"builtin"}]}})
    }
    #[test]
    fn validates_protocol_without_trusting_matches() {
        assert!(!parse_result(response()).unwrap().targets["light.test"].matched);
        for (pointer, value) in [
            ("/id", json!(2)),
            ("/success", json!(false)),
            ("/result/results", json!([])),
            ("/result/results/0", Value::Null),
            ("/result/results/0/source", json!("trigger")),
            ("/result/results/0/match", json!(false)),
            (
                "/result/results/0/targets/light.test/matched",
                json!("true"),
            ),
        ] {
            let mut r = response();
            *r.pointer_mut(pointer).unwrap() = value;
            assert!(parse_result(r).is_err(), "{pointer}");
        }
        assert_eq!(
            websocket_url("https://host/core/").unwrap(),
            "wss://host/core/api/websocket"
        );
    }
    #[tokio::test]
    async fn websocket_sends_only_auth_and_diagnostic_recognition() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::Text(
                json!({"type":"auth_required"}).to_string().into(),
            ))
            .await
            .unwrap();
            let auth: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(auth, json!({"type":"auth","access_token":"test"}));
            ws.send(Message::Text(json!({"type":"auth_ok"}).to_string().into()))
                .await
                .unwrap();
            let request: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(
                request,
                json!({"id":1,"type":"conversation/agent/homeassistant/debug","sentences":["show status and activate the night scene"],"language":"en"})
            );
            ws.send(Message::Text(response().to_string().into()))
                .await
                .unwrap();
            // Client closes after the diagnostic result; no execution follows.
            assert!(!matches!(ws.next().await, Some(Ok(Message::Text(_)))));
        });
        // Keep the URL alive for the borrowed client.
        let url = format!("http://{address}");
        let client = RecognitionClient::new(&url, "test", 2)?;
        client
            .recognize("show status and activate the night scene", "en")
            .await?;
        server.await?;
        Ok(())
    }

    #[tokio::test]
    async fn authentication_failure_and_deadline_do_not_fall_back_to_execution() -> Result<()> {
        for stalled in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let url = format!("http://{}", listener.local_addr()?);
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                if stalled {
                    // Client's overall deadline also covers the initial auth wait.
                    assert!(!matches!(ws.next().await, Some(Ok(Message::Text(_)))));
                    return;
                }
                ws.send(Message::Text(
                    json!({"type":"auth_required"}).to_string().into(),
                ))
                .await
                .unwrap();
                let auth: Value =
                    serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                assert_eq!(auth["type"], "auth");
                ws.send(Message::Text(
                    json!({"type":"auth_invalid","message":"private server details"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
                assert!(!matches!(ws.next().await, Some(Ok(Message::Text(_)))));
            });
            let client = RecognitionClient::new(&url, "test", 1)?;
            let error = client
                .recognize("turn everything on", "en")
                .await
                .err()
                .context("expected rejection")?;
            assert!(!error.to_string().contains("private server details"));
            server.await?;
        }
        Ok(())
    }
}
