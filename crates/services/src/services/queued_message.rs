use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use db::models::scratch::DraftFollowUpData;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedMutexGuard};
use ts_rs::TS;
use uuid::Uuid;

/// Represents a queued follow-up message for a session
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct QueuedMessage {
    /// The session this message is queued for
    pub session_id: Uuid,
    /// The follow-up data (message + variant)
    pub data: DraftFollowUpData,
    /// Timestamp when the message was queued
    pub queued_at: DateTime<Utc>,
}

/// Status of the queue for a session (for frontend display)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QueueStatus {
    /// No message queued
    Empty,
    /// Message is queued and waiting for execution to complete
    Queued { message: QueuedMessage },
}

/// In-memory service for managing queued follow-up messages.
/// One queued message per session.
#[derive(Clone)]
pub struct QueuedMessageService {
    queue: Arc<DashMap<Uuid, QueuedMessage>>,
    session_operation_locks: Arc<Mutex<HashMap<Uuid, Weak<Mutex<()>>>>>,
}

impl QueuedMessageService {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(DashMap::new()),
            session_operation_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Serialize queue insertion, consumption/launch, and deletion for this
    /// Session. Acquire before any database transaction; never while holding a
    /// run lock. Callers keep the guard until their database work is complete.
    pub async fn lock_session(&self, session_id: Uuid) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.session_operation_locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = locks.get(&session_id).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(session_id, Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }

    /// Queue a message for a session. Replaces any existing queued message.
    /// Hold `lock_session` and revalidate Session existence before insertion.
    pub fn queue_message(&self, session_id: Uuid, data: DraftFollowUpData) -> QueuedMessage {
        let queued = QueuedMessage {
            session_id,
            data,
            queued_at: Utc::now(),
        };
        self.queue.insert(session_id, queued.clone());
        queued
    }

    /// Cancel/remove a queued message for a session
    pub fn cancel_queued(&self, session_id: Uuid) -> Option<QueuedMessage> {
        self.queue.remove(&session_id).map(|(_, v)| v)
    }

    /// Get the queued message for a session (if any)
    pub fn get_queued(&self, session_id: Uuid) -> Option<QueuedMessage> {
        self.queue.get(&session_id).map(|r| r.clone())
    }

    /// Take (remove and return) the queued message for a session.
    /// Used by finalization flow to consume the queued message.
    /// Hold `lock_session` until the follow-up has a durable run reservation.
    pub fn take_queued(&self, session_id: Uuid) -> Option<QueuedMessage> {
        self.queue.remove(&session_id).map(|(_, v)| v)
    }

    /// Check if a session has a queued message
    pub fn has_queued(&self, session_id: Uuid) -> bool {
        self.queue.contains_key(&session_id)
    }

    pub fn queued_session_ids(&self) -> Vec<Uuid> {
        self.queue.iter().map(|item| *item.key()).collect()
    }

    /// Get queue status for frontend display
    pub fn get_status(&self, session_id: Uuid) -> QueueStatus {
        match self.get_queued(session_id) {
            Some(msg) => QueueStatus::Queued { message: msg },
            None => QueueStatus::Empty,
        }
    }
}

impl Default for QueuedMessageService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::task::Poll;

    use super::*;

    #[tokio::test]
    async fn queue_and_deletion_share_a_lock_only_for_the_same_session() {
        let queue = QueuedMessageService::new();
        let clone = queue.clone();
        let session_id = Uuid::new_v4();
        let guard = queue.lock_session(session_id).await;
        let mut same_session = Box::pin(clone.lock_session(session_id));
        assert!(matches!(futures::poll!(&mut same_session), Poll::Pending));
        let mut other_session = Box::pin(clone.lock_session(Uuid::new_v4()));
        assert!(matches!(futures::poll!(&mut other_session), Poll::Ready(_)));
        drop(guard);
        assert!(matches!(futures::poll!(&mut same_session), Poll::Ready(_)));
    }
}
