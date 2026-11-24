use super::*;
use futures_util::StreamExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use teloxide::update_listeners::AsUpdateStream;

fn message() -> Value {
    json!({
        "message_id":12,"date":1,
        "from":{"id":7,"is_bot":false,"first_name":"Test"},
        "chat":{"id":7,"type":"private"},
        "text":"/start",
        "entities":[{"type":"bot_command","offset":0,"length":6}]
    })
}

#[test]
fn ordinary_command_survives_raw_polling() -> Result<()> {
    let update = normalize(&json!({"update_id":3,"message":message()}))?;
    assert_eq!(update.id, UpdateId(3));
    assert!(crate::bot::is_verified_private_update(&update));
    let UpdateKind::Message(message) = update.kind else {
        panic!("Expected a command message, got {:?}", update.kind);
    };
    assert_eq!(message.text(), Some("/start"));
    assert_eq!(message.entities().unwrap().len(), 1);
    Ok(())
}

#[test]
fn classic_callback_survives_raw_polling() -> Result<()> {
    let update = normalize(&json!({"update_id":4,"callback_query":{
        "id":"classic","from":{"id":7,"is_bot":false,"first_name":"Test"},
        "chat_instance":"chat","data":"existing-route","message":message()
    }}))?;
    assert!(crate::bot::is_verified_private_update(&update));
    let UpdateKind::CallbackQuery(query) = update.kind else {
        panic!("Expected a classic callback, got {:?}", update.kind);
    };
    assert_eq!(query.data.as_deref(), Some("existing-route"));
    assert!(!super::super::is_widget(&query));
    Ok(())
}

#[test]
fn form_reply_preserves_text_sender_and_reply() -> Result<()> {
    let mut reply = message();
    reply["text"] = json!("Название сценария");
    reply["entities"] = json!([]);
    reply["reply_to_message"] = message();
    let update = normalize(&json!({"update_id":5,"message":reply}))?;
    assert!(crate::bot::is_verified_private_update(&update));
    let UpdateKind::Message(message) = update.kind else {
        panic!("Expected a form reply");
    };
    assert_eq!(message.text(), Some("Название сценария"));
    assert_eq!(message.from.as_ref().unwrap().id.0, 7);
    assert_eq!(message.chat.id, ChatId(7));
    assert_eq!(message.reply_to_message().unwrap().id.0, 12);
    Ok(())
}

#[test]
fn voice_message_survives_raw_polling() -> Result<()> {
    let mut voice = message();
    voice.as_object_mut().unwrap().remove("text");
    voice.as_object_mut().unwrap().remove("entities");
    voice["voice"] = json!({
        "file_id":"voice-file","file_unique_id":"unique-voice","duration":2,
        "mime_type":"audio/ogg","file_size":128
    });
    let update = normalize(&json!({"update_id":6,"message":voice}))?;
    let UpdateKind::Message(message) = update.kind else {
        panic!("Expected a voice message");
    };
    assert_eq!(message.voice().unwrap().file.id.0, "voice-file");
    Ok(())
}

#[test]
fn unknown_and_malformed_updates_do_not_reach_dispatcher() {
    for raw in [
        json!({"update_id":7,"future_update":{"data":"private"}}),
        json!({"update_id":8,"message":{"message_id":1,"text":"private"}}),
        json!({"update_id":9,"callback_query":{"id":"q","data":"private"}}),
        json!({"update_id":10,"widget_query":{"id":"q","message_id":1}}),
        json!({"message":message()}),
        json!({"update_id":11}),
    ] {
        assert!(normalize(&raw).is_err());
    }
}

#[test]
fn normalizes_standard_widget_data_queries_and_rejects_wallet_queries() -> Result<()> {
    let mut raw = json!({"update_id":18,"widget_query":{
        "id":"remote-id",
        "from":{"id":7,"is_bot":false,"first_name":"Test"},
        "chat":{"id":7,"type":"private"},
        "chat_instance":"telegram-chat",
        "message_id":12,
        "type":{"type":"data"},
        "data":"opaque-key"
    }});
    let update = normalize(&raw)?;
    assert!(crate::bot::is_verified_private_update(&update));
    let UpdateKind::CallbackQuery(query) = update.kind else {
        panic!("Expected a widget callback");
    };
    assert!(super::super::is_widget(&query));
    assert_eq!(query.id.0, "html:remote-id");
    assert_eq!(query.data.as_deref(), Some("opaque-key"));
    assert_eq!(query.message.unwrap().id().0, 12);

    for kind in ["sign_data", "sent_transaction"] {
        raw["widget_query"]["type"]["type"] = json!(kind);
        assert!(normalize(&raw).is_err());
    }
    Ok(())
}

#[test]
fn fallback_requires_an_explicit_update_type_rejection() {
    for (code, reason) in [
        (400, "Bad Request: unknown update type \"widget_query\""),
        (400, "Bad Request: unsupported update type"),
        (400, "Bad Request: widget_query is not supported"),
        (
            400,
            "Bad Request: allowed_updates includes an invalid update type",
        ),
    ] {
        assert!(rejects_widget_updates(code, reason), "{code} {reason}");
    }
    for (code, reason) in [
        (400, "Bad Request: invalid offset"),
        (400, "Bad Request: could not parse allowed_updates"),
        (401, "Unauthorized"),
        (409, "Conflict: terminated by other getUpdates request"),
        (429, "Too Many Requests"),
        (503, "Service unavailable: widget_query not supported"),
    ] {
        assert!(!rejects_widget_updates(code, reason), "{code} {reason}");
    }
}

#[test]
fn recognizes_widget_edit_errors_by_description() {
    for (reason, expected) in [
        (
            "Bad Request: message to edit not found",
            ApiError::MessageToEditNotFound,
        ),
        (
            "Bad Request: MESSAGE_ID_INVALID",
            ApiError::MessageIdInvalid,
        ),
        (
            "Bad Request: message is not modified",
            ApiError::MessageNotModified,
        ),
        (
            "Bad Request: MESSAGE_NOT_MODIFIED",
            ApiError::MessageNotModified,
        ),
        (
            "Bad Request: message is not modified: new chat data match",
            ApiError::MessageNotModified,
        ),
    ] {
        assert_eq!(widget_edit_error(reason), Some(expected));
    }
    assert!(widget_edit_error("Bad Request: invalid widget data").is_none());
}

#[tokio::test]
async fn polling_continues_with_classic_updates_at_the_same_offset_after_widget_rejection(
) -> Result<()> {
    let callback = json!({"update_id":43,"callback_query":{
        "id":"classic","from":{"id":7,"is_bot":false,"first_name":"Test"},
        "chat_instance":"chat","data":"existing-route","message":message()
    }});
    let (bot, server) = crate::bot::router::control_tests::mock_telegram_edits(vec![
        (200, json!({"ok":true,"result":[{"update_id":41,"message":message()}]})),
        (401, json!({"ok":false,"error_code":401,"description":"Unauthorized"})),
        (400, json!({"ok":false,"error_code":400,"description":"Bad Request: widget_query is not supported"})),
        (200, json!({"ok":true,"result":[callback]})),
    ])
    .await?;
    let mut api_url = bot.api_url();
    api_url.set_path("/custom-bot-api/");
    let rejected = Arc::new(AtomicBool::new(false));
    let signal = rejected.clone();
    let mut listener = listener(bot.set_api_url(api_url), move || {
        signal.store(true, Ordering::Relaxed);
    });
    let updates = listener.as_stream();
    futures_util::pin_mut!(updates);
    let first = tokio::time::timeout(Duration::from_secs(5), updates.next())
        .await?
        .context("Expected a command")??;
    assert_eq!(first.id.0, 41);
    assert!(matches!(first.kind, UpdateKind::Message(_)));
    assert!(!rejected.load(Ordering::Relaxed));
    let error = tokio::time::timeout(Duration::from_secs(5), updates.next())
        .await?
        .context("Expected an authorization error")?
        .unwrap_err();
    assert!(error.to_string().contains("Unauthorized"));
    assert!(!rejected.load(Ordering::Relaxed));
    let second = tokio::time::timeout(Duration::from_secs(5), updates.next())
        .await?
        .context("Expected a classic callback")??;
    assert_eq!(second.id.0, 43);
    assert!(matches!(second.kind, UpdateKind::CallbackQuery(query)
        if query.data.as_deref() == Some("existing-route") && !super::super::is_widget(&query)));
    assert!(rejected.load(Ordering::Relaxed));

    let polls = server.await??;
    let combined = json!(["message", "callback_query", "widget_query"]);
    let classic = json!(["message", "callback_query"]);
    assert_eq!(
        polls
            .iter()
            .map(|poll| (&poll["offset"], &poll["allowed_updates"]))
            .collect::<Vec<_>>(),
        vec![
            (&json!(0), &combined),
            (&json!(42), &combined),
            (&json!(42), &combined),
            (&json!(42), &classic)
        ]
    );
    assert!(polls.iter().all(|poll| poll["method"]
        .as_str()
        .is_some_and(|method| method.starts_with("POST /custom-bot-api/bot123:test/getUpdates "))));
    Ok(())
}
