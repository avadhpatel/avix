use avix_core::error::AvixError;
use avix_core::types::*;

#[test]
fn pid_zero_is_kernel() {
    let pid = Pid::new(0);
    assert!(pid.is_kernel());
}

#[test]
fn pid_nonzero_is_not_kernel() {
    assert!(!Pid::new(57).is_kernel());
}

#[test]
fn pid_display() {
    assert_eq!(Pid::new(42).to_string(), "42");
}

#[test]
fn pid_ordering() {
    assert!(Pid::new(1) < Pid::new(2));
}

#[test]
fn ipc_addr_kernel_unix() {
    let addr = IpcAddr::from_name("kernel");
    #[cfg(unix)]
    assert_eq!(addr.os_path(), "/run/avix/kernel.sock");
}

#[test]
fn ipc_addr_agent_unix() {
    let addr = IpcAddr::for_agent(Pid::new(57));
    #[cfg(unix)]
    assert_eq!(addr.os_path(), "/run/avix/agents/57.sock");
}

#[test]
fn ipc_addr_service_unix() {
    let addr = IpcAddr::for_service("github-svc");
    #[cfg(unix)]
    assert_eq!(addr.os_path(), "/run/avix/services/github-svc.sock");
}

#[test]
fn ipc_addr_router_unix() {
    let addr = IpcAddr::router();
    #[cfg(unix)]
    assert_eq!(addr.os_path(), "/run/avix/router.sock");
}

#[test]
fn role_ordering_admin_highest() {
    assert!(Role::Admin > Role::Operator);
    assert!(Role::Operator > Role::User);
    assert!(Role::User > Role::Guest);
}

#[test]
fn role_can_access_proc_domain() {
    assert!(Role::Guest.can_access_domain("proc"));
    assert!(Role::User.can_access_domain("proc"));
}

#[test]
fn role_sys_requires_admin() {
    assert!(!Role::User.can_access_domain("sys"));
    assert!(!Role::Operator.can_access_domain("sys"));
    assert!(Role::Admin.can_access_domain("sys"));
}

#[test]
fn role_cap_requires_admin() {
    assert!(!Role::Operator.can_access_domain("cap"));
    assert!(Role::Admin.can_access_domain("cap"));
}

#[test]
fn role_from_str() {
    assert_eq!("admin".parse::<Role>().unwrap(), Role::Admin);
    assert_eq!("operator".parse::<Role>().unwrap(), Role::Operator);
    assert_eq!("user".parse::<Role>().unwrap(), Role::User);
    assert_eq!("guest".parse::<Role>().unwrap(), Role::Guest);
    assert!("unknown".parse::<Role>().is_err());
}

#[test]
fn modality_as_str() {
    assert_eq!(Modality::Text.as_str(), "text");
    assert_eq!(Modality::Image.as_str(), "image");
    assert_eq!(Modality::Speech.as_str(), "speech");
    assert_eq!(Modality::Transcription.as_str(), "transcription");
    assert_eq!(Modality::Embedding.as_str(), "embedding");
}

#[test]
fn modality_from_str() {
    assert_eq!("text".parse::<Modality>().unwrap(), Modality::Text);
    assert_eq!("image".parse::<Modality>().unwrap(), Modality::Image);
    assert!("video".parse::<Modality>().is_err());
}

#[test]
fn modality_round_trip() {
    for m in Modality::all() {
        assert_eq!(m.as_str().parse::<Modality>().unwrap(), *m);
    }
}

#[test]
fn tool_name_valid() {
    assert!(ToolName::parse("fs/read").is_ok());
    assert!(ToolName::parse("mcp/github/list-prs").is_ok());
    assert!(ToolName::parse("llm/generate-image").is_ok());
}

#[test]
fn tool_name_rejects_double_underscore() {
    let err = ToolName::parse("bad__name").unwrap_err();
    assert!(matches!(err, AvixError::InvalidToolName { .. }));
}

#[test]
fn tool_name_rejects_empty() {
    assert!(ToolName::parse("").is_err());
}

#[test]
fn tool_name_mangle() {
    assert_eq!(ToolName::parse("fs/write").unwrap().mangled(), "fs__write");
    assert_eq!(
        ToolName::parse("mcp/github/list-prs").unwrap().mangled(),
        "mcp__github__list-prs"
    );
}

#[test]
fn tool_name_unmangle() {
    assert_eq!(
        ToolName::unmangle("mcp__github__list-prs")
            .unwrap()
            .as_str(),
        "mcp/github/list-prs"
    );
}

#[test]
fn tool_name_mangle_round_trip() {
    let original = "llm/generate-image";
    let mangled = ToolName::parse(original).unwrap().mangled();
    let back = ToolName::unmangle(&mangled).unwrap();
    assert_eq!(back.as_str(), original);
}

#[test]
fn tool_state_available_can_transition() {
    assert!(ToolState::Available.can_transition_to(&ToolState::Degraded));
    assert!(ToolState::Available.can_transition_to(&ToolState::Unavailable));
}

#[test]
fn tool_state_unavailable_can_recover() {
    assert!(ToolState::Unavailable.can_transition_to(&ToolState::Available));
    assert!(ToolState::Unavailable.can_transition_to(&ToolState::Degraded));
}

#[test]
fn tool_category_direct() {
    assert_eq!(ToolCategory::classify("fs/read"), ToolCategory::Direct);
    assert_eq!(ToolCategory::classify("llm/complete"), ToolCategory::Direct);
    assert_eq!(ToolCategory::classify("exec/python"), ToolCategory::Direct);
}

#[test]
fn tool_category_avix_behaviour() {
    assert_eq!(
        ToolCategory::classify("agent/spawn"),
        ToolCategory::AvixBehaviour
    );
    assert_eq!(
        ToolCategory::classify("pipe/open"),
        ToolCategory::AvixBehaviour
    );
    assert_eq!(
        ToolCategory::classify("cap/request-tool"),
        ToolCategory::AvixBehaviour
    );
    assert_eq!(
        ToolCategory::classify("cap/escalate"),
        ToolCategory::AvixBehaviour
    );
    assert_eq!(
        ToolCategory::classify("job/watch"),
        ToolCategory::AvixBehaviour
    );
}

#[test]
fn capability_map_spawn_grants_agent_tools() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("agent:spawn");
    assert!(tools.contains(&"agent/spawn"));
    assert!(tools.contains(&"agent/kill"));
    assert!(tools.contains(&"agent/list"));
    assert!(tools.contains(&"agent/wait"));
    assert!(tools.contains(&"agent/send-message"));
}

#[test]
fn capability_map_pipe_grants_pipe_tools() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("pipe:use");
    assert!(tools.contains(&"pipe/open"));
    assert!(tools.contains(&"pipe/write"));
    assert!(tools.contains(&"pipe/read"));
    assert!(tools.contains(&"pipe/close"));
}

#[test]
fn capability_map_always_present_tools() {
    let map = CapabilityToolMap::default();
    let always = map.always_present();
    assert!(always.contains(&"cap/request-tool"));
    assert!(always.contains(&"cap/escalate"));
    assert!(always.contains(&"cap/list"));
    assert!(always.contains(&"job/watch"));
}

#[test]
fn capability_map_llm_inference_grants_complete() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("llm:inference");
    assert!(tools.contains(&"llm/complete"));
}

#[test]
fn capability_map_llm_image_grants_generate_image() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("llm:image");
    assert!(tools.contains(&"llm/generate-image"));
}

#[test]
fn capability_map_unknown_capability_returns_empty() {
    let map = CapabilityToolMap::default();
    let tools = map.tools_for_capability("not:a:real:cap");
    assert!(tools.is_empty());
}
