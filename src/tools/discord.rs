use std::sync::Arc;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;

use super::Tool;
use crate::chat::Run;

mod act;
mod read;

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    let mut tools = read::tools(run);
    tools.extend(act::tools(run));
    tools
}

pub(crate) fn channel_id(run: &Run, requested: Option<super::Snowflake>) -> serenity::ChannelId {
    requested.map_or(run.channel_id, |id| serenity::ChannelId::new(id.get()))
}

pub(crate) fn require_permissions(
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    channel_id: serenity::ChannelId,
    required: &[(serenity::Permissions, &'static str)],
) -> Result<serenity::GuildChannel> {
    let access = resolve_channel(discord, guild_id, member, channel_id)?;
    if let Some((_, name)) = required
        .iter()
        .find(|(permission, _)| !access.permissions.contains(*permission))
    {
        bail!("invoker is missing {name} in this channel");
    }
    Ok(access.channel)
}

pub(crate) struct ChannelAccess {
    pub channel: serenity::GuildChannel,
    pub permissions: serenity::Permissions,
}

pub(crate) fn resolve_channel(
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    channel_id: serenity::ChannelId,
) -> Result<ChannelAccess> {
    let guild = discord
        .cache
        .guild(guild_id)
        .context("server is not available in the Discord cache")?;
    let channel = guild
        .channels
        .get(&channel_id)
        .or_else(|| guild.threads.iter().find(|thread| thread.id == channel_id))
        .context("channel does not belong to this server")?;
    if channel.guild_id != guild_id {
        bail!("channel does not belong to this server");
    }
    let permission_channel = if is_thread(channel.kind) {
        let parent_id = channel.parent_id.context("thread has no parent channel")?;
        guild
            .channels
            .get(&parent_id)
            .context("thread parent is not available in the Discord cache")?
    } else {
        channel
    };
    let permissions = guild.user_permissions_in(permission_channel, member);
    Ok(ChannelAccess {
        channel: channel.clone(),
        permissions,
    })
}

pub(crate) const fn is_thread(kind: serenity::ChannelType) -> bool {
    matches!(
        kind,
        serenity::ChannelType::NewsThread
            | serenity::ChannelType::PublicThread
            | serenity::ChannelType::PrivateThread
    )
}

pub(crate) fn read_channel(
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    channel_id: serenity::ChannelId,
) -> Result<serenity::GuildChannel> {
    require_permissions(
        discord,
        guild_id,
        member,
        channel_id,
        &[
            (serenity::Permissions::VIEW_CHANNEL, "VIEW_CHANNEL"),
            (
                serenity::Permissions::READ_MESSAGE_HISTORY,
                "READ_MESSAGE_HISTORY",
            ),
        ],
    )
}

pub(crate) fn jump_link(
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
) -> String {
    format!("https://discord.com/channels/{guild_id}/{channel_id}/{message_id}")
}

pub(crate) fn channel_link(guild_id: serenity::GuildId, channel_id: serenity::ChannelId) -> String {
    format!("https://discord.com/channels/{guild_id}/{channel_id}")
}
