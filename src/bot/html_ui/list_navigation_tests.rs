use super::*;

#[tokio::test]
async fn source_list_arrows_open_first_or_last_part_without_skipping_items() -> Result<()> {
    let (config, _) = mock_config().await?;
    let fake = FakeTelegram::new().await?;
    for id in 2..=25 {
        db::add_user(id, &config.db).await?;
    }
    let dialogue = Dialogue::new(
        teloxide::dispatching::dialogue::InMemStorage::<crate::bot::State>::new(), ChatId(1),
    );
    publish(&fake, &config, Payload::Admin(AdminPayload::ListUsersPage { page: 0 })).await?;
    let first = current(&config);
    assert!(first.pages.len() > 1);
    assert_eq!(first.pages[0].list.as_ref().unwrap().0, "Список 1/3");
    let original_actions = first.actions.values().filter(|p| matches!(p, Payload::Admin(AdminPayload::UserProfile { .. }))).count();
    assert_eq!(original_actions, 10);
    assert!(first.pages.iter().flat_map(|p| &p.rows).flatten().all(|b| !matches!(first.actions.get(&b.key), Some(Payload::Admin(AdminPayload::ListUsersPage { .. })))));
    resolve(&fake.bot, &query(&first, &format!("p{}", first.pages.len()-1)), &config).await?;
    let before = current(&config);
    let next = before.pages[before.index].list.as_ref().unwrap().2.as_deref().unwrap();
    press(&fake, &config, &dialogue, query(&before, next)).await?;
    let second = current(&config);
    assert_eq!(second.index, 0, "Next source page must start at its first part");
    assert_eq!(second.pages[0].list.as_ref().unwrap().0, "Список 2/3");
    let previous = second.pages[0].list.as_ref().unwrap().1.as_deref().unwrap();
    press(&fake, &config, &dialogue, query(&second, previous)).await?;
    let returned = current(&config);
    assert_eq!(returned.index, returned.pages.len()-1, "Previous source page must open its last part");
    assert_eq!(returned.pages[0].list.as_ref().unwrap().0, "Список 1/3");
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users").fetch_one(&config.db).await?, 25);
    Ok(())
}

#[tokio::test]
async fn details_keep_personal_actions_before_full_diagnostics() -> Result<()> {
    let (config, _) = mock_config().await?;
    let (room, device) = device(&config, "light.floor_lamp").await?;
    let view = router(Payload::DeviceDetails { room, device }, 1, config.clone()).await?;
    let prepared = presentation::prepare(&view, "a".repeat(32), 1)?;
    let first = &prepared.pages[0];
    assert!(first.rows.iter().flatten().any(|b| matches!(prepared.actions.get(&b.key), Some(Payload::Personal(PersonalPayload::FavoriteOptions { .. })))));
    assert!(first.rows.iter().flatten().any(|b| matches!(prepared.actions.get(&b.key), Some(Payload::Personal(PersonalPayload::SetSubscription { .. })))));
    assert!(!first.body.contains("EntityAttributes"));
    assert!(prepared.pages.iter().skip(1).any(|p| p.body.contains("EntityAttributes")));
    assert_complete(&view, &prepared)?;
    Ok(())
}
