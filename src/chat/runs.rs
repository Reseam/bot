use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use poise::serenity_prelude as serenity;
use tokio::sync::OwnedMutexGuard;

use super::Run;
use crate::locks::KeyedLocks;

#[derive(Default)]
struct ActiveRuns {
    messages: HashMap<serenity::MessageId, Arc<Run>>,
    conversations: HashMap<i64, Arc<Run>>,
}

#[derive(Default)]
pub struct Runs {
    active: Mutex<ActiveRuns>,
    locks: KeyedLocks<i64>,
}

impl Runs {
    pub fn get(&self, message_id: serenity::MessageId) -> Option<Arc<Run>> {
        self.active.lock().messages.get(&message_id).cloned()
    }

    pub fn get_conversation(&self, conversation_id: i64) -> Option<Arc<Run>> {
        self.active
            .lock()
            .conversations
            .get(&conversation_id)
            .cloned()
    }

    pub fn start(&self, run: &Arc<Run>) {
        self.active
            .lock()
            .conversations
            .insert(run.conversation_id, run.clone());
    }

    pub fn register_message(&self, message_id: serenity::MessageId, run: &Arc<Run>) {
        run.message_ids.lock().insert(message_id);
        self.active.lock().messages.insert(message_id, run.clone());
    }

    pub fn remove(&self, run: &Arc<Run>) {
        let mut active = self.active.lock();
        active.messages.retain(|_, value| !Arc::ptr_eq(value, run));
        active
            .conversations
            .retain(|_, value| !Arc::ptr_eq(value, run));
    }

    pub fn cancel_all(&self) {
        let runs = self
            .active
            .lock()
            .conversations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        runs.iter().for_each(|run| run.cancel.cancel());
    }

    pub fn try_lock(&self, conversation_id: i64) -> Option<OwnedMutexGuard<()>> {
        self.locks.try_lock(&conversation_id)
    }

    pub async fn lock(&self, conversation_id: i64) -> OwnedMutexGuard<()> {
        self.locks.lock(&conversation_id).await
    }
}
