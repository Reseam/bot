use anyhow::Result;
use poise::serenity_prelude::{RoleId, UserId};

use crate::config::Config;
use crate::{Data, Error};

#[derive(Clone, Copy, Debug, Eq, PartialEq, poise::ChoiceParameter)]
pub enum Tier {
    #[name = "team"]
    Team,
    #[name = "members"]
    Member,
}

pub fn tier(config: &Config, user_id: UserId, roles: &[RoleId]) -> Tier {
    if is_team(config, user_id, roles) {
        Tier::Team
    } else {
        Tier::Member
    }
}

pub fn is_team(config: &Config, user_id: UserId, roles: &[RoleId]) -> bool {
    config.access.owner_ids.contains(&user_id) || has_any(roles, &config.access.team_role_ids)
}

pub fn has_ai_access(config: &Config, user_id: UserId, roles: &[RoleId]) -> bool {
    is_team(config, user_id, roles) || has_any(roles, &config.access.member_role_ids)
}

fn has_any(roles: &[RoleId], allowed: &[RoleId]) -> bool {
    roles.iter().any(|role| allowed.contains(role))
}

pub async fn ai_access(ctx: poise::Context<'_, Data, Error>) -> Result<bool, Error> {
    check(ctx, has_ai_access).await
}

pub async fn team_only(ctx: poise::Context<'_, Data, Error>) -> Result<bool, Error> {
    check(ctx, is_team).await
}

async fn check(
    ctx: poise::Context<'_, Data, Error>,
    allowed: fn(&Config, UserId, &[RoleId]) -> bool,
) -> Result<bool, Error> {
    let Some(member) = ctx.author_member().await else {
        return Ok(false);
    };
    Ok(allowed(&ctx.data().config, ctx.author().id, &member.roles))
}
