use std::sync::Arc;
use uuid::Uuid;
use tracing::instrument;

use crate::error::AvixError;
use crate::history::record::{MessageRecord, PartRecord};
use crate::history::HistoryStore;

pub struct HistoryManager {
    store: Option<Arc<HistoryStore>>,
}

impl HistoryManager {
    pub fn new(store: Option<Arc<HistoryStore>>) -> Self {
        Self { store }
    }

    pub fn with_store(mut self, store: Arc<HistoryStore>) -> Self {
        self.store = Some(store);
        self
    }

    #[instrument(skip(self))]
    pub async fn create_message(&self, msg: &MessageRecord) -> Result<(), AvixError> {
        match &self.store {
            Some(s) => s.create_message(msg).await,
            None => Err(AvixError::NotFound("history store not configured".into())),
        }
    }

    pub async fn get_message(&self, id: &Uuid) -> Result<Option<MessageRecord>, AvixError> {
        match &self.store {
            Some(s) => s.get_message(id).await,
            None => Ok(None),
        }
    }

    #[instrument(skip(self))]
    pub async fn list_messages(&self, session_id: &Uuid) -> Result<Vec<MessageRecord>, AvixError> {
        match &self.store {
            Some(s) => s.list_messages(session_id).await,
            None => Ok(vec![]),
        }
    }

    #[instrument(skip(self))]
    pub async fn create_part(&self, part: &PartRecord) -> Result<(), AvixError> {
        match &self.store {
            Some(s) => s.create_part(part).await,
            None => Err(AvixError::NotFound("history store not configured".into())),
        }
    }

    #[instrument(skip(self))]
    pub async fn get_part(&self, id: &Uuid) -> Result<Option<PartRecord>, AvixError> {
        match &self.store {
            Some(s) => s.get_part(id).await,
            None => Ok(None),
        }
    }

    #[instrument(skip(self))]
    pub async fn list_parts(&self, message_id: &Uuid) -> Result<Vec<PartRecord>, AvixError> {
        match &self.store {
            Some(s) => s.list_parts(message_id).await,
            None => Ok(vec![]),
        }
    }
}