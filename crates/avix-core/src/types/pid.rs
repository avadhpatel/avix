use std::fmt;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct Pid(u32);

impl Pid {
    pub fn new(n: u32) -> Self {
        Self(n)
    }
    pub fn is_kernel(&self) -> bool {
        self.0 == 0
    }
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
