use super::state::ParsedCommand;

/// Parse a command input string starting with '/' into a ParsedCommand.
/// Returns an error string if parsing fails.
pub fn parse(input: &str) -> Result<ParsedCommand, String> {
    if !input.starts_with('/') {
        return Err("Command must start with '/'".to_string());
    }

    let trimmed = input[1..].trim();
    if trimmed.is_empty() {
        return Err("Empty command".to_string());
    }

    let parts = parse_quoted(trimmed);

    match parts.len() {
        1 => match parts[0].as_str() {
            "quit" | "q" => Ok(ParsedCommand::Quit),
            "connect" | "c" => Ok(ParsedCommand::Connect),
            "help" | "h" | "?" => Ok(ParsedCommand::Help),
            "logs" | "log" => Ok(ParsedCommand::ToggleLogs),
            "notifs" | "notifications" | "n" => Ok(ParsedCommand::ToggleNotifications),
            "new-agent-form" | "new" | "f" => Ok(ParsedCommand::ToggleNewAgentForm),
            "catalog" => Ok(ParsedCommand::Catalog),
            _ => Err(format!("Unknown command: {}", trimmed)),
        },
        2 => match parts[0].as_str() {
            "kill" => match parts[1].parse::<u64>() {
                Ok(pid) => Ok(ParsedCommand::Kill { pid }),
                Err(_) => Err("kill requires a valid PID (number)".to_string()),
            },
            _ => Err(format!("Unknown command: {}", trimmed)),
        },
        3 => match parts[0].as_str() {
            "spawn" => {
                let name = &parts[1];
                let goal = &parts[2];
                if name.is_empty() || goal.is_empty() {
                    Err("spawn requires name and goal".to_string())
                } else {
                    Ok(ParsedCommand::Spawn {
                        name: name.clone(),
                        goal: goal.clone(),
                    })
                }
            }
            _ => Err(format!("Unknown command: {}", trimmed)),
        },
        _ => Err(format!("Unknown command: {}", trimmed)),
    }
}

/// Parse a string into parts, respecting quoted strings.
/// Supports double quotes for spaces in arguments.
fn parse_quoted(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in input.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
            }
            ' ' if !in_quotes => {
                if !current.is_empty() {
                    parts.push(current);
                    current = String::new();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }

    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_quit() {
        assert_eq!(parse("/quit"), Ok(ParsedCommand::Quit));
        assert_eq!(parse("/q"), Ok(ParsedCommand::Quit));
    }

    #[test]
    fn parse_connect() {
        assert_eq!(parse("/connect"), Ok(ParsedCommand::Connect));
        assert_eq!(parse("/c"), Ok(ParsedCommand::Connect));
    }

    #[test]
    fn parse_help() {
        assert_eq!(parse("/help"), Ok(ParsedCommand::Help));
        assert_eq!(parse("/h"), Ok(ParsedCommand::Help));
        assert_eq!(parse("/?"), Ok(ParsedCommand::Help));
    }

    #[test]
    fn parse_toggle_logs() {
        assert_eq!(parse("/logs"), Ok(ParsedCommand::ToggleLogs));
        assert_eq!(parse("/log"), Ok(ParsedCommand::ToggleLogs));
    }

    #[test]
    fn parse_toggle_notifs() {
        assert_eq!(parse("/notifs"), Ok(ParsedCommand::ToggleNotifications));
        assert_eq!(parse("/n"), Ok(ParsedCommand::ToggleNotifications));
    }

    #[test]
    fn parse_toggle_new_agent_form() {
        assert_eq!(
            parse("/new-agent-form"),
            Ok(ParsedCommand::ToggleNewAgentForm)
        );
        assert_eq!(parse("/f"), Ok(ParsedCommand::ToggleNewAgentForm));
    }

    #[test]
    fn parse_spawn() {
        assert_eq!(
            parse("/spawn foo \"analyze logs\""),
            Ok(ParsedCommand::Spawn {
                name: "foo".to_string(),
                goal: "analyze logs".to_string(),
            })
        );
        assert_eq!(
            parse("/spawn bar test"),
            Ok(ParsedCommand::Spawn {
                name: "bar".to_string(),
                goal: "test".to_string(),
            })
        );
    }

    #[test]
    fn parse_kill() {
        assert_eq!(parse("/kill 123"), Ok(ParsedCommand::Kill { pid: 123 }));
    }

    #[test]
    fn parse_invalid() {
        assert!(parse("quit").is_err());
        assert!(parse("/").is_err());
        assert!(parse("/unknown").is_err());
        assert!(parse("/spawn foo").is_err());
        assert!(parse("/kill abc").is_err());
    }

    #[test]
    fn test_parse_quoted() {
        assert_eq!(parse_quoted("a b c"), vec!["a", "b", "c"]);
        assert_eq!(parse_quoted("a \"b c\" d"), vec!["a", "b c", "d"]);
        assert_eq!(parse_quoted("\"hello world\""), vec!["hello world"]);
        assert_eq!(
            parse_quoted("spawn foo \"goal with spaces\""),
            vec!["spawn", "foo", "goal with spaces"]
        );
    }
}
