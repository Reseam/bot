use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::Mutex;
use poise::serenity_prelude as serenity;
use tokio::sync::{OwnedMutexGuard, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::agent::{Agent, AgentEvent, CompactionSettings};
use crate::llm::{Message, Model, clear_replay};
use crate::sandbox::SandboxKind;
use crate::settings;
use crate::tools;
use crate::{App, access, conversations};

pub mod approval;
pub(crate) mod context;
mod prompt;
mod render;
mod runs;
mod triggers;

use render::{FinalState, Renderer};
pub use runs::Runs;
pub use triggers::{handle_component, handle_message};

pub struct Run {
    pub app: Arc<App>,
    pub discord: serenity::Context,
    pub guild_id: serenity::GuildId,
    pub channel_id: serenity::ChannelId,
    pub invoker: serenity::Member,
    pub team: bool,
    pub model: Arc<Model>,
    pub sandbox: SandboxKind,
    pub sandbox_image: Option<String>,
    pub conversation_id: i64,
    pub cancel: CancellationToken,
    steering: mpsc::UnboundedSender<Message>,
    grants: Mutex<HashSet<String>>,
    denials: Mutex<HashSet<String>>,
    approval_notices: Mutex<Vec<String>>,
    message_ids: Mutex<HashSet<serenity::MessageId>>,
}

impl Run {
    pub fn steer(&self, message: Message) -> bool {
        self.steering.send(message).is_ok()
    }

    fn message_ids(&self) -> Vec<serenity::MessageId> {
        let mut ids = self.message_ids.lock().iter().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }
}

pub struct RunRequest {
    pub conversation_id: i64,
    pub lock: OwnedMutexGuard<()>,
    pub guild_id: serenity::GuildId,
    pub channel_id: serenity::ChannelId,
    pub reply_to: serenity::MessageId,
    pub invoker: serenity::Member,
    pub model: Arc<Model>,
    pub transcript: Vec<Message>,
    pub prompt_prefix: Option<String>,
    pub sandbox_image: Option<String>,
}

pub struct NewRun {
    pub guild_id: serenity::GuildId,
    pub channel_id: serenity::ChannelId,
    pub invoker: serenity::Member,
    pub include_history: bool,
    pub input: context::ContextInput,
}

pub async fn start_new(app: &Arc<App>, discord: &serenity::Context, new_run: NewRun) -> Result<()> {
    let conversation_id = conversations::create(
        &app.db,
        new_run.guild_id,
        new_run.channel_id,
        new_run.invoker.user.id,
    )
    .await?;
    let lock = app
        .runs
        .try_lock(conversation_id)
        .expect("a newly created conversation has no other lock holder");
    let reply_to = new_run.input.addressed_id;
    let model = guild_model(app, new_run.guild_id).await;
    let transcript = context::build(
        app,
        discord,
        new_run.channel_id,
        &new_run.invoker,
        new_run.include_history,
        model.config.vision,
        new_run.input,
    )
    .await?;
    tokio::spawn(run(
        app.clone(),
        discord.clone(),
        RunRequest {
            conversation_id,
            lock,
            guild_id: new_run.guild_id,
            channel_id: new_run.channel_id,
            reply_to,
            invoker: new_run.invoker,
            model,
            transcript,
            prompt_prefix: None,
            sandbox_image: None,
        },
    ));
    Ok(())
}

pub async fn run(app: Arc<App>, discord: serenity::Context, request: RunRequest) {
    let RunRequest {
        conversation_id,
        lock,
        guild_id,
        channel_id,
        reply_to,
        invoker,
        model,
        mut transcript,
        prompt_prefix,
        sandbox_image,
    } = request;
    let mut renderer = match Renderer::start(&discord, channel_id, reply_to).await {
        Ok(renderer) => renderer,
        Err(error) => {
            error!(?error, %channel_id, "failed to start agent run");
            return;
        }
    };
    let (steering, mut steering_rx) = mpsc::unbounded_channel();
    let team = access::is_team(&app.config, invoker.user.id, &invoker.roles);
    let sandbox = if team {
        guild_sandbox(&app, guild_id).await
    } else {
        SandboxKind::JustBash
    };
    let run = Arc::new(Run {
        app: app.clone(),
        discord: discord.clone(),
        guild_id,
        channel_id,
        invoker,
        team,
        model,
        sandbox,
        sandbox_image,
        conversation_id,
        cancel: CancellationToken::new(),
        steering,
        grants: Mutex::default(),
        denials: Mutex::default(),
        approval_notices: Mutex::default(),
        message_ids: Mutex::default(),
    });
    app.runs.start(&run);
    app.runs.register_message(reply_to, &run);
    let started = Instant::now();
    info!(
        conversation_id,
        %channel_id,
        invoker = %run.invoker.user.name,
        invoker_id = %run.invoker.user.id,
        team = run.team,
        model = %run.model.key,
        sandbox = ?run.sandbox,
        "run started"
    );
    for id in renderer.message_ids() {
        app.runs.register_message(id, &run);
    }

    let personality = settings::personality(&app.db, guild_id)
        .await
        .unwrap_or_else(|error| {
            error!(error = %format!("{error:#}"), %guild_id, "failed to read personality");
            None
        });
    let system = prompt::system(&run, personality.as_deref());
    let tools = tools::for_run(&run);
    let prefix = format!(
        "{system}\n{}",
        serde_json::to_string(&tools.specs()).expect("tool specs serialize to JSON")
    );
    if prompt_prefix.as_deref() != Some(prefix.as_str()) {
        clear_replay(&mut transcript);
    }
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let agent = Agent {
        model: &run.model,
        tools: &tools,
        system: &system,
        max_turns: app.config.agent.max_turns,
        compaction: CompactionSettings {
            context_window: u64::from(run.model.config.context_window),
            max_output_tokens: u64::from(run.model.config.max_output_tokens),
            compact_at_tokens: app.config.agent.compact_at_tokens,
            reserve_tokens: app.config.agent.compaction_reserve_tokens,
            keep_recent_tokens: app.config.agent.keep_recent_tokens,
        },
    };
    let final_state = {
        let agent_run = agent.run(&run.cancel, &mut transcript, &mut steering_rx, &event_tx);
        tokio::pin!(agent_run);
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                result = &mut agent_run => {
                    break match result {
                        Ok(outcome) => FinalState::Outcome(outcome),
                        Err(error) => {
                            error!(error = %format!("{error:#}"), "agent run failed");
                            FinalState::Error(error.to_string())
                        }
                    };
                }
                Some(event) = event_rx.recv() => renderer.apply(event),
                _ = tick.tick() => {
                    match renderer.refresh(&discord).await {
                        Ok(ids) => ids.into_iter().for_each(|id| app.runs.register_message(id, &run)),
                        Err(error) => debug!(?error, "streaming reply update failed"),
                    }
                }
            }
        }
    };
    while let Ok(event) = event_rx.try_recv() {
        renderer.apply(event);
    }
    info!(
        conversation_id,
        elapsed_secs = started.elapsed().as_secs(),
        state = ?final_state,
        "run finished"
    );
    match renderer.finish(&discord, final_state).await {
        Ok(ids) => ids
            .into_iter()
            .for_each(|id| app.runs.register_message(id, &run)),
        Err(error) => error!(?error, "failed to render final agent response"),
    }
    if run.sandbox == SandboxKind::Modal {
        save_sandbox(&run).await;
    }
    if let Err(error) = conversations::save(
        &app.db,
        conversation_id,
        &transcript,
        &prefix,
        &run.message_ids(),
    )
    .await
    {
        error!(error = %format!("{error:#}"), conversation_id, "failed to save conversation");
    }
    app.runs.remove(&run);
    drop(lock);
}

pub async fn guild_model(app: &App, guild_id: serenity::GuildId) -> Arc<Model> {
    let selected = settings::model(&app.db, guild_id)
        .await
        .unwrap_or_else(|error| {
            error!(error = %format!("{error:#}"), %guild_id, "failed to read model");
            None
        });
    let Some(key) = selected else {
        return app.llm.default_model().clone();
    };
    app.llm
        .get(&key)
        .unwrap_or_else(|| {
            warn!(model = %key, %guild_id, "selected model is no longer configured, using the default");
            app.llm.default_model()
        })
        .clone()
}

pub async fn guild_sandbox(app: &App, guild_id: serenity::GuildId) -> SandboxKind {
    let selected = settings::sandbox(&app.db, guild_id)
        .await
        .unwrap_or_else(|error| {
            error!(error = %format!("{error:#}"), %guild_id, "failed to read sandbox setting");
            None
        });
    match selected {
        Some(SandboxKind::Modal) if app.config.sandbox.modal.is_some() => SandboxKind::Modal,
        _ => SandboxKind::JustBash,
    }
}

async fn save_sandbox(run: &Run) {
    let saved = match run.app.sandbox.save(run).await {
        Ok(Some(image)) => {
            conversations::set_sandbox_image(&run.app.db, run.conversation_id, &image).await
        }
        Ok(None) => Ok(()),
        Err(error) => Err(error),
    };
    if let Err(error) = saved {
        error!(error = %format!("{error:#}"), conversation_id = run.conversation_id, "failed to save the sandbox");
    }
}
