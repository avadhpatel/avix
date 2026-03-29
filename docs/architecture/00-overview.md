# 00 — Overview

> **Authoritative reference** for the Avix agent operating system design.
> Read this before any other architecture doc.

---

## What is Avix?

Avix is an agent operating system modelled on Unix/Linux primitives. The design maps agentic
concepts onto familiar OS abstractions so that existing OS intuition transfers directly.

**The load-bearing insight:** The LLM is stateless — analogous to a CPU executing instructions.
The `RuntimeExecutor` is the actual process — stateful, owns the conversation context, enforces
capability policy, and manages the full tool dispatch loop. Services are traditional deterministic
software written in any language. The capability token is the file descriptor table.

---

## System Architecture

```mermaid
graph TB
    subgraph EXTERNAL["External Layer (ATP over WebSocket/TLS)"]
        CLI[avix CLI]
        TUI[TUI Dashboard]
        TAURI[Tauri Desktop App]
        WEB[Web Client]
    end

    subgraph GATEWAY["gateway.svc — Protocol Bridge"]
        GW[GatewayServer\nWebSocket listener\nports 7700 / 7701]
        BUS[AtpEventBus\nbroadcast channel]
        GW <--> BUS
    end

    subgraph KERNEL["Kernel Layer"]
        KSOCK[kernel.sock\nKernelIpcServer]
        PROC[ProcHandler\nkernel/proc/spawn etc.]
        FACTORY[IpcExecutorFactory\nlaunches RuntimeExecutors]
        KSOCK --> PROC --> FACTORY
        FACTORY --> BUS
    end

    subgraph AGENTS["Agent Layer"]
        RE1[RuntimeExecutor PID 57\nLLM turn loop]
        RE2[RuntimeExecutor PID 58]
        RE1 --> BUS
        RE2 --> BUS
    end

    subgraph ROUTER["router.svc — Tool Dispatch Backbone"]
        RSOCK[router.sock\nRouterIpcServer]
    end

    subgraph SERVICES["Service Layer (JSON-RPC 2.0 over IPC)"]
        LLM[llm.svc\nllm.sock]
        FS[memfs.svc]
        EXEC[exec.svc]
        MCP[mcp-bridge.svc]
        THIRD[third-party services]
    end

    subgraph VFS["MemFS (VfsRouter)"]
        PROC_FS[/proc/pid/status.yaml\n/proc/pid/resolved.yaml]
        ETC_FS[/etc/avix/]
        USER_FS[/users/]
    end

    CLI & TUI & TAURI & WEB -->|ATP WebSocket| GW
    GW -->|JSON-RPC 2.0 IPC| KSOCK
    GW -->|JSON-RPC 2.0 IPC| RSOCK
    RE1 -->|JSON-RPC 2.0 IPC| RSOCK
    RE1 -->|JSON-RPC 2.0 IPC via IpcLlmClient| LLM
    RSOCK --> LLM & FS & EXEC & MCP & THIRD
    FACTORY -->|spawns| RE1 & RE2
    PROC --> VFS
    RE1 --> VFS
```

---

## Linux ↔ Avix Mapping

| Linux concept     | Avix equivalent                                                              |
|-------------------|------------------------------------------------------------------------------|
| Kernel / PID 1    | `avix` runtime binary + `kernel.agent`                                       |
| Processes         | Agents (LLM conversation loops + `RuntimeExecutor`)                          |
| Filesystem        | MemFS — driver-swappable VFS                                                 |
| Syscalls          | `/tools/kernel/**` — 32 calls across 6 domains                               |
| Shared libraries  | Services exposing tools at `/tools/<namespace>/`                             |
| IPC / sockets     | `router.svc` + platform-native local sockets at `/run/avix/`                 |
| Capabilities      | HMAC-signed `CapabilityToken` issued by `auth.svc`                           |
| Signals           | `SIGSTART`, `SIGPAUSE`, `SIGRESUME`, `SIGKILL`, `SIGSTOP`, `SIGSAVE`, `SIGPIPE`, `SIGESCALATE` |
| cgroups           | Capability token scopes                                                      |
| /proc             | `/proc/<pid>/status.yaml`, `/proc/<pid>/resolved.yaml`                       |
| dmesg             | Panic ring buffer → `kernel/sys/boot-log`                                    |
| systemd units     | `service.unit` and `agent.unit` files                                        |
| /etc/passwd       | `/etc/avix/users.yaml`                                                       |
| /etc/group        | `/etc/avix/crews.yaml`                                                       |
| sudoers           | `/etc/avix/auth.conf` + `kernel/cap/policy`                                  |

---

## The LLM-as-CPU Analogy

```
LLM inference call   =   CPU instruction execution  (stateless, repeatable)
RuntimeExecutor      =   OS process                 (stateful, owns context)
Capability token     =   File descriptor table      (scoped access list)
/tools/**            =   System call table          (stable API surface)
```

Every Avix feature is exposed to the LLM as a **tool** — never as raw IPC, signals, or
capability tokens. There are three tool categories:

| Category | Examples | How it works |
|---|---|---|
| **1 — Direct** | `fs/read`, `llm/complete` | LLM calls → RuntimeExecutor validates + dispatches via IPC |
| **2 — Avix Behaviour** | `agent/spawn`, `pipe/open` | Registered at spawn by RuntimeExecutor; translates to kernel syscall |
| **3 — Transparent** | HIL gating, token renewal | LLM never sees these; RuntimeExecutor handles automatically |

---

## Two Communication Layers

Avix has exactly two communication protocols that never cross:

```
EXTERNAL — clients ↔ Avix             INTERNAL — processes inside Avix
────────────────────────────          ─────────────────────────────────
ATP over WebSocket (TLS)              JSON-RPC 2.0 over local IPC sockets
Human users, apps, tooling            Services, agents, kernel
Authenticated via ATPToken            Authenticated via CapabilityToken / SvcToken
gateway.svc is the sole boundary      router.svc is the backbone
Long-lived, reconnectable             Fresh connection per call
```

`gateway.svc` is the **only** process that speaks both protocols. It translates ATP
commands into IPC calls. The internal world never speaks ATP. ATP never goes inside.

---

## Avix is LLM-Optional

The service tier runs with zero LLM dependency. `kernel.agent` activates only when a model
configuration is present and an LLM service is reachable. Services handle all deterministic
work and never require inference.

The test for whether something is an Agent or a Service:
> *Could a deterministic program with fixed rules solve this reliably?*
> - **YES → Service.** File I/O, auth, routing, scheduling, code execution, MCP adapting.
> - **NO → Agent.** Interpreting intent, multi-step planning, synthesising context.

---

## Key Design Invariants

These are hard rules. Violating any of them is a bug.

1. `auth.conf` must exist before `avix start` — no setup mode inside core.
2. `credential.type: none` does not exist — all auth uses `api_key` or `password`.
3. ATP (external) and IPC (internal) never cross the boundary.
4. `llm.svc` owns all AI inference — `RuntimeExecutor` never calls provider APIs directly.
5. Kernel syscalls are deterministic — they are never LLM-decided.
6. Tool names use `/` as separator; the wire mangles to `__`; no Avix tool name ever contains `__`.
7. Secrets in `/secrets/` are never VFS-readable — kernel-injected into agent env only.
8. Category 2 tools are registered by `RuntimeExecutor` at agent spawn and removed at exit.
9. Fresh IPC connection per call — no persistent multiplexed channels.
10. `ApprovalToken` is single-use — atomic first-responder-wins semantics.
11. The kernel never writes into user-owned trees (`/users/`, `/services/`, `/crews/`).
12. Users and agents never write into ephemeral (`/proc/`, `/kernel/`) or system trees.

---

## Architecture Documents Index

| Doc | Topic |
|-----|-------|
| 00-overview.md | **This file** — design philosophy, mapping, invariants |
| 01-filesystem.md | VFS trees, disk layout, write protection, mount system |
| 02-bootstrap.md | Boot phases 0–4, VFS init, config init, component wiring |
| 03-ipc.md | IPC protocol, wire format, component topology, message flows |
| 04-atp.md | Avix Terminal Protocol, gateway bridge, event delivery chain |
| 05-capabilities.md | CapabilityToken, HIL, role hierarchy, session model |
| 06-agents.md | RuntimeExecutor, spawn, turn loop, ATP event emission |
| 07-services.md | Service lifecycle, `service.unit` TOML, installation pipeline, `_caller` injection, restart watchdog, secrets |
| 08-llm-service.md | llm.svc multi-modality, provider routing |
| 09-runtime-executor-tools.md | Tool categories, 7-step turn loop, HIL scenarios |

**Comprehensive narrative reference:** `docs/Avix-Architecture.md`
