use std::fmt;

use rand::Rng;

/// Epoch offset for time-seeded PID generation: 2025-01-01T00:00:00Z in milliseconds.
const PID_EPOCH_MS: u64 = 1_735_689_600_000;

/// A globally unique process identifier.
///
/// Format: upper 42 bits = milliseconds since 2025-01-01 UTC (good for ~139 years),
/// lower 22 bits = cryptographically random salt (~4M distinct values per millisecond).
/// `Pid(0)` is reserved for the kernel pseudo-process.
///
/// PIDs are unique across kernel restarts — no counter file or coordination needed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Pid(u64);

impl Pid {
    /// Generate a new globally-unique PID. Call once per agent spawn.
    pub fn generate() -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let ts = now_ms.saturating_sub(PID_EPOCH_MS) & 0x3FF_FFFF_FFFF; // 42 bits
        let salt: u64 = rand::thread_rng().gen::<u32>() as u64 & 0x3FFFFF; // 22 bits
        Self((ts << 22) | salt)
    }

    /// The kernel pseudo-process sentinel (`Pid(0)`).
    pub fn kernel() -> Self {
        Self(0)
    }

    /// Construct from a raw u64 (use for tests and deserialization only).
    pub fn from_u64(n: u64) -> Self {
        Self(n)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn is_kernel(&self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_kernel() {
            write!(f, "kernel")
        } else {
            write!(f, "{}", self.0)
        }
    }
}
