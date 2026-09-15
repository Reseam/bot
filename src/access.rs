use anyhow::Result;
use poise::serenity_prelude::{RoleId, UserId};

use crate::config::Config;
use crate::{Data, Error};

pub fn has_ai_access(config: &Config, user_id: UserId, roles: &[RoleId]) -> bool {
    config.access.owner_ids.contains(&user_id)
        || roles
            .iter()
            .any(|role| config.access.role_ids.contains(role))
}

pub async fn ai_access(ctx: poise::Context<'_, Data, Error>) -> Result<bool, Error> {
    let Some(member) = ctx.author_member().await else {
        return Ok(false);
    };
    Ok(has_ai_access(
        &ctx.data().config,
        ctx.author().id,
        &member.roles,
    ))
}
