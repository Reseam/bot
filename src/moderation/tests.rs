use anyhow::Result;
use poise::serenity_prelude as serenity;
use sqlx::sqlite::SqlitePoolOptions;

use super::*;

#[test]
fn lock_blocks_members_and_saves_what_unlock_restores() {
    let everyone = serenity::RoleId::new(1);
    let member_role = serenity::RoleId::new(2);
    let staff_role = serenity::RoleId::new(3);
    let everyone_overwrite = serenity::PermissionOverwrite {
        allow: serenity::Permissions::VIEW_CHANNEL,
        deny: serenity::Permissions::ADD_REACTIONS,
        kind: serenity::PermissionOverwriteType::Role(everyone),
    };
    let role_overwrite = |role| serenity::PermissionOverwrite {
        allow: serenity::Permissions::SEND_MESSAGES | serenity::Permissions::ATTACH_FILES,
        deny: serenity::Permissions::empty(),
        kind: serenity::PermissionOverwriteType::Role(role),
    };
    let overwrites = [
        everyone_overwrite.clone(),
        role_overwrite(member_role),
        role_overwrite(staff_role),
    ];

    let plan = lock_plan(&overwrites, everyone, |role| role == staff_role);

    assert_eq!(plan.len(), 2);
    let (saved, locked) = &plan[0];
    assert_eq!(
        saved,
        &SavedOverwrite {
            role: 1,
            original: Some((
                everyone_overwrite.allow.bits(),
                everyone_overwrite.deny.bits()
            )),
        }
    );
    assert!(locked.deny.contains(serenity::Permissions::SEND_MESSAGES));
    assert!(locked.deny.contains(serenity::Permissions::ADD_REACTIONS));
    let (saved, locked) = &plan[1];
    assert_eq!(saved.role, 2);
    assert!(!locked.allow.contains(serenity::Permissions::SEND_MESSAGES));
    assert!(locked.allow.contains(serenity::Permissions::ATTACH_FILES));

    let without_everyone = lock_plan(&[], everyone, |_| false);
    assert_eq!(without_everyone[0].0.original, None);
}

#[tokio::test]
async fn permanent_ban_and_unban_clear_a_pending_temporary_ban() -> Result<()> {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await?;
    sqlx::migrate!().run(&db).await?;
    let guild = serenity::GuildId::new(1);
    let user = serenity::UserId::new(2);

    set_temp_ban(&db, guild, user, Some(100)).await?;
    set_temp_ban(&db, guild, user, Some(300)).await?;
    assert!(due_temp_bans(&db, 200).await?.is_empty());
    assert_eq!(due_temp_bans(&db, 300).await?.len(), 1);

    set_temp_ban(&db, guild, user, None).await?;
    assert!(due_temp_bans(&db, i64::MAX).await?.is_empty());
    Ok(())
}
