use std::sync::Arc;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::chat::Run;
use crate::tools::discord::{is_thread, require_permissions};
use crate::tools::{Snowflake, Tool, ToolOutput};

const DEFAULT_MEMBER_LIMIT: u8 = 10;

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    vec![
        Tool::new::<MemberInfo, _, _, _>(
            "discord_member_info",
            "Get profile, roles, dates, and current-channel permissions for a server member.",
            run.clone(),
            member_info,
        ),
        Tool::new::<SearchMembers, _, _, _>(
            "discord_search_members",
            "Search server members by username or nickname prefix.",
            run.clone(),
            search_members,
        ),
    ]
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemberInfo {
    user_id: Snowflake,
}

async fn member_info(run: Arc<Run>, args: MemberInfo) -> Result<ToolOutput> {
    let user_id = serenity::UserId::new(args.user_id.get());
    let cached = run
        .discord
        .cache
        .guild(run.guild_id)
        .and_then(|guild| guild.members.get(&user_id).cloned());
    let member = match cached {
        Some(member) => member,
        None => run
            .guild_id
            .member(&run.discord, user_id)
            .await
            .context("failed to fetch Discord member")?,
    };
    let channel = require_permissions(
        &run.discord,
        run.guild_id,
        &run.invoker,
        run.channel_id,
        &[(serenity::Permissions::VIEW_CHANNEL, "VIEW_CHANNEL")],
    )?;
    let guild = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?;
    let permission_channel = if is_thread(channel.kind) {
        channel
            .parent_id
            .and_then(|id| guild.channels.get(&id))
            .context("thread parent is not available in the Discord cache")?
    } else {
        &channel
    };
    let permissions = guild.user_permissions_in(permission_channel, &member);
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
    let permission_names = permissions.get_permission_names().join(", ");
    Ok(ToolOutput::text(format!(
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
        if permission_names.is_empty() {
            "none"
        } else {
            &permission_names
        }
    )))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchMembers {
    query: String,
    limit: Option<u8>,
}

async fn search_members(run: Arc<Run>, args: SearchMembers) -> Result<ToolOutput> {
    if args.query.trim().is_empty() {
        bail!("query must not be empty");
    }
    let limit = args.limit.unwrap_or(DEFAULT_MEMBER_LIMIT);
    if !(1..=25).contains(&limit) {
        bail!("limit must be between 1 and 25");
    }
    let members = run
        .guild_id
        .search_members(&run.discord, &args.query, Some(u64::from(limit)))
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
    Ok(ToolOutput::text(if output.is_empty() {
        "No members found.".to_owned()
    } else {
        output
    }))
}
