use std::time::Duration;

use anyhow::Result;
use poise::serenity_prelude as serenity;
use sqlx::sqlite::SqlitePoolOptions;

use super::*;

#[test]
fn hierarchy_requires_a_strictly_higher_role_unless_owner() {
    assert!(hierarchy_allows(10, 9, false));
    assert!(!hierarchy_allows(10, 10, false));
    assert!(!hierarchy_allows(9, 10, false));
    assert!(hierarchy_allows(0, 100, true));
}

#[test]
fn duration_is_positive_and_at_most_twenty_eight_days() -> Result<()> {
    let max = Duration::from_secs(28 * 86_400);
    assert!(crate::text::parse_duration("28d")? <= max);
    assert!(crate::text::parse_duration("28d 1s")? > max);
    Ok(())
}

#[test]
fn purge_predicate_applies_every_filter() -> Result<()> {
    let cutoff = serenity::Timestamp::from_unix_timestamp(1_000)?;
    let recent = serenity::Timestamp::from_unix_timestamp(1_001)?;
    let author = serenity::UserId::new(7);
    let candidate = || PurgeCandidate {
        timestamp: recent,
        author,
        author_is_bot: true,
        content: "release bot completed",
    };
    assert!(purge_matches(
        candidate(),
        Some(author),
        Some("completed"),
        Some(true),
        cutoff,
    ));
    assert!(!purge_matches(
        PurgeCandidate {
            timestamp: cutoff,
            ..candidate()
        },
        None,
        None,
        None,
        cutoff,
    ));
    assert!(!purge_matches(
        candidate(),
        Some(serenity::UserId::new(8)),
        None,
        None,
        cutoff,
    ));
    assert!(!purge_matches(
        candidate(),
        None,
        Some("failed"),
        None,
        cutoff,
    ));
    assert!(!purge_matches(candidate(), None, None, Some(false), cutoff,));
    Ok(())
}

#[test]
fn lock_overwrite_preserves_unrelated_bits_and_unlock_only_clears_denies() {
    let everyone = serenity::RoleId::new(1);
    let original = serenity::PermissionOverwrite {
        allow: serenity::Permissions::SEND_MESSAGES | serenity::Permissions::VIEW_CHANNEL,
        deny: serenity::Permissions::ADD_REACTIONS,
        kind: serenity::PermissionOverwriteType::Role(everyone),
    };
    let locked = update_everyone_overwrite(Some(&original), everyone, true);
    assert!(locked.allow.contains(serenity::Permissions::SEND_MESSAGES));
    assert!(locked.allow.contains(serenity::Permissions::VIEW_CHANNEL));
    assert!(locked.deny.contains(serenity::Permissions::SEND_MESSAGES));
    assert!(locked.deny.contains(serenity::Permissions::ADD_REACTIONS));

    let unlocked = update_everyone_overwrite(Some(&locked), everyone, false);
    assert!(!unlocked.deny.contains(serenity::Permissions::SEND_MESSAGES));
    assert!(unlocked.deny.contains(serenity::Permissions::ADD_REACTIONS));
    assert!(unlocked.allow.contains(serenity::Permissions::VIEW_CHANNEL));
}

#[tokio::test]
async fn stores_cases_and_only_queries_due_unresolved_bans() -> Result<()> {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::migrate!().run(&db).await?;
    let guild = serenity::GuildId::new(1);
    let target = serenity::UserId::new(2);
    let moderator = serenity::UserId::new(3);
    let channel = serenity::ChannelId::new(4);

    let due = insert_case(
        &db,
        guild,
        moderator,
        &NewCase {
            action: Action::Ban,
            target_id: Some(target),
            channel_id: None,
            reason: "temporary",
            duration: Some(Duration::from_secs(60)),
            expires_at: Some(100),
            dm_delivered: None,
        },
    )
    .await?;
    insert_case(
        &db,
        guild,
        moderator,
        &NewCase {
            action: Action::Ban,
            target_id: Some(target),
            channel_id: None,
            reason: "future",
            duration: Some(Duration::from_secs(60)),
            expires_at: Some(300),
            dm_delivered: None,
        },
    )
    .await?;
    insert_case(
        &db,
        guild,
        moderator,
        &NewCase {
            action: Action::Warn,
            target_id: Some(target),
            channel_id: None,
            reason: "warning",
            duration: None,
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await?;
    let purge = insert_case(
        &db,
        guild,
        moderator,
        &NewCase {
            action: Action::Purge,
            target_id: Some(target),
            channel_id: Some(channel),
            reason: "filtered purge",
            duration: None,
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await?;

    let cases = cases_for(&db, guild, target).await?;
    assert_eq!(cases.len(), 4);
    assert_eq!(cases[0].action, Action::Purge);
    assert_eq!(purge.target_id, Some(2));
    assert_eq!(purge.channel_id, Some(4));
    assert_eq!(
        case_by_id(&db, guild, due.id)
            .await?
            .map(|case| case.reason),
        Some("temporary".to_owned())
    );
    assert_eq!(
        expired_bans(&db, 200)
            .await?
            .iter()
            .map(|case| case.id)
            .collect::<Vec<_>>(),
        [due.id]
    );

    sqlx::query("UPDATE mod_cases SET resolved = 1 WHERE id = ?")
        .bind(due.id)
        .execute(&db)
        .await?;
    assert!(expired_bans(&db, 200).await?.is_empty());
    Ok(())
}
