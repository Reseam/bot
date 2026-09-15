use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;

use crate::agent::{AgentEvent, Outcome};
use crate::text::{DISCORD_MESSAGE_LIMIT, split_message, truncate_chars};

const ANSWER_LIMIT: usize = 1_800;
const EDIT_INTERVAL: Duration = Duration::from_millis(1_500);
const FOOTER_LIMIT: usize = DISCORD_MESSAGE_LIMIT - ANSWER_LIMIT;

pub enum FinalState {
    Outcome(Outcome),
    Error(String),
}

pub struct Renderer {
    messages: Vec<serenity::Message>,
    answer: String,
    running: Vec<String>,
    finished: usize,
    failed: usize,
    rendered: Vec<String>,
    last_edit: Instant,
    separate_next_text: bool,
    compacted: bool,
}

impl Renderer {
    pub async fn start(
        discord: &serenity::Context,
        channel_id: serenity::ChannelId,
        reply_to: serenity::MessageId,
    ) -> Result<Self> {
        let mut message = channel_id
            .send_message(
                discord,
                serenity::CreateMessage::new()
                    .content("-# Thinking…")
                    .reference_message((channel_id, reply_to))
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send initial run reply")?;
        message
            .edit(
                discord,
                serenity::EditMessage::new().components(stop_components(message.id)),
            )
            .await
            .context("failed to attach stop button")?;
        Ok(Self {
            messages: vec![message],
            answer: String::new(),
            running: Vec::new(),
            finished: 0,
            failed: 0,
            rendered: vec!["-# Thinking…".to_owned()],
            last_edit: Instant::now(),
            separate_next_text: false,
            compacted: false,
        })
    }

    pub fn message_ids(&self) -> impl Iterator<Item = serenity::MessageId> + '_ {
        self.messages.iter().map(|message| message.id)
    }

    pub fn apply(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::TurnStarted => self.separate_next_text = !self.answer.is_empty(),
            AgentEvent::Text(text) => {
                self.compacted = false;
                if self.separate_next_text {
                    self.answer.truncate(self.answer.trim_end().len());
                    if !self.answer.is_empty() {
                        self.answer.push_str("\n\n");
                    }
                    self.separate_next_text = false;
                }
                self.answer.push_str(&text);
            }
            AgentEvent::ToolStarted { id } => self.running.push(id),
            AgentEvent::ToolFinished { id, is_error } => {
                self.running.retain(|running| *running != id);
                self.finished += 1;
                self.failed += usize::from(is_error);
            }
            AgentEvent::Compacted => self.compacted = true,
        }
    }

    pub async fn refresh(
        &mut self,
        discord: &serenity::Context,
    ) -> Result<Vec<serenity::MessageId>> {
        if self.last_edit.elapsed() < EDIT_INTERVAL {
            return Ok(Vec::new());
        }
        let mut footer = match self.running.len() {
            0 => "-# Thinking…".to_owned(),
            1 => "-# Running a command…".to_owned(),
            count => format!("-# Running {count} commands…"),
        };
        if self.compacted {
            footer.push_str("\n-# Compacted earlier context");
        }
        self.update(discord, &footer, true).await
    }

    pub async fn finish(
        &mut self,
        discord: &serenity::Context,
        state: FinalState,
    ) -> Result<Vec<serenity::MessageId>> {
        let footer = match state {
            FinalState::Outcome(Outcome::Finished) if self.finished == 0 => String::new(),
            FinalState::Outcome(Outcome::Finished) => self.commands_footer(),
            FinalState::Outcome(Outcome::Cancelled) => "-# Stopped".to_owned(),
            FinalState::Outcome(Outcome::TurnLimit) => "-# Reached the turn limit".to_owned(),
            FinalState::Error(error) => format!("-# Error: {}", short_error(&error)),
        };
        if self.answer.trim().is_empty() && footer.is_empty() {
            self.answer = "Done.".to_owned();
        }
        self.update(discord, &footer, false).await
    }

    async fn update(
        &mut self,
        discord: &serenity::Context,
        footer: &str,
        with_stop: bool,
    ) -> Result<Vec<serenity::MessageId>> {
        let footer = truncate_chars(footer, FOOTER_LIMIT.saturating_sub(2));
        let mut chunks = split_message(self.answer.trim_end(), ANSWER_LIMIT);
        if chunks.is_empty() {
            chunks.push(String::new());
        }
        let final_index = chunks.len() - 1;
        if !footer.is_empty() {
            if !chunks[final_index].is_empty() {
                chunks[final_index].push_str("\n\n");
            }
            chunks[final_index].push_str(&footer);
        }

        let mut added = Vec::new();
        while self.messages.len() < chunks.len() {
            let previous = self
                .messages
                .last()
                .map(|message| message.id)
                .context("renderer has no message to continue from")?;
            if let Some(message) = self.messages.last_mut() {
                message
                    .edit(discord, serenity::EditMessage::new().components(Vec::new()))
                    .await
                    .context("failed to move stop button")?;
            }
            let message = self.messages[0]
                .channel_id
                .send_message(
                    discord,
                    serenity::CreateMessage::new()
                        .content("-# Thinking…")
                        .reference_message((self.messages[0].channel_id, previous))
                        .allowed_mentions(serenity::CreateAllowedMentions::new()),
                )
                .await
                .context("failed to send continued run reply")?;
            added.push(message.id);
            self.messages.push(message);
            self.rendered.push("-# Thinking…".to_owned());
        }

        for (index, chunk) in chunks.iter().enumerate() {
            let components = if with_stop && index == final_index {
                stop_components(self.messages[index].id)
            } else {
                Vec::new()
            };
            if self.rendered[index] != *chunk {
                self.messages[index]
                    .edit(
                        discord,
                        serenity::EditMessage::new()
                            .content(chunk)
                            .components(components)
                            .allowed_mentions(serenity::CreateAllowedMentions::new()),
                    )
                    .await
                    .context("failed to update run reply")?;
                self.rendered[index] = chunk.clone();
            }
        }
        self.last_edit = Instant::now();
        Ok(added)
    }

    fn commands_footer(&self) -> String {
        let commands = if self.finished == 1 {
            "1 command".to_owned()
        } else {
            format!("{} commands", self.finished)
        };
        if self.failed == 0 {
            format!("-# Ran {commands}")
        } else {
            format!("-# Ran {commands} ({} failed)", self.failed)
        }
    }
}

fn stop_components(message_id: serenity::MessageId) -> Vec<serenity::CreateActionRow> {
    vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("stop:{message_id}"))
            .label("Stop")
            .style(serenity::ButtonStyle::Danger),
    ])]
}

fn short_error(error: &str) -> String {
    truncate_chars(error.lines().next().unwrap_or("run failed"), 160)
}
