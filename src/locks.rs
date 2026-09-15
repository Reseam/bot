use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use tokio::sync::OwnedMutexGuard;

pub struct KeyedLocks<K>(Mutex<HashMap<K, Weak<tokio::sync::Mutex<()>>>>);

impl<K> Default for KeyedLocks<K> {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

impl<K: Eq + Hash + Clone> KeyedLocks<K> {
    pub fn try_lock(&self, key: &K) -> Option<OwnedMutexGuard<()>> {
        self.handle(key).try_lock_owned().ok()
    }

    pub async fn lock(&self, key: &K) -> OwnedMutexGuard<()> {
        self.handle(key).lock_owned().await
    }

    fn handle(&self, key: &K) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.0.lock();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(key.clone(), Arc::downgrade(&lock));
        lock
    }
}
