# 01 — Filesystem

> VFS trees, disk layout, directory reference, write-protection rules, and mount system.

---

## Filesystem Trees

The Avix filesystem is divided into four ownership classes. **Ownership is encoded in location** —
a file in the wrong tree is a bug.

```
┌─────────────────────────────────────────────────────────────────┐
│  EPHEMERAL — Owner: Kernel — Lifetime: Lost on reboot           │
│                                                                 │
│  /proc/      per-agent, per-user, per-service runtime state     │
│  /kernel/    system-wide defaults and limits (VFS, not disk)    │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — SYSTEM — Owner: root — Survives reboot            │
│                                                                 │
│  /bin/       system-installed agents                            │
│  /etc/avix/  system configuration                               │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — SECRETS — Kernel-mediated — Not portable          │
│                                                                 │
│  /secrets/<username>/    encrypted credential store per user    │
│  /secrets/services/<n>/  encrypted credential store per service │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — USER/OPERATOR — Portable — Freely exportable      │
│                                                                 │
│  /users/<username>/        human operator workspaces            │
│  /services/<svcname>/      service account workspaces           │
│  /crews/<crew-name>/       crew shared spaces                   │
└─────────────────────────────────────────────────────────────────┘
```

---

## Disk Layout — AVIX_ROOT

All persistent trees live under a single `AVIX_ROOT` directory. Avix derives all internal
VFS paths from it. Individual subtrees can be overridden via `fstab.yaml`.

```
AVIX_ROOT/                 (e.g. ~/avix-data or /var/avix-data)
├── etc/                   → VFS /etc/avix/
│   ├── auth.conf          (chmod 600 — credential hashes)
│   ├── kernel.yaml        (chmod 600 — master key source config)
│   ├── users.yaml
│   ├── crews.yaml
│   ├── crontab.yaml
│   └── fstab.yaml
├── bin/                   → VFS /bin/
├── services/              → VFS /services/
│   └── <svcname>/
│       ├── service.yaml
│       ├── bin/
│       ├── tools/
│       ├── workspace/
│       └── .install.json
├── users/                 → VFS /users/
│   └── <username>/
│       ├── workspace/
│       ├── snapshots/
│       ├── defaults.yaml
│       ├── limits.yaml
│       ├── bin/               → VFS /users/<username>/bin/ (user-installed agents)
│       │   └── <agent-name>/
│       │       └── manifest.yaml
│       └── agents/            → invocation records (written by kernel via LocalProvider)
│           └── <agent-name>/
│               └── invocations/
│                   ├── <uuid>.yaml          (summary: status, tokens, timing)
│                   └── <uuid>/
│                       └── conversation.jsonl
├── crews/                 → VFS /crews/
├── secrets/               → VFS /secrets/ (AES-256-GCM blobs, chmod 700)
└── logs/                  → /var/log/avix/
```

File permissions (set by installer, not Avix core):

| Path | Mode | Reason |
|------|------|--------|
| `AVIX_ROOT/etc/` | 700 | Only avix process user can read |
| `AVIX_ROOT/etc/auth.conf` | 600 | Credential hashes |
| `AVIX_ROOT/etc/kernel.yaml` | 600 | Master key source |
| `AVIX_ROOT/secrets/` | 700 | Kernel-only tree |
| `AVIX_ROOT/secrets/**/*.enc` | 600 | Encrypted blobs |
| `AVIX_ROOT/users/` | 755 | No secrets — freely readable |

---

## Full VFS Directory Reference

### Ephemeral Tree

Written at runtime, lost on reboot. Kernel-owned.

```
/proc/
├── <pid>/
│   ├── status.yaml          AgentStatus — written by RuntimeExecutor at spawn
│   ├── resolved.yaml        Resolved config — written by RuntimeExecutor at spawn
│   ├── pipes/<pipe-id>.yaml
│   └── hil-queue/<request-id>.yaml
├── users/<username>/
│   ├── status.yaml
│   ├── sessions/<session-id>.yaml   SessionManifest — written by SessionStore
│   ├── logs/
│   └── resolved/<kind>.yaml
├── services/<svcname>/
│   ├── status.yaml
│   └── logs/
├── gateway/
│   ├── connections.yaml
│   └── subscriptions.yaml
└── spawn-errors/<request-id>.yaml

/kernel/
├── defaults/
│   ├── agent.yaml           AgentDefaults — written by bootstrap Phase 1
│   └── pipe.yaml            PipeDefaults — written by bootstrap Phase 1
└── limits/
    └── agent.yaml           AgentLimits — written by bootstrap Phase 1
```

### Persistent System Tree

```
/bin/<agent>/manifest.yaml

/etc/avix/
├── auth.conf
├── kernel.yaml
├── users.yaml
├── crews.yaml
├── crontab.yaml
└── fstab.yaml
```

### Persistent Secrets Tree

**No path under `/secrets/` is ever readable via a VFS `read` call.** Returns `EPERM`.

```
/secrets/
├── <username>/<secret-name>.enc
└── services/<svcname>/<secret-name>.enc
```

### Persistent User/Operator Tree

```
/users/<username>/
├── defaults.yaml
├── limits.yaml
├── workspace/
└── snapshots/<agentName>-<YYYYMMDD>-<HHMM>.yaml   # SnapshotFile YAML; kind: Snapshot

/services/<svcname>/
├── defaults.yaml
├── limits.yaml
├── workspace/
└── snapshots/

/crews/<crew-name>/
├── defaults.yaml
├── limits.yaml
└── shared/
```

---

## Write Protection Rules

Ownership is enforced at the syscall layer, not in `MemFs` itself. `MemFs::write()` is
unrestricted for legitimate kernel code. The `kernel/fs/write` syscall handler is where
agent write attempts are blocked.

### Agent-writable paths

| Path prefix | Agent writable? | Reason |
|-------------|:--------------:|--------|
| `/users/` | Yes | User-owned workspace |
| `/services/` | Yes | Service-owned workspace |
| `/crews/` | Yes | Crew shared space |
| `/proc/` | **No** | Kernel-generated runtime state |
| `/kernel/` | **No** | Compiled-in defaults and limits |
| `/secrets/` | **No** | Kernel-managed encrypted store |
| `/etc/avix/` | **No** | System configuration (operator-only) |
| `/bin/` | **No** | System agents (operator-only) |

**Rule:** `VfsPath::is_agent_writable()` returns `false` for the five blocked prefixes.
The `kernel/fs/write` syscall handler calls this before any write and returns `EPERM`
if the path is kernel-owned.

```
kernel/fs/write to /proc/<anything>   → EPERM (even with admin token)
kernel/fs/write to /kernel/<anything> → EPERM (even with admin token)
kernel/fs/read  of /proc/<pid>/status.yaml → OK (reads are not blocked)
```

**`MemFs::write()` itself is never modified** — kernel boot code, agent spawn, and session
manifests all call it directly and require unrestricted write access.

---

## YAML Schema Conventions

All Avix config files use YAML with Kubernetes-style structure:

| Field | Rule |
|-------|------|
| `apiVersion` | Always `avix/v1` |
| `kind` | PascalCase resource type |
| `metadata` | Provenance fields (name, version, timestamps) |
| `spec` | **Required** on authored files |
| `status` | Kernel-written runtime state — read-only |
| `limits` | Kernel-owned bounds — read-only |
| `resolved` | Kernel-derived merge — never authored |

All timestamps: ISO 8601 with timezone. All durations in seconds unless noted.

---

## Schema Index

| # | Kind | Location | Direction |
|---|------|----------|-----------|
| 1 | AgentManifest | `/bin/<agent>/manifest.yaml` | Config (static) |
| 2 | AgentStatus | `/proc/<pid>/status.yaml` | Status (dynamic) |
| 3 | Users | `/etc/avix/users.yaml` | Config (static) |
| 4 | Crews | `/etc/avix/crews.yaml` | Config (static) |
| 5 | KernelConfig | `/etc/avix/kernel.yaml` | Config (static) |
| 6 | AuthConfig | `/etc/avix/auth.conf` | Config (static) |
| 7 | CapabilityToken | Issued by kernel at spawn | Runtime (issued) |
| 8 | ATPToken | Issued by auth.svc on login | Runtime (issued) |
| 9 | SessionManifest | `/proc/users/<username>/sessions/<sid>.yaml` | Status (ephemeral) |
| 10 | Resolved | `/proc/<pid>/resolved.yaml` | Runtime (kernel-derived) |
| 11 | AgentDefaults | `/kernel/defaults/agent.yaml` | Config (compiled-in) |
| 12 | PipeDefaults | `/kernel/defaults/pipe.yaml` | Config (compiled-in) |
| 13 | AgentLimits | `/kernel/limits/agent.yaml` | Runtime (kernel-owned) |
| 14 | Fstab | `/etc/avix/fstab.yaml` | Config (static) |
| 15 | Crontab | `/etc/avix/crontab.yaml` | Config (static) |

---

## Mount System — Status and Roadmap

### v0.1 Status (current)

All VFS access goes through `MemFs` (in-memory, non-persistent). `fstab.yaml` is **written**
by `avix config init` but **not parsed or acted on** during bootstrap. When the process exits,
all VFS state is lost. This is acceptable for v0.1 because the full agent loop, capability
system, and tool infrastructure are the primary deliverables.

### v0.2 Roadmap

The mount system will add:

1. `FstabConfig` struct + bootstrap parsing — failed mount → `EUNAVAIL` for affected paths
2. `StorageProvider` trait + `LocalProvider` + `MemoryProvider` (wraps existing MemFs)
3. Mount registry: `HashMap<String, Arc<dyn StorageProvider>>`, longest-prefix-first routing
4. `kernel/fs/*` syscalls route through `MountRegistry` instead of `MemFs` directly
5. `avix mount` CLI commands

### v0.3+ Roadmap

Cloud providers (`s3`, `gcs`, `azure-blob`) behind feature flags.

**Dependencies before mount system can be scheduled:**
- Bootstrap Phase 1 VFS init must be complete (see `02-bootstrap.md`)
- `avix config init` must write `fstab.yaml` (see `02-bootstrap.md`)
- Day-21 `kernel/fs/*` syscalls must be complete
