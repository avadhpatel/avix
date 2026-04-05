use serde_json::Value;

use crate::syscall::{
    domain::{cap_, fs_, pkg_, proc_, sched_, snap_, sys_},
    SyscallContext, SyscallError, SyscallResult,
};

pub struct SyscallHandler;

impl SyscallHandler {
    pub fn dispatch(&self, ctx: &SyscallContext, method: &str, params: Value) -> SyscallResult {
        // Check capability
        if !ctx.token.has_tool(method) {
            return Err(SyscallError::Eperm(ctx.caller_pid, method.to_string()));
        }
        match method {
            "kernel/proc/spawn" => proc_::spawn(ctx, params),
            "kernel/proc/kill" => proc_::kill(ctx, params),
            "kernel/proc/list" => proc_::list(ctx, params),
            "kernel/proc/info" => proc_::info(ctx, params),
            "kernel/proc/wait" => proc_::wait(ctx, params),
            "kernel/proc/signal" => proc_::signal(ctx, params),
            "kernel/fs/read" => fs_::read(ctx, params),
            "kernel/fs/write" => fs_::write(ctx, params),
            "kernel/fs/list" => fs_::list(ctx, params),
            "kernel/fs/exists" => fs_::exists(ctx, params),
            "kernel/fs/delete" => fs_::delete(ctx, params),
            "kernel/fs/watch" => fs_::watch(ctx, params),
            "kernel/cap/issue" => cap_::issue(ctx, params),
            "kernel/cap/validate" => cap_::validate(ctx, params),
            "kernel/cap/revoke" => cap_::revoke(ctx, params),
            "kernel/cap/policy" => cap_::policy(ctx, params),
            "kernel/sys/info" => sys_::info(ctx, params),
            "kernel/sys/boot-log" => sys_::boot_log(ctx, params),
            "kernel/sys/reboot" => sys_::reboot(ctx, params),
            "kernel/sched/cron-add" => sched_::cron_add(ctx, params),
            "kernel/sched/cron-remove" => sched_::cron_remove(ctx, params),
            "kernel/sched/cron-list" => sched_::cron_list(ctx, params),
            "kernel/snap/save" => snap_::save(ctx, params),
            "kernel/snap/restore" => snap_::restore(ctx, params),
            "kernel/snap/list" => snap_::list(ctx, params),
            "kernel/snap/delete" => snap_::delete(ctx, params),
            "proc/package/install-agent" => {
                pkg_::install_agent_sync(ctx, params, std::path::Path::new("/tmp"))
            }
            "proc/package/install-service" => {
                pkg_::install_service_sync(ctx, params, std::path::Path::new("/tmp"))
            }
            "proc/package/uninstall-agent" => {
                pkg_::uninstall_agent(ctx, params, std::path::Path::new("/tmp"))
            }
            "proc/package/uninstall-service" => {
                pkg_::uninstall_service(ctx, params, std::path::Path::new("/tmp"))
            }
            "proc/package/trust-add" => pkg_::trust_add(ctx, params, std::path::Path::new("/tmp")),
            "proc/package/trust-list" => {
                pkg_::trust_list(ctx, params, std::path::Path::new("/tmp"))
            }
            "proc/package/trust-remove" => {
                pkg_::trust_remove(ctx, params, std::path::Path::new("/tmp"))
            }
            _ => Err(SyscallError::Einval(format!("unknown syscall: {method}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::token::CapabilityToken;
    use serde_json::json;

    fn make_ctx(tools: &[&str]) -> SyscallContext {
        SyscallContext {
            caller_pid: 42,
            token: CapabilityToken::test_token(tools),
        }
    }

    #[test]
    fn test_proc_spawn_ok() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/proc/spawn"]);
        let res = handler.dispatch(&ctx, "kernel/proc/spawn", json!({"name": "test-agent"}));
        assert!(res.is_ok());
        let v = res.unwrap();
        assert!(v.get("pid").is_some());
    }

    #[test]
    fn test_eperm_no_token() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&[]); // no tools granted
        let res = handler.dispatch(&ctx, "kernel/proc/spawn", json!({"name": "x"}));
        assert!(matches!(res, Err(SyscallError::Eperm(_, _))));
    }

    #[test]
    fn test_proc_kill() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/proc/kill"]);
        let res = handler.dispatch(&ctx, "kernel/proc/kill", json!({"pid": 99}));
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["killed"], 99);
    }

    #[test]
    fn test_proc_kill_missing_pid() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/proc/kill"]);
        let res = handler.dispatch(&ctx, "kernel/proc/kill", json!({}));
        assert!(matches!(res, Err(SyscallError::Einval(_))));
    }

    #[test]
    fn test_proc_list() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/proc/list"]);
        let res = handler.dispatch(&ctx, "kernel/proc/list", json!({}));
        assert!(res.is_ok());
        assert!(res.unwrap()["processes"].is_array());
    }

    #[test]
    fn test_proc_info() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/proc/info"]);
        let res = handler.dispatch(&ctx, "kernel/proc/info", json!({"pid": 5}));
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["pid"], 5);
    }

    #[test]
    fn test_proc_wait() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/proc/wait"]);
        let res = handler.dispatch(&ctx, "kernel/proc/wait", json!({"pid": 7}));
        assert!(res.is_ok());
        let v = res.unwrap();
        assert_eq!(v["pid"], 7);
        assert_eq!(v["exit_code"], 0);
    }

    #[test]
    fn test_proc_signal() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/proc/signal"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/proc/signal",
            json!({"pid": 3, "signal": "SIGKILL"}),
        );
        assert!(res.is_ok());
        let v = res.unwrap();
        assert_eq!(v["delivered"], true);
        assert_eq!(v["signal"], "SIGKILL");
    }

    #[test]
    fn test_fs_read() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/fs/read"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/fs/read",
            json!({"path": "/users/alice/data.txt"}),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_fs_read_secrets_eperm() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/fs/read"]);
        let res = handler.dispatch(&ctx, "kernel/fs/read", json!({"path": "/secrets/ns/key"}));
        assert!(matches!(res, Err(SyscallError::Eperm(_, _))));
    }

    #[test]
    fn test_fs_write() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/fs/write"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/fs/write",
            json!({"path": "/users/alice/file.txt", "content": "hello"}),
        );
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["bytes_written"], 5);
    }

    #[test]
    fn test_fs_list() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/fs/list"]);
        let res = handler.dispatch(&ctx, "kernel/fs/list", json!({"path": "/users/alice/"}));
        assert!(res.is_ok());
        assert!(res.unwrap()["entries"].is_array());
    }

    #[test]
    fn test_fs_exists() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/fs/exists"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/fs/exists",
            json!({"path": "/users/alice/file.txt"}),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_fs_delete() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/fs/delete"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/fs/delete",
            json!({"path": "/users/alice/old.txt"}),
        );
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["deleted"], true);
    }

    #[test]
    fn test_fs_watch() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/fs/watch"]);
        let res = handler.dispatch(&ctx, "kernel/fs/watch", json!({"path": "/users/alice/"}));
        assert!(res.is_ok());
        assert!(res.unwrap()["watch_id"].is_string());
    }

    #[test]
    fn test_cap_issue() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/cap/issue"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/cap/issue",
            json!({"target_pid": 10, "tools": ["fs/read"]}),
        );
        assert!(res.is_ok());
        assert!(res.unwrap()["token_id"].is_string());
    }

    #[test]
    fn test_cap_validate() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/cap/validate"]);
        let res = handler.dispatch(&ctx, "kernel/cap/validate", json!({"token_id": "tok-abc"}));
        assert!(res.is_ok());
        let v = res.unwrap();
        assert_eq!(v["valid"], true);
    }

    #[test]
    fn test_cap_revoke() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/cap/revoke"]);
        let res = handler.dispatch(&ctx, "kernel/cap/revoke", json!({"token_id": "tok-abc"}));
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["revoked"], true);
    }

    #[test]
    fn test_cap_policy() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/cap/policy"]);
        let res = handler.dispatch(&ctx, "kernel/cap/policy", json!({"action": "allow"}));
        assert!(res.is_ok());
    }

    #[test]
    fn test_sys_info() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/sys/info"]);
        let res = handler.dispatch(&ctx, "kernel/sys/info", json!({}));
        assert!(res.is_ok());
        assert!(res.unwrap()["version"].is_string());
    }

    #[test]
    fn test_sys_boot_log() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/sys/boot-log"]);
        let res = handler.dispatch(&ctx, "kernel/sys/boot-log", json!({"limit": 10}));
        assert!(res.is_ok());
        assert!(res.unwrap()["lines"].is_array());
    }

    #[test]
    fn test_sys_reboot() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/sys/reboot"]);
        let res = handler.dispatch(&ctx, "kernel/sys/reboot", json!({"confirm": true}));
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["rebooting"], true);
    }

    #[test]
    fn test_sys_reboot_requires_confirm() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/sys/reboot"]);
        let res = handler.dispatch(&ctx, "kernel/sys/reboot", json!({"confirm": false}));
        assert!(matches!(res, Err(SyscallError::Einval(_))));
    }

    #[test]
    fn test_sched_cron_add() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/sched/cron-add"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/sched/cron-add",
            json!({"name": "cleanup", "expression": "0 * * * *"}),
        );
        assert!(res.is_ok());
        assert!(res.unwrap()["job_id"].is_string());
    }

    #[test]
    fn test_sched_cron_remove() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/sched/cron-remove"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/sched/cron-remove",
            json!({"job_id": "cron-1"}),
        );
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["removed"], true);
    }

    #[test]
    fn test_sched_cron_list() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/sched/cron-list"]);
        let res = handler.dispatch(&ctx, "kernel/sched/cron-list", json!({}));
        assert!(res.is_ok());
        assert!(res.unwrap()["jobs"].is_array());
    }

    #[test]
    fn test_snap_save() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/snap/save"]);
        let res = handler.dispatch(&ctx, "kernel/snap/save", json!({"pid": 42}));
        assert!(res.is_ok());
        assert!(res.unwrap()["snapshot_id"].is_string());
    }

    #[test]
    fn test_snap_restore() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/snap/restore"]);
        let res = handler.dispatch(
            &ctx,
            "kernel/snap/restore",
            json!({"snapshot_id": "snap-1"}),
        );
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["restored"], true);
    }

    #[test]
    fn test_snap_list() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/snap/list"]);
        let res = handler.dispatch(&ctx, "kernel/snap/list", json!({"pid": 42}));
        assert!(res.is_ok());
        assert!(res.unwrap()["snapshots"].is_array());
    }

    #[test]
    fn test_snap_delete() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/snap/delete"]);
        let res = handler.dispatch(&ctx, "kernel/snap/delete", json!({"snapshot_id": "snap-1"}));
        assert!(res.is_ok());
        assert_eq!(res.unwrap()["deleted"], true);
    }

    #[test]
    fn test_unknown_syscall() {
        let handler = SyscallHandler;
        let ctx = make_ctx(&["kernel/unknown/call"]);
        let res = handler.dispatch(&ctx, "kernel/unknown/call", json!({}));
        assert!(matches!(res, Err(SyscallError::Einval(_))));
    }
}
