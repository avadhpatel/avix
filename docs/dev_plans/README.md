# Development Plans

This folder is used for active development initiatives.
It may contains markdown files that will be used for dev reviews and coding
tools to help build the right solution.

Consider files in this folder temporary and can be deleted as per dev's needs.

---

## Active Plans

### Filesystem Gaps (VFS)

| File | Description |
|------|-------------|
| `fs-gap-A-bootstrap-vfs-init.md` | Phase 1 VFS skeleton (proc/, kernel/ trees) |
| `fs-gap-B-agent-spawn-vfs-writes.md` | /proc/<pid>/status.yaml + resolved.yaml at spawn |
| `fs-gap-C-config-init-completeness.md` | config init writes all 6 /etc/avix/ files |
| `fs-gap-D-vfs-write-protection.md` | VfsPath::is_agent_writable() + syscall enforcement |
| `fs-gap-E-mount-system.md` | Mount system design (deferred to v0.2) |
| `fs-gap-F-session-vfs-manifest.md` | SessionStore writes /proc/users/<u>/sessions/ |

### IPC Gaps

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `ipc-gap-A-transport-server.md` | Real IpcServer + IpcClient over Unix sockets | Critical | — |
| `ipc-gap-B-router-dispatch.md` | Tool call dispatch, capability enforcement, tool name mangling | Critical | Gap A |
| `ipc-gap-C-signal-delivery.md` | Signal delivery over IPC to agent sockets; agent receiver loop | High | Gap A |
| `ipc-gap-D-jobs-service.md` | Jobs service: job lifecycle, events, `job/watch` tool | High | — |
| `ipc-gap-E-pipe-ipc-tools.md` | `pipe/open`, `pipe/write`, `pipe/read`, `pipe/close` IPC handlers | Medium | Gap A, Gap B, Gap C |

### Recommended Build Order

```
ipc-gap-A  →  ipc-gap-B  →  ipc-gap-E
           →  ipc-gap-C  ↗
ipc-gap-D  (independent, can run in parallel with A)
```
