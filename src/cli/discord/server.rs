use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Args;
use poise::serenity_prelude as serenity;

use crate::chat::Run;
use crate::cli::{CommandOutput, snowflake};
use crate::discord::{is_thread, require_permissions};
use crate::text::truncate_chars;

const CHANNEL_TOPIC_LIMIT: usize = 200;

#[derive(Args)]
pub struct Member {
    #[arg(value_parser = snowflake)]
    user: u64,
}

#[derive(Args)]
pub struct Members {
    query: String,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=25))]
    limit: u64,
}

pub fn channels(run: &Arc<Run>) -> Result<CommandOutput> {
    let guild = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?;
    let mut channels = guild.channels.values().collect::<Vec<_>>();
    channels.sort_by_key(|channel| (channel.position, channel.id));
    let visible = |channel: &serenity::GuildChannel| {
        let permission_channel = if is_thread(channel.kind) {
            channel.parent_id.and_then(|id| guild.channels.get(&id))
        } else {
            Some(channel)
        };
        permission_channel.is_some_and(|channel| {
            guild
                .user_permissions_in(channel, &run.invoker)
                .view_channel()
        })
    };
    let mut output = String::new();
    for category in channels
        .iter()
        .filter(|channel| channel.kind == serenity::ChannelType::Category && visible(channel))
    {
        writeln!(output, "Category: {} ({})", category.name, category.id)?;
        for channel in channels
            .iter()
            .filter(|channel| channel.parent_id == Some(category.id) && visible(channel))
        {
            writeln!(output, "  {}", describe_channel(channel))?;
        }
    }
    for channel in channels.iter().filter(|channel| {
        channel.kind != serenity::ChannelType::Category
            && channel.parent_id.is_none()
            && visible(channel)
    }) {
        writeln!(output, "{}", describe_channel(channel))?;
    }
    let threads = guild
        .threads
        .iter()
        .filter(|thread| thread.kind != serenity::ChannelType::PrivateThread && visible(thread))
        .collect::<Vec<_>>();
    if !threads.is_empty() {
        output.push_str("Active public threads:\n");
        for thread in threads {
            writeln!(
                output,
                "  #{} ({}, parent {})",
                thread.name,
                thread.id,
                thread
                    .parent_id
                    .map_or_else(|| "unknown".to_owned(), |id| id.to_string())
            )?;
        }
    }
    Ok(CommandOutput::text(output))
}

fn describe_channel(channel: &serenity::GuildChannel) -> String {
    let topic = channel.topic.as_deref().map_or_else(String::new, |topic| {
        format!(" | {}", truncate_chars(topic, CHANNEL_TOPIC_LIMIT))
    });
    format!(
        "{} #{} ({}){topic}",
        channel.kind.name(),
        channel.name,
        channel.id
    )
}

pub fn server(run: &Arc<Run>) -> Result<CommandOutput> {
    let guild = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?;
    let mut roles = guild.roles.values().collect::<Vec<_>>();
    roles.sort_by_key(|role| std::cmp::Reverse((role.position, role.id)));
    let mut output = format!(
        "Name: {}\nID: {}\nOwner ID: {}\nCreated: {}\nApproximate members: {}\nDescription: {}\nBoost tier: {:?}\nBoosts: {}\nEmoji count: {}\nSticker count: {}\nFeatures: {}\nRoles:",
        guild.name,
        guild.id,
        guild.owner_id,
        guild.id.created_at(),
        guild.approximate_member_count.unwrap_or(guild.member_count),
        guild.description.as_deref().unwrap_or("none"),
        guild.premium_tier,
        guild.premium_subscription_count.unwrap_or(0),
        guild.emojis.len(),
        guild.stickers.len(),
        guild.features.join(", ")
    );
    for role in roles {
        write!(
            output,
            "\n- {} ({}) | position {} | managed {}",
            role.name, role.id, role.position, role.managed
        )?;
    }
    Ok(CommandOutput::text(output))
}

pub async fn member(run: &Arc<Run>, args: Member) -> Result<CommandOutput> {
    let user_id = serenity::UserId::new(args.user);
    let channel = require_permissions(
        &run.discord,
        run.guild_id,
        &run.invoker,
        run.channel_id,
        serenity::Permissions::VIEW_CHANNEL,
    )
    .await?;
    let member = run
        .guild_id
        .member(&run.discord, user_id)
        .await
        .context("failed to fetch Discord member")?;
    let guild = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?;
    let permission_channel = match channel.parent_id {
        Some(parent_id) if is_thread(channel.kind) => guild
            .channels
            .get(&parent_id)
            .context("thread parent is not available in the Discord cache")?,
        _ => &channel,
    };
    let roles = member
        .roles
        .iter()
        .map(|id| {
            guild
                .roles
                .get(id)
                .map_or_else(|| id.to_string(), |role| format!("{} ({id})", role.name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let permissions = guild
        .user_permissions_in(permission_channel, &member)
        .get_permission_names()
        .join(", ");
    Ok(CommandOutput::text(format!(
        "Display name: {}\nUsername: {}\nID: {}\nBot: {}\nRoles: {}\nJoined: {}\nAccount created: {}\nTimeout ends: {}\nAvatar: {}\nChannel permissions: {}",
        member.display_name(),
        member.user.name,
        member.user.id,
        member.user.bot,
        if roles.is_empty() { "none" } else { &roles },
        member
            .joined_at
            .map_or_else(|| "unknown".to_owned(), |time| time.to_string()),
        member.user.created_at(),
        member
            .communication_disabled_until
            .map_or_else(|| "none".to_owned(), |time| time.to_string()),
        member.face(),
        if permissions.is_empty() {
            "none"
        } else {
            &permissions
        }
    )))
}

pub async fn members(run: &Arc<Run>, args: Members) -> Result<CommandOutput> {
    let members = run
        .guild_id
        .search_members(&run.discord, &args.query, Some(args.limit))
        .await
        .context("failed to search Discord members")?;
    let output = members
        .iter()
        .map(|member| {
            format!(
                "{} (@{}, {})",
                member.display_name(),
                member.user.name,
                member.user.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CommandOutput::text(if output.is_empty() {
        "No members found.".to_owned()
    } else {
        output
    }))
}
