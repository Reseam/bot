use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use poise::serenity_prelude as serenity;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::llm::Message;

pub struct RunHandle {
    pub cancel: CancellationToken,
    pub steering: mpsc::UnboundedSender<Message>,
    pub invoker: serenity::UserId,
    message_ids: Mutex<HashSet<serenity::MessageId>>,
}

impl RunHandle {
    pub fn new(
        cancel: CancellationToken,
        steering: mpsc::UnboundedSender<Message>,
        invoker: serenity::UserId,
    ) -> Self {
        Self {
            cancel,
            steering,
            invoker,
            message_ids: Mutex::new(HashSet::new()),
        }
    }

    pub fn record(&self, message_id: serenity::MessageId) {
        lock(&self.message_ids).insert(message_id);
    }

    pub fn message_ids(&self) -> Vec<serenity::MessageId> {
        let mut message_ids = lock(&self.message_ids).iter().copied().collect::<Vec<_>>();
        message_ids.sort_unstable();
        message_ids
    }
}

#[derive(Default)]
struct RunMaps {
    messages: HashMap<serenity::MessageId, Arc<RunHandle>>,
    conversations: HashMap<i64, Arc<RunHandle>>,
}

#[derive(Default)]
pub struct Runs(Mutex<RunMaps>);

impl Runs {
    pub fn get(&self, message_id: serenity::MessageId) -> Option<Arc<RunHandle>> {
        lock(&self.0).messages.get(&message_id).cloned()
    }

    pub fn get_conversation(&self, conversation_id: i64) -> Option<Arc<RunHandle>> {
        lock(&self.0).conversations.get(&conversation_id).cloned()
    }

    pub fn register_message(&self, message_id: serenity::MessageId, handle: Arc<RunHandle>) {
        handle.record(message_id);
        lock(&self.0).messages.insert(message_id, handle);
    }

    pub fn register_conversation(&self, conversation_id: i64, handle: Arc<RunHandle>) {
        lock(&self.0).conversations.insert(conversation_id, handle);
    }

    pub fn remove(&self, handle: &Arc<RunHandle>) {
        let mut runs = lock(&self.0);
        runs.messages.retain(|_, value| !Arc::ptr_eq(value, handle));
        runs.conversations
            .retain(|_, value| !Arc::ptr_eq(value, handle));
    }

    pub fn cancel_all(&self) {
        let handles = lock(&self.0).messages.values().cloned().collect::<Vec<_>>();
        handles.iter().for_each(|handle| handle.cancel.cancel());
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
