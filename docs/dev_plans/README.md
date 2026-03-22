# Development Plans

This folder is used for active development initiatives.
It may contains markdown files that will be used for dev reviews and coding
tools to help build the right solution.

Consider files in this folder temporary and can be deleted as per dev's needs.

---

## Active Plans

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

### KernelConfig + Param System Gaps

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `param-gap-A-kernel-config-expansion.md` | Full KernelConfig schema, validation, reload classification, config_init template | High | — |
| `param-gap-B-defaults-limits-types.md` | Typed Defaults and Limits structs; constraint types; replace hard-coded bootstrap values | High | — |
| `param-gap-C-resolution-engine.md` | ParamResolver: merge system→crew→user→manifest, clamp against limits, provenance annotations | High | Gap B |
| `param-gap-D-resolved-at-spawn.md` | Wire resolution engine into RuntimeExecutor; enforce resolved values; spawn error files | High | Gap B, Gap C |
| `param-gap-E-resolve-cli.md` | `avix resolve` CLI + `avix config reload --check` command | Medium | Gap B, Gap C, Gap D |

### Recommended Build Order

```
param-gap-A  (independent — expand KernelConfig struct)
param-gap-B  (independent — typed Defaults/Limits structs)
param-gap-B  →  param-gap-C  →  param-gap-D  →  param-gap-E
```
