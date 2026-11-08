use super::*;

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
