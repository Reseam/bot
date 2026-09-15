use anyhow::Result;
use poise::serenity_prelude::{RoleId, UserId};

use crate::config::Config;
use crate::{Data, Error};

pub fn is_team(config: &Config, user_id: UserId, roles: &[RoleId]) -> bool {
    config.access.owner_ids.contains(&user_id)
        || roles
            .iter()
            .any(|role| config.access.team_role_ids.contains(role))
}

pub async fn team_only(ctx: poise::Context<'_, Data, Error>) -> Result<bool, Error> {
    let Some(member) = ctx.author_member().await else {
        return Ok(false);
    };
    Ok(is_team(&ctx.data().config, ctx.author().id, &member.roles))
}
