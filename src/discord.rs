use std::cmp::Reverse;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;

const PAGE_SIZE: u8 = 100;
pub const UNKNOWN_MEMBER: isize = 10_007;
pub const UNKNOWN_BAN: isize = 10_026;
pub const ATTACHMENT_LIMIT: usize = 10;
pub const UPLOAD_LIMIT_BYTES: usize = 10 * 1024 * 1024;

pub struct ChannelAccess {
    pub channel: serenity::GuildChannel,
    pub permissions: serenity::Permissions,
}

pub async fn resolve_channel(
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    channel_id: serenity::ChannelId,
) -> Result<ChannelAccess> {
    let access = {
        let guild = discord
            .cache
            .guild(guild_id)
            .context("server is not available in the Discord cache")?;
        let channel = guild
            .channels
            .get(&channel_id)
            .or_else(|| guild.threads.iter().find(|thread| thread.id == channel_id))
            .context("channel does not belong to this server")?;
        let permission_channel = if is_thread(channel.kind) {
            let parent_id = channel.parent_id.context("thread has no parent channel")?;
            guild
                .channels
                .get(&parent_id)
                .context("thread parent is not available in the Discord cache")?
        } else {
            channel
        };
        ChannelAccess {
            permissions: guild.user_permissions_in(permission_channel, member),
            channel: channel.clone(),
        }
    };
    if access.channel.kind == serenity::ChannelType::PrivateThread {
        let bot_id = discord.cache.current_user().id;
        if !is_thread_member(discord, channel_id, bot_id).await? {
            bail!("the bot is not a member of this private thread");
        }
        if !is_thread_member(discord, channel_id, member.user.id).await? {
            bail!("the invoker is not a member of this private thread");
        }
    }
    Ok(access)
}

pub async fn require_permissions(
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
    channel_id: serenity::ChannelId,
    required: serenity::Permissions,
) -> Result<serenity::GuildChannel> {
    let access = resolve_channel(discord, guild_id, member, channel_id).await?;
    let missing = required - access.permissions;
    if !missing.is_empty() {
        bail!(
            "invoker is missing {} in this channel",
            missing.get_permission_names().join(", ")
        );
    }
    Ok(access.channel)
}

pub async fn read_channel(
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
        serenity::Permissions::VIEW_CHANNEL | serenity::Permissions::READ_MESSAGE_HISTORY,
    )
    .await
}

pub fn send_permission(channel: &serenity::GuildChannel) -> serenity::Permissions {
    if is_thread(channel.kind) {
        serenity::Permissions::SEND_MESSAGES_IN_THREADS
    } else {
        serenity::Permissions::SEND_MESSAGES
    }
}

async fn is_thread_member(
    discord: &serenity::Context,
    thread_id: serenity::ChannelId,
    user_id: serenity::UserId,
) -> Result<bool> {
    match thread_id.get_thread_member(discord, user_id, false).await {
        Ok(_) => Ok(true),
        Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)))
            if response.status_code.as_u16() == 404 =>
        {
            Ok(false)
        }
        Err(error) => Err(error).context("failed to check private thread membership"),
    }
}

pub fn error_code(error: &serenity::Error) -> Option<isize> {
    match error {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) => {
            Some(response.error.code)
        }
        _ => None,
    }
}

pub const fn is_thread(kind: serenity::ChannelType) -> bool {
    matches!(
        kind,
        serenity::ChannelType::NewsThread
            | serenity::ChannelType::PublicThread
            | serenity::ChannelType::PrivateThread
    )
}

pub fn jump_link(
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
) -> String {
    format!("https://discord.com/channels/{guild_id}/{channel_id}/{message_id}")
}

pub fn channel_link(guild_id: serenity::GuildId, channel_id: serenity::ChannelId) -> String {
    format!("https://discord.com/channels/{guild_id}/{channel_id}")
}

pub struct History {
    channel_id: serenity::ChannelId,
    cursor: Cursor,
}

#[derive(Clone, Copy)]
enum Cursor {
    Before(Option<serenity::MessageId>),
    After(serenity::MessageId),
    Done,
}

impl History {
    pub fn before(channel_id: serenity::ChannelId, before: Option<serenity::MessageId>) -> Self {
        Self {
            channel_id,
            cursor: Cursor::Before(before),
        }
    }

    pub fn after(channel_id: serenity::ChannelId, after: serenity::MessageId) -> Self {
        Self {
            channel_id,
            cursor: Cursor::After(after),
        }
    }

    pub async fn next_page(
        &mut self,
        discord: &serenity::Context,
    ) -> Result<Vec<serenity::Message>> {
        let builder = serenity::GetMessages::new().limit(PAGE_SIZE);
        let builder = match self.cursor {
            Cursor::Before(Some(id)) => builder.before(id),
            Cursor::Before(None) => builder,
            Cursor::After(id) => builder.after(id),
            Cursor::Done => return Ok(Vec::new()),
        };
        let mut page = self
            .channel_id
            .messages(discord, builder)
            .await
            .context("failed to fetch channel messages")?;
        self.cursor = match self.cursor {
            Cursor::Before(_) => {
                page.sort_by_key(|message| Reverse(message.id));
                page.last()
                    .map_or(Cursor::Done, |message| Cursor::Before(Some(message.id)))
            }
            _ => {
                page.sort_by_key(|message| message.id);
                page.last()
                    .map_or(Cursor::Done, |message| Cursor::After(message.id))
            }
        };
        if page.len() < usize::from(PAGE_SIZE) {
            self.cursor = Cursor::Done;
        }
        Ok(page)
    }
}
