use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;
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
        self.message_ids.lock().insert(message_id);
    }

    pub fn message_ids(&self) -> Vec<serenity::MessageId> {
        let mut message_ids = self.message_ids.lock().iter().copied().collect::<Vec<_>>();
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
        self.0.lock().messages.get(&message_id).cloned()
    }

    pub fn get_conversation(&self, conversation_id: i64) -> Option<Arc<RunHandle>> {
        self.0.lock().conversations.get(&conversation_id).cloned()
    }

    pub fn register_message(&self, message_id: serenity::MessageId, handle: Arc<RunHandle>) {
        handle.record(message_id);
        self.0.lock().messages.insert(message_id, handle);
    }

    pub fn register_conversation(&self, conversation_id: i64, handle: Arc<RunHandle>) {
        self.0.lock().conversations.insert(conversation_id, handle);
    }

    pub fn remove(&self, handle: &Arc<RunHandle>) {
        let mut runs = self.0.lock();
        runs.messages.retain(|_, value| !Arc::ptr_eq(value, handle));
        runs.conversations
            .retain(|_, value| !Arc::ptr_eq(value, handle));
    }

    pub fn cancel_all(&self) {
        let handles = self.0.lock().messages.values().cloned().collect::<Vec<_>>();
        handles.iter().for_each(|handle| handle.cancel.cancel());
    }
}
