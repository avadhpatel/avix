#[derive(Debug, Clone)]
pub struct ToolChangedEvent {
    pub op: String,
    pub tools: Vec<String>,
}
