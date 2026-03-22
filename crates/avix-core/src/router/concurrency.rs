use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};

use crate::error::AvixError;
use crate::types::Pid;

pub struct ConcurrencyGuard {
    _permit: OwnedSemaphorePermit,
    counter: Arc<AtomicUsize>,
}

impl ConcurrencyGuard {
    pub fn is_valid(&self) -> bool {
        true
    }
}

impl Drop for ConcurrencyGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct ConcurrencyLimiter {
    semaphore: Arc<Semaphore>,
    active: Arc<AtomicUsize>,
}

impl ConcurrencyLimiter {
    pub fn new(max: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max)),
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub async fn acquire(&self) -> Result<ConcurrencyGuard, AvixError> {
        let permit = Arc::clone(&self.semaphore)
            .acquire_owned()
            .await
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.active.fetch_add(1, Ordering::Relaxed);
        Ok(ConcurrencyGuard {
            _permit: permit,
            counter: Arc::clone(&self.active),
        })
    }

    /// Non-blocking acquire. Returns `None` if the limit is already reached.
    pub fn try_acquire(&self) -> Option<ConcurrencyGuard> {
        Arc::clone(&self.semaphore)
            .try_acquire_owned()
            .ok()
            .map(|permit| {
                self.active.fetch_add(1, Ordering::Relaxed);
                ConcurrencyGuard {
                    _permit: permit,
                    counter: Arc::clone(&self.active),
                }
            })
    }

    pub async fn active_count(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
}

struct PerPidEntry {
    semaphore: Arc<Semaphore>,
    active: Arc<AtomicUsize>,
}

pub struct CallerScopedLimiter {
    max_per_pid: usize,
    limits: Arc<RwLock<HashMap<u32, PerPidEntry>>>,
}

impl CallerScopedLimiter {
    pub fn new(max_per_pid: usize) -> Self {
        Self {
            max_per_pid,
            limits: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn acquire(&self, pid: Pid) -> Result<ConcurrencyGuard, AvixError> {
        let entry = {
            let mut map = self.limits.write().await;
            map.entry(pid.as_u32()).or_insert_with(|| PerPidEntry {
                semaphore: Arc::new(Semaphore::new(self.max_per_pid)),
                active: Arc::new(AtomicUsize::new(0)),
            });
            let e = map.get(&pid.as_u32()).unwrap();
            (Arc::clone(&e.semaphore), Arc::clone(&e.active))
        };
        let permit = entry
            .0
            .acquire_owned()
            .await
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        entry.1.fetch_add(1, Ordering::Relaxed);
        Ok(ConcurrencyGuard {
            _permit: permit,
            counter: entry.1,
        })
    }
}
