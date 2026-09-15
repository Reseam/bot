use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use tracing::error;

use crate::db::{discord_id, stored_discord_id};
use crate::discord::{UNKNOWN_BAN, error_code};

pub mod actions;
pub mod commands;

const SEND_PERMISSIONS: serenity::Permissions = serenity::Permissions::SEND_MESSAGES
    .union(serenity::Permissions::SEND_MESSAGES_IN_THREADS)
    .union(serenity::Permissions::CREATE_PUBLIC_THREADS)
    .union(serenity::Permissions::CREATE_PRIVATE_THREADS);
const STAFF_PERMISSIONS: serenity::Permissions = serenity::Permissions::ADMINISTRATOR
    .union(serenity::Permissions::MANAGE_CHANNELS)
    .union(serenity::Permissions::MANAGE_MESSAGES)
    .union(serenity::Permissions::MODERATE_MEMBERS);

#[derive(Clone, Copy)]
pub enum Action {
    Warn,
    Timeout,
    Untimeout,
    Kick,
    Ban,
    Unban,
    Delete,
    Purge,
    Slowmode,
    Lock,
    Unlock,
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Warn => "Warn",
            Self::Timeout => "Timeout",
            Self::Untimeout => "Remove timeout",
            Self::Kick => "Kick",
            Self::Ban => "Ban",
            Self::Unban => "Unban",
            Self::Delete => "Delete message",
            Self::Purge => "Purge",
            Self::Slowmode => "Slowmode",
            Self::Lock => "Lock",
            Self::Unlock => "Unlock",
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

pub struct Record<'a> {
    pub action: Action,
    pub target: Option<serenity::UserId>,
    pub channel: Option<serenity::ChannelId>,
    pub reason: &'a str,
    pub duration: Option<Duration>,
    pub expires_at: Option<i64>,
}

pub fn hierarchy_allows(actor_position: u16, target_position: u16, actor_is_owner: bool) -> bool {
    actor_is_owner || actor_position > target_position
}

pub struct PurgeCandidate<'a> {
    pub author: serenity::UserId,
    pub author_is_bot: bool,
    pub content: &'a str,
}

pub fn purge_matches(
    candidate: &PurgeCandidate<'_>,
    user: Option<serenity::UserId>,
    contains: Option<&str>,
    bots: Option<bool>,
) -> bool {
    user.is_none_or(|id| candidate.author == id)
        && contains.is_none_or(|text| candidate.content.contains(text))
        && bots.is_none_or(|want_bots| candidate.author_is_bot == want_bots)
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedOverwrite {
    pub role: u64,
    pub original: Option<(u64, u64)>,
}

pub fn lock_plan(
    overwrites: &[serenity::PermissionOverwrite],
    everyone: serenity::RoleId,
    is_staff: impl Fn(serenity::RoleId) -> bool,
) -> Vec<(SavedOverwrite, serenity::PermissionOverwrite)> {
    let existing = overwrites
        .iter()
        .find(|overwrite| overwrite.kind == serenity::PermissionOverwriteType::Role(everyone));
    let mut locked = existing.cloned().unwrap_or(serenity::PermissionOverwrite {
        allow: serenity::Permissions::empty(),
        deny: serenity::Permissions::empty(),
        kind: serenity::PermissionOverwriteType::Role(everyone),
    });
    locked.allow.remove(SEND_PERMISSIONS);
    locked.deny.insert(SEND_PERMISSIONS);
    let mut plan = vec![(
        SavedOverwrite {
            role: everyone.get(),
            original: existing.map(|overwrite| (overwrite.allow.bits(), overwrite.deny.bits())),
        },
        locked,
    )];
    for overwrite in overwrites {
        let serenity::PermissionOverwriteType::Role(role) = overwrite.kind else {
            continue;
        };
        if role == everyone || is_staff(role) || !overwrite.allow.intersects(SEND_PERMISSIONS) {
            continue;
        }
        let mut locked = overwrite.clone();
        locked.allow.remove(SEND_PERMISSIONS);
        plan.push((
            SavedOverwrite {
                role: role.get(),
                original: Some((overwrite.allow.bits(), overwrite.deny.bits())),
            },
            locked,
        ));
    }
    plan
}

pub fn is_staff_role(permissions: serenity::Permissions) -> bool {
    permissions.intersects(STAFF_PERMISSIONS)
}

pub async fn set_temp_ban(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
    expires_at: Option<i64>,
) -> Result<()> {
    let query = match expires_at {
        Some(expires_at) => sqlx::query(
            "INSERT INTO temp_bans (guild_id, user_id, expires_at) VALUES (?, ?, ?) \
             ON CONFLICT(guild_id, user_id) DO UPDATE SET expires_at = excluded.expires_at",
        )
        .bind(discord_id(guild_id.get())?)
        .bind(discord_id(user_id.get())?)
        .bind(expires_at),
        None => sqlx::query("DELETE FROM temp_bans WHERE guild_id = ? AND user_id = ?")
            .bind(discord_id(guild_id.get())?)
            .bind(discord_id(user_id.get())?),
    };
    query
        .execute(db)
        .await
        .context("failed to update temporary ban")?;
    Ok(())
}

#[derive(FromRow)]
struct TempBan {
    guild_id: i64,
    user_id: i64,
}

async fn due_temp_bans(db: &SqlitePool, now: i64) -> Result<Vec<TempBan>> {
    sqlx::query_as("SELECT guild_id, user_id FROM temp_bans WHERE expires_at <= ?")
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
                error!(error = %format!("{error:#}"), "failed to process expired temporary bans");
            }
        }
    });
}

async fn process_expired_bans(discord: &serenity::Context, db: &SqlitePool) -> Result<()> {
    let bot_id = discord.cache.current_user().id;
    for ban in due_temp_bans(db, serenity::Timestamp::now().unix_timestamp()).await? {
        let guild_id = serenity::GuildId::new(stored_discord_id(ban.guild_id)?);
        let user_id = serenity::UserId::new(stored_discord_id(ban.user_id)?);
        match guild_id.unban(discord, user_id).await {
            Ok(()) => {
                set_temp_ban(db, guild_id, user_id, None).await?;
                actions::post_log(
                    discord,
                    db,
                    guild_id,
                    bot_id,
                    &Record {
                        action: Action::Unban,
                        target: Some(user_id),
                        channel: None,
                        reason: "Temporary ban expired",
                        duration: None,
                        expires_at: None,
                    },
                )
                .await;
            }
            Err(error) if error_code(&error) == Some(UNKNOWN_BAN) => {
                set_temp_ban(db, guild_id, user_id, None).await?;
            }
            Err(error) => {
                error!(?error, %guild_id, %user_id, "failed to lift expired temporary ban");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
