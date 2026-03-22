pub fn build_system_prompt(
    pid: u32,
    agent_name: &str,
    goal: &str,
    pending_messages: &[String],
) -> String {
    let mut prompt = format!("# Agent Identity\nName: {agent_name}\nPID: {pid}\nGoal: {goal}\n");
    if !pending_messages.is_empty() {
        prompt.push_str("\n# Pending Messages\n");
        for msg in pending_messages {
            prompt.push_str(msg);
            prompt.push('\n');
        }
    }
    prompt
}
