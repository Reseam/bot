use anyhow::Result;
use poise::serenity_prelude as serenity;
use sqlx::sqlite::SqlitePoolOptions;

use super::*;

#[tokio::test]
async fn reminders_are_capped_and_scoped_to_their_owner() -> Result<()> {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::migrate!().run(&db).await?;
    let guild_id = serenity::GuildId::new(1);
    let owner = serenity::UserId::new(2);
    let other = serenity::UserId::new(3);
    let reminder = |user_id| NewReminder {
        guild_id,
        channel_id: serenity::ChannelId::new(4),
        user_id,
        message: "check the deploy",
    };

    for due_at in 0..MAX_PER_USER {
        assert!(insert(&db, &reminder(owner), 0, 100 + due_at).await?);
    }
    assert!(!insert(&db, &reminder(owner), 0, 500).await?);
    assert!(insert(&db, &reminder(other), 0, 50).await?);

    let first = list(&db, guild_id, owner).await?[0].id;
    assert!(!cancel(&db, guild_id, other, first).await?);
    assert!(cancel(&db, guild_id, owner, first).await?);
    assert!(insert(&db, &reminder(owner), 0, 500).await?);

    assert_eq!(due(&db, 100).await?.len(), 1);
    assert_eq!(due(&db, 101).await?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn schedule_rejects_bad_delays_and_messages() -> Result<()> {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::migrate!().run(&db).await?;
    let reminder = |message| NewReminder {
        guild_id: serenity::GuildId::new(1),
        channel_id: serenity::ChannelId::new(2),
        user_id: serenity::UserId::new(3),
        message,
    };
    let long = "a".repeat(MAX_MESSAGE_CHARS + 1);

    assert!(schedule(&db, &reminder("ok"), "366d").await.is_err());
    assert!(schedule(&db, &reminder("ok"), "tomorrow").await.is_err());
    assert!(schedule(&db, &reminder("  \n"), "1h").await.is_err());
    assert!(schedule(&db, &reminder(&long), "1h").await.is_err());
    let before = serenity::Timestamp::now().unix_timestamp();
    let due_at = schedule(&db, &reminder("ok"), "365d").await?;
    assert!((0..=1).contains(&(due_at - before - 365 * 86_400)));
    Ok(())
}
