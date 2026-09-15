use anyhow::Result;
use sqlx::sqlite::SqlitePoolOptions;

use super::*;
use crate::llm::{ImageUrl, UserContent};
use crate::test_support::user;

async fn database() -> Result<SqlitePool> {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::migrate!().run(&db).await?;
    Ok(db)
}

fn save_request<'a>(
    id: Option<i64>,
    transcript: &'a [Message],
    message_ids: &'a [serenity::MessageId],
) -> Save<'a> {
    Save {
        id,
        guild_id: serenity::GuildId::new(10),
        channel_id: serenity::ChannelId::new(20),
        started_by: serenity::UserId::new(30),
        transcript,
        message_ids,
    }
}

#[tokio::test]
async fn saves_finds_updates_and_maps_every_message() -> Result<()> {
    let db = database().await?;
    let first = vec![user("first")];
    let first_ids = [serenity::MessageId::new(101), serenity::MessageId::new(102)];
    let id = save(&db, save_request(None, &first, &first_ids)).await?;

    for message_id in first_ids {
        let found = find_by_message(&db, message_id)
            .await?
            .expect("mapped conversation should exist");
        assert_eq!(found.id, id);
        assert_eq!(found.transcript, first);
    }

    let updated = vec![user("first"), user("second")];
    let added_ids = [serenity::MessageId::new(103)];
    assert_eq!(
        save(&db, save_request(Some(id), &updated, &added_ids)).await?,
        id
    );
    let found = find_by_message(&db, added_ids[0])
        .await?
        .expect("new mapping should exist");
    assert_eq!(found.transcript, updated);
    assert_eq!(last_message_id(&db, id).await?, Some(added_ids[0]));
    Ok(())
}

#[tokio::test]
async fn strips_image_data_from_saved_transcripts() -> Result<()> {
    let db = database().await?;
    let transcript = vec![Message::User {
        content: UserContent::Parts(vec![
            ContentPart::Text {
                text: "look".to_owned(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,secret".to_owned(),
                },
            },
        ]),
    }];
    let ids = [serenity::MessageId::new(201)];
    save(&db, save_request(None, &transcript, &ids)).await?;

    let found = find_by_message(&db, ids[0])
        .await?
        .expect("saved conversation should exist");
    assert!(matches!(
        &found.transcript[0],
        Message::User { content: UserContent::Parts(parts) }
            if matches!(&parts[1], ContentPart::Text { text } if text == "[image omitted from saved history]")
    ));
    Ok(())
}

#[tokio::test]
async fn prunes_only_conversations_older_than_cutoff() -> Result<()> {
    let db = database().await?;
    let transcript = [user("old")];
    let old_ids = [serenity::MessageId::new(301)];
    let old = save(&db, save_request(None, &transcript, &old_ids)).await?;
    let new_ids = [serenity::MessageId::new(302)];
    let new = save(&db, save_request(None, &transcript, &new_ids)).await?;
    sqlx::query("UPDATE conversations SET updated_at = 10 WHERE id = ?")
        .bind(old)
        .execute(&db)
        .await?;
    sqlx::query("UPDATE conversations SET updated_at = 20 WHERE id = ?")
        .bind(new)
        .execute(&db)
        .await?;

    assert_eq!(prune(&db, 20).await?, 1);
    assert!(find_by_message(&db, old_ids[0]).await?.is_none());
    assert!(find_by_message(&db, new_ids[0]).await?.is_some());
    Ok(())
}
