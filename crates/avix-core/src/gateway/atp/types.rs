use serde::{Deserialize, Serialize};

/// The 11 command domains defined in ATP §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AtpDomain {
    Auth,
    Proc,
    Signal,
    Fs,
    Snap,
    Cron,
    Users,
    Crews,
    Cap,
    Sys,
    Pipe,
    Session,
}

/// All 16 server-push event kinds defined in ATP §7.
/// Serialise as dot-separated lowercase strings, e.g. `"agent.output"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AtpEventKind {
    #[serde(rename = "session.ready")]
    SessionReady,
    #[serde(rename = "session.closing")]
    SessionClosing,
    #[serde(rename = "token.expiring")]
    TokenExpiring,
    #[serde(rename = "agent.output")]
    AgentOutput,
    #[serde(rename = "agent.status")]
    AgentStatus,
    #[serde(rename = "agent.tool_call")]
    AgentToolCall,
    #[serde(rename = "agent.tool_result")]
    AgentToolResult,
    #[serde(rename = "agent.exit")]
    AgentExit,
    #[serde(rename = "proc.start")]
    ProcStart,
    #[serde(rename = "proc.output")]
    ProcOutput,
    #[serde(rename = "proc.exit")]
    ProcExit,
    #[serde(rename = "proc.signal")]
    ProcSignal,
    #[serde(rename = "hil.request")]
    HilRequest,
    #[serde(rename = "hil.resolved")]
    HilResolved,
    #[serde(rename = "fs.changed")]
    FsChanged,
    #[serde(rename = "tool.changed")]
    ToolChanged,
    #[serde(rename = "cron.fired")]
    CronFired,
    #[serde(rename = "sys.service")]
    SysService,
    #[serde(rename = "sys.alert")]
    SysAlert,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_domains_round_trip() {
        for domain in [
            AtpDomain::Auth,
            AtpDomain::Proc,
            AtpDomain::Signal,
            AtpDomain::Fs,
            AtpDomain::Snap,
            AtpDomain::Cron,
            AtpDomain::Users,
            AtpDomain::Crews,
            AtpDomain::Cap,
            AtpDomain::Sys,
            AtpDomain::Pipe,
        ] {
            let s = serde_json::to_string(&domain).unwrap();
            let back: AtpDomain = serde_json::from_str(&s).unwrap();
            assert_eq!(back, domain, "domain {domain:?} failed round-trip");
        }
    }

    #[test]
    fn domains_serialize_lowercase() {
        assert_eq!(serde_json::to_string(&AtpDomain::Auth).unwrap(), "\"auth\"");
        assert_eq!(serde_json::to_string(&AtpDomain::Proc).unwrap(), "\"proc\"");
        assert_eq!(
            serde_json::to_string(&AtpDomain::Signal).unwrap(),
            "\"signal\""
        );
        assert_eq!(serde_json::to_string(&AtpDomain::Fs).unwrap(), "\"fs\"");
        assert_eq!(serde_json::to_string(&AtpDomain::Snap).unwrap(), "\"snap\"");
        assert_eq!(serde_json::to_string(&AtpDomain::Cron).unwrap(), "\"cron\"");
        assert_eq!(
            serde_json::to_string(&AtpDomain::Users).unwrap(),
            "\"users\""
        );
        assert_eq!(
            serde_json::to_string(&AtpDomain::Crews).unwrap(),
            "\"crews\""
        );
        assert_eq!(serde_json::to_string(&AtpDomain::Cap).unwrap(), "\"cap\"");
        assert_eq!(serde_json::to_string(&AtpDomain::Sys).unwrap(), "\"sys\"");
        assert_eq!(serde_json::to_string(&AtpDomain::Pipe).unwrap(), "\"pipe\"");
    }

    #[test]
    fn event_kinds_serialize_dot_notation() {
        assert_eq!(
            serde_json::to_string(&AtpEventKind::SessionReady).unwrap(),
            "\"session.ready\""
        );
        assert_eq!(
            serde_json::to_string(&AtpEventKind::AgentOutput).unwrap(),
            "\"agent.output\""
        );
        assert_eq!(
            serde_json::to_string(&AtpEventKind::AgentToolCall).unwrap(),
            "\"agent.tool_call\""
        );
        assert_eq!(
            serde_json::to_string(&AtpEventKind::HilRequest).unwrap(),
            "\"hil.request\""
        );
        assert_eq!(
            serde_json::to_string(&AtpEventKind::FsChanged).unwrap(),
            "\"fs.changed\""
        );
        assert_eq!(
            serde_json::to_string(&AtpEventKind::SysService).unwrap(),
            "\"sys.service\""
        );
    }

    #[test]
    fn all_event_kinds_round_trip() {
        for kind in [
            AtpEventKind::SessionReady,
            AtpEventKind::SessionClosing,
            AtpEventKind::TokenExpiring,
            AtpEventKind::AgentOutput,
            AtpEventKind::AgentStatus,
            AtpEventKind::AgentToolCall,
            AtpEventKind::AgentToolResult,
            AtpEventKind::AgentExit,
            AtpEventKind::ProcSignal,
            AtpEventKind::HilRequest,
            AtpEventKind::HilResolved,
            AtpEventKind::FsChanged,
            AtpEventKind::ToolChanged,
            AtpEventKind::CronFired,
            AtpEventKind::SysService,
            AtpEventKind::SysAlert,
        ] {
            let s = serde_json::to_string(&kind).unwrap();
            let back: AtpEventKind = serde_json::from_str(&s).unwrap();
            assert_eq!(back, kind, "event kind {kind:?} failed round-trip");
        }
    }
}
