use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use tracing::error;

pub mod actions;
pub mod commands;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Warn,
    Note,
    Timeout,
    Untimeout,
    Kick,
    Ban,
    Unban,
    Purge,
    Slowmode,
    Lock,
    Unlock,
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Warn => "warn",
            Self::Note => "note",
            Self::Timeout => "timeout",
            Self::Untimeout => "untimeout",
            Self::Kick => "kick",
            Self::Ban => "ban",
            Self::Unban => "unban",
            Self::Purge => "purge",
            Self::Slowmode => "slowmode",
            Self::Lock => "lock",
            Self::Unlock => "unlock",
        })
    }
}

pub struct Moderator<'a> {
    pub discord: &'a serenity::Context,
    pub db: &'a SqlitePool,
    pub guild_id: serenity::GuildId,
    pub actor: serenity::Member,
    pub via_ai: bool,
}

impl Moderator<'_> {
    pub fn reason(&self, reason: &str) -> String {
        if self.via_ai {
            format!(
                "{reason} (via Reseam Bot, requested by {})",
                self.actor.display_name()
            )
        } else {
            reason.to_owned()
        }
    }
}

#[derive(Clone, Debug, FromRow)]
pub struct Case {
    pub id: i64,
    pub guild_id: i64,
    pub action: Action,
    pub target_id: Option<i64>,
    pub channel_id: Option<i64>,
    pub moderator_id: i64,
    pub reason: String,
    pub duration_secs: Option<i64>,
    pub expires_at: Option<i64>,
    pub resolved: bool,
    pub created_at: i64,
}

pub struct NewCase<'a> {
    pub action: Action,
    pub target_id: Option<serenity::UserId>,
    pub channel_id: Option<serenity::ChannelId>,
    pub reason: &'a str,
    pub duration: Option<Duration>,
    pub expires_at: Option<i64>,
    pub dm_delivered: Option<bool>,
}

pub async fn insert_case(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    moderator_id: serenity::UserId,
    case: &NewCase<'_>,
) -> Result<Case> {
    let now = serenity::Timestamp::now().unix_timestamp();
    let duration_secs = case
        .duration
        .map(|duration| i64::try_from(duration.as_secs()).context("duration is too large"))
        .transpose()?;
    sqlx::query_as(
        "INSERT INTO mod_cases (guild_id, action, target_id, channel_id, moderator_id, reason, \
         duration_secs, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING *",
    )
    .bind(crate::db::discord_id(guild_id.get())?)
    .bind(case.action)
    .bind(
        case.target_id
            .map(|id| crate::db::discord_id(id.get()))
            .transpose()?,
    )
    .bind(
        case.channel_id
            .map(|id| crate::db::discord_id(id.get()))
            .transpose()?,
    )
    .bind(crate::db::discord_id(moderator_id.get())?)
    .bind(case.reason)
    .bind(duration_secs)
    .bind(case.expires_at)
    .bind(now)
    .fetch_one(db)
    .await
    .context("failed to record moderation case")
}

pub async fn case_by_id(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    id: i64,
) -> Result<Option<Case>> {
    sqlx::query_as("SELECT * FROM mod_cases WHERE guild_id = ? AND id = ?")
        .bind(crate::db::discord_id(guild_id.get())?)
        .bind(id)
        .fetch_optional(db)
        .await
        .context("failed to read moderation case")
}

pub async fn cases_for(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    target_id: serenity::UserId,
) -> Result<Vec<Case>> {
    sqlx::query_as(
        "SELECT * FROM mod_cases WHERE guild_id = ? AND target_id = ? \
         ORDER BY id DESC LIMIT 25",
    )
    .bind(crate::db::discord_id(guild_id.get())?)
    .bind(crate::db::discord_id(target_id.get())?)
    .fetch_all(db)
    .await
    .context("failed to list moderation cases")
}

pub async fn expired_bans(db: &SqlitePool, now: i64) -> Result<Vec<Case>> {
    sqlx::query_as(
        "SELECT * FROM mod_cases WHERE action = ? AND expires_at <= ? AND resolved = 0 \
         ORDER BY expires_at",
    )
    .bind(Action::Ban)
    .bind(now)
    .fetch_all(db)
    .await
    .context("failed to find expired temporary bans")
}

pub fn spawn_expired_bans(discord: serenity::Context, db: SqlitePool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(error) = process_expired_bans(&discord, &db).await {
                error!(?error, "failed to process expired temporary bans");
            }
        }
    });
}

async fn process_expired_bans(discord: &serenity::Context, db: &SqlitePool) -> Result<()> {
    let cases = expired_bans(db, serenity::Timestamp::now().unix_timestamp()).await?;
    let bot_id = discord.cache.current_user().id;
    for case in cases {
        let guild_id = serenity::GuildId::new(crate::db::stored_discord_id(case.guild_id)?);
        let target_id = serenity::UserId::new(crate::db::stored_discord_id(
            case.target_id.context("expired ban case has no target")?,
        )?);
        if let Err(error) = guild_id.unban(discord, target_id).await {
            error!(?error, case_id = case.id, %guild_id, %target_id, "failed to expire temporary ban");
            continue;
        }
        sqlx::query("UPDATE mod_cases SET resolved = 1 WHERE id = ?")
            .bind(case.id)
            .execute(db)
            .await
            .context("failed to resolve expired ban case")?;
        let unban = insert_case(
            db,
            guild_id,
            bot_id,
            &NewCase {
                action: Action::Unban,
                target_id: Some(target_id),
                channel_id: None,
                reason: "Temporary ban expired",
                duration: None,
                expires_at: None,
                dm_delivered: None,
            },
        )
        .await?;
        actions::post_log(discord, db, &unban).await;
    }
    Ok(())
}

pub fn hierarchy_allows(actor_position: u16, target_position: u16, actor_is_owner: bool) -> bool {
    actor_is_owner || actor_position > target_position
}

pub struct PurgeCandidate<'a> {
    pub timestamp: serenity::Timestamp,
    pub author: serenity::UserId,
    pub author_is_bot: bool,
    pub content: &'a str,
}

pub fn purge_matches(
    candidate: PurgeCandidate<'_>,
    user: Option<serenity::UserId>,
    contains: Option<&str>,
    bots: Option<bool>,
    cutoff: serenity::Timestamp,
) -> bool {
    candidate.timestamp > cutoff
        && user.is_none_or(|id| candidate.author == id)
        && contains.is_none_or(|text| candidate.content.contains(text))
        && bots.is_none_or(|want_bots| candidate.author_is_bot == want_bots)
}

pub fn update_everyone_overwrite(
    overwrite: Option<&serenity::PermissionOverwrite>,
    everyone: serenity::RoleId,
    locked: bool,
) -> serenity::PermissionOverwrite {
    let blocked = serenity::Permissions::SEND_MESSAGES
        | serenity::Permissions::SEND_MESSAGES_IN_THREADS
        | serenity::Permissions::CREATE_PUBLIC_THREADS
        | serenity::Permissions::CREATE_PRIVATE_THREADS;
    let mut updated = overwrite.cloned().unwrap_or(serenity::PermissionOverwrite {
        allow: serenity::Permissions::empty(),
        deny: serenity::Permissions::empty(),
        kind: serenity::PermissionOverwriteType::Role(everyone),
    });
    if locked {
        updated.deny.insert(blocked);
    } else {
        updated.deny.remove(blocked);
    }
    updated
}

#[cfg(test)]
mod tests;
