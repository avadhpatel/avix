use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::channel::{Pipe, PipeError};

pub struct PipeRegistry {
    pipes: RwLock<HashMap<String, Arc<Pipe>>>,
}

impl PipeRegistry {
    pub fn new() -> Self {
        Self {
            pipes: RwLock::new(HashMap::new()),
        }
    }

    pub async fn open(&self, owner_pid: u64, capacity: usize) -> String {
        let pipe = Arc::new(Pipe::new(owner_pid, capacity));
        let id = pipe.id.clone();
        self.pipes.write().await.insert(id.clone(), pipe);
        id
    }

    pub async fn write(&self, id: &str, msg: String) -> Result<(), PipeError> {
        let pipes = self.pipes.read().await;
        let pipe = pipes
            .get(id)
            .ok_or_else(|| PipeError::Closed(id.to_string()))?;
        pipe.write(msg).await
    }

    pub async fn read(&self, id: &str) -> Option<String> {
        let pipes = self.pipes.read().await;
        let pipe = pipes.get(id)?;
        pipe.read().await
    }

    pub async fn close(&self, id: &str) {
        let mut pipes = self.pipes.write().await;
        if let Some(pipe) = pipes.get(id) {
            pipe.close();
        }
        pipes.remove(id);
    }

    pub async fn pipe_count(&self) -> usize {
        self.pipes.read().await.len()
    }
}

impl Default for PipeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_open_returns_unique_ids() {
        let reg = PipeRegistry::new();
        let id1 = reg.open(1, 10).await;
        let id2 = reg.open(1, 10).await;
        assert_ne!(id1, id2);
    }

    #[tokio::test]
    async fn test_write_read_fifo() {
        let reg = PipeRegistry::new();
        let id = reg.open(1, 10).await;
        reg.write(&id, "hello".into()).await.unwrap();
        reg.write(&id, "world".into()).await.unwrap();
        assert_eq!(reg.read(&id).await.unwrap(), "hello");
        assert_eq!(reg.read(&id).await.unwrap(), "world");
    }

    #[tokio::test]
    async fn test_backpressure() {
        let reg = PipeRegistry::new();
        let id = reg.open(1, 1).await; // capacity 1
        reg.write(&id, "msg1".into()).await.unwrap();
        // second write should fail with Full
        let res = reg.write(&id, "msg2".into()).await;
        assert!(matches!(res, Err(PipeError::Full(_))));
    }

    #[tokio::test]
    async fn test_closed_pipe_write_returns_sigpipe() {
        let reg = PipeRegistry::new();
        let id = reg.open(1, 10).await;
        reg.close(&id).await;
        let res = reg.write(&id, "msg".into()).await;
        assert!(matches!(res, Err(PipeError::Closed(_))));
    }

    #[tokio::test]
    async fn test_close_removes_from_registry() {
        let reg = PipeRegistry::new();
        let id = reg.open(1, 10).await;
        assert_eq!(reg.pipe_count().await, 1);
        reg.close(&id).await;
        assert_eq!(reg.pipe_count().await, 0);
    }

    #[tokio::test]
    async fn test_multiple_pipes_independent() {
        let reg = PipeRegistry::new();
        let id1 = reg.open(1, 10).await;
        let id2 = reg.open(2, 10).await;
        reg.write(&id1, "from-pipe-1".into()).await.unwrap();
        reg.write(&id2, "from-pipe-2".into()).await.unwrap();
        assert_eq!(reg.read(&id1).await.unwrap(), "from-pipe-1");
        assert_eq!(reg.read(&id2).await.unwrap(), "from-pipe-2");
    }

    #[tokio::test]
    async fn test_read_nonexistent_returns_none() {
        let reg = PipeRegistry::new();
        let result = reg.read("nonexistent-id").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_write_nonexistent_returns_closed_error() {
        let reg = PipeRegistry::new();
        let res = reg.write("nonexistent-id", "msg".into()).await;
        assert!(matches!(res, Err(PipeError::Closed(_))));
    }

    #[tokio::test]
    async fn test_pipe_count_increases() {
        let reg = PipeRegistry::new();
        assert_eq!(reg.pipe_count().await, 0);
        reg.open(1, 10).await;
        assert_eq!(reg.pipe_count().await, 1);
        reg.open(2, 10).await;
        assert_eq!(reg.pipe_count().await, 2);
    }

    #[tokio::test]
    async fn test_multiple_messages_ordering() {
        let reg = PipeRegistry::new();
        let id = reg.open(1, 100).await;
        for i in 0..10 {
            reg.write(&id, format!("msg-{i}")).await.unwrap();
        }
        for i in 0..10 {
            let msg = reg.read(&id).await.unwrap();
            assert_eq!(msg, format!("msg-{i}"));
        }
    }

    #[tokio::test]
    async fn test_close_double_is_noop() {
        let reg = PipeRegistry::new();
        let id = reg.open(1, 10).await;
        reg.close(&id).await;
        // Second close should not panic
        reg.close(&id).await;
        assert_eq!(reg.pipe_count().await, 0);
    }
}
