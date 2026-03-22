use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

#[derive(Default)]
pub struct RefreshScheduler {
    handles: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
}

impl RefreshScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn schedule<F>(&self, name: &str, interval: Duration, callback: F)
    where
        F: Fn() + Send + 'static,
    {
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await; // skip first immediate tick
            ticker.tick().await; // wait for first real tick
            callback();
        });
        self.handles.lock().await.insert(name.to_string(), handle);
    }
}
