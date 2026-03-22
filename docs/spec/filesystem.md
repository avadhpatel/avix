# Avix Filesystem Specification (v1)

This document is the authoritative reference for the Avix Virtual Filesystem (VFS). It
covers the full directory structure, the persistence model, access control rules, the
secrets store, and the mount system that allows individual trees to be backed by
different storage providers — local disk, cloud object stores, encrypted volumes, or
remote filesystems.

-----

## Table of Contents

1. [Design Principles](#1-design-principles)
1. [Filesystem Trees](#2-filesystem-trees)
1. [Full Directory Reference](#3-full-directory-reference)
1. [Persistence Model](#4-persistence-model)
1. [Access Control](#5-access-control)
1. [Secrets Store](#6-secrets-store)
1. [Mount System](#7-mount-system)
1. [Storage Providers](#8-storage-providers)
1. [Portability and Migration](#9-portability-and-migration)
1. [Mount Configuration Reference](#10-mount-configuration-reference)

-----

## 1. Design Principles

The Avix filesystem is built around five rules that inform every path and access decision:

**1. Ownership is encoded in location.**
Where a file lives tells you who wrote it and who may change it. Kernel-generated files
live under `/proc/` and `/kernel/`. User files live under `/users/`. There are no
exceptions. A file in the wrong tree is a bug.

**2. Ephemeral and persistent content never share a tree.**
Runtime state that the kernel generates and discards on reboot lives in a completely
separate set of paths from content that must survive across reboots. You should be able
to back up Avix by syncing three directories (`/users/`, `/services/`, `/crews/`) plus
system config (`/etc/avix/`, `/bin/`) — nothing else is needed.

**3. The portable user workspace contains no sensitive or runtime content.**
`/users/<username>/` is designed to be freely exported, backed up, and migrated between
instances. Sessions are runtime state — they live in `/proc/`. Secrets are sensitive and
instance-scoped — they live in `/secrets/`. The user workspace tree can be copied without
any security consideration.

**4. Every persistent tree is independently mountable.**
`/users/<username>/`, `/services/<svcname>/`, `/crews/<crew-name>/shared/`, and
`/secrets/<username>/` are all designed to be backed by any compatible storage provider.
The kernel does not care where the bytes live.

**5. The kernel mediates all access.**
No agent or user accesses the filesystem directly. Every read and write goes through the
kernel, which enforces tool grants, ACL rules, and mount permissions. Storage providers
are interchangeable without any change to agent code.

-----

## 2. Filesystem Trees

The Avix filesystem is divided into four trees based on ownership and lifetime.

```
┌─────────────────────────────────────────────────────────────────┐
│  EPHEMERAL                                                      │
│  Owner: Kernel    Lifetime: Lost on reboot                      │
│                                                                 │
│  /proc/      per-agent, per-user, per-service runtime state     │
│  /kernel/    system-wide VFS (defaults, limits)                 │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — SYSTEM                                            │
│  Owner: root      Lifetime: Survives reboot                     │
│                                                                 │
│  /bin/       system-installed agents                            │
│  /etc/avix/  system configuration                               │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — SECRETS                                           │
│  Owner: Kernel (writes) / User (via avix secret CLI only)       │
│  Lifetime: Survives reboot    Portable: No — instance-scoped    │
│                                                                 │
│  /secrets/<username>/    encrypted credential store per user    │
│  /secrets/services/<n>/  encrypted credential store per service │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — USER / OPERATOR                                   │
│  Owner: users, services, crews    Lifetime: Survives reboot     │
│  Portable: yes — freely exportable, no sensitive content        │
│                                                                 │
│  /users/<username>/        human operator workspaces            │
│  /services/<svcname>/      service account workspaces           │
│  /crews/<crew-name>/       crew shared spaces                   │
└─────────────────────────────────────────────────────────────────┘
```

**The hard rules:**

- The kernel never writes into user-owned trees (`/users/`, `/services/`, `/crews/`)
- Users and agents never write into ephemeral or system trees
- Secrets in `/secrets/` are never readable via the VFS — only injectable by the kernel
- Sessions live in `/proc/` — they are runtime state, not user data

-----

## 3. Full Directory Reference

### 3.1 Ephemeral Tree

All paths under `/proc/` and `/kernel/` are kernel-generated. They are populated at boot
or at agent spawn time and are not persisted across reboots. They may not be written to
by users, agents, or operators.

```
/proc/
│
├── <pid>/                          Per-agent runtime state (one dir per running agent)
│   ├── status.yaml                 Agent state, context usage, metrics — see AgentStatus
│   ├── resolved.yaml               Fully merged config this agent runs under — see Resolved
│   ├── pipes/
│   │   └── <pipe-id>.yaml          Active pipe descriptors — see Pipe
│   └── hil-queue/                  Human-in-the-loop requests awaiting approval
│       └── <request-id>.yaml
│
├── users/
│   └── <username>/                 Per-user kernel-generated views (read-only)
│       ├── status.yaml             Summary: running agents, quota consumption
│       ├── sessions/               Active sessions for this user (runtime only)
│       │   └── <session-id>.yaml   See SessionManifest — ephemeral, lost on reboot
│       ├── logs/                   Runtime logs for this user's agents (current session)
│       └── resolved/               Pre-spawn resolved config previews
│           └── <kind>.yaml
│
├── services/
│   └── <svcname>/                  Per-service kernel-generated views (read-only)
│       ├── status.yaml
│       ├── sessions/               Active sessions for this service (runtime only)
│       │   └── <session-id>.yaml
│       ├── logs/
│       └── resolved/
│           └── <kind>.yaml
│
└── spawn-errors/                   Failed spawn attempts and resolve errors
    └── <request-id>.yaml


/kernel/
│
├── defaults/                       Compiled-in system defaults (static, build-time)
│   ├── <kind>.yaml                 e.g. agent-manifest.yaml, pipe.yaml
│   ├── tools/
│   │   └── <tool>.yaml             e.g. web.yaml, email.yaml, code-exec.yaml
│   └── models/
│       └── <model>.yaml            e.g. claude-sonnet-4.yaml, claude-haiku-4.yaml
│
└── limits/                         Dynamic system limits (kernel-updated at runtime)
    ├── <kind>.yaml
    ├── tools/
    │   └── <tool>.yaml
    └── models/
        └── <model>.yaml
```

### 3.2 Persistent System Tree

Root-owned. Changed only by system operators. Backed up as part of system configuration.

```
/bin/
└── <agent>/                        System-installed agent
    ├── manifest.yaml               AgentManifest — see agent-manifest schema
    └── ...                         Agent implementation files

/etc/avix/
├── kernel.yaml                     Kernel master config — see KernelConfig
├── users.yaml                      User registry — see Users
├── crews.yaml                      Crew registry — see Crews
├── crontab.yaml                    Scheduled jobs — see Crontab
└── fstab.yaml                      Mount configuration — see §10
```

### 3.3 Persistent Secrets Tree

Kernel-managed. Never readable via VFS. Survives reboots but is **not portable** — secrets
are encrypted with an instance-scoped master key. See [§6 Secrets Store](#6-secrets-store)
for the full model.

```
/secrets/
│
├── <username>/                     Per-user encrypted credential store
│   └── <secret-name>.enc           AES-256-GCM encrypted blob; kernel-managed
│
└── services/
    └── <svcname>/                  Per-service encrypted credential store
        └── <secret-name>.enc
```

> **Access rule:** No path under `/secrets/` is ever readable via a VFS `read` call —
> not by agents, not by users, not by operators browsing the filesystem. The kernel reads
> and decrypts blobs exclusively in response to a `ResourceRequest` for a named secret,
> then injects the plaintext value directly into the requesting agent’s context.

### 3.4 Persistent User Tree

Each principal (`/users/<username>/`, `/services/<svcname>/`) gets a clean, portable
workspace. The entire tree can be exported, backed up, or migrated without any security
consideration — it contains no secrets and no runtime state.

```
/users/<username>/
│
├── bin/                            User-installed agents
│   └── <agent>/
│       ├── manifest.yaml           AgentManifest for this agent
│       └── ...
│
├── defaults.yaml                   User default overrides (nested by target kind)
├── limits.yaml                     User agent limits (nested by target kind)
│
├── workspace/                      Primary agent working space
│   │                               Agents read and write here by default.
│   │                               Structure is user-defined — no enforced layout.
│   └── ...
│
└── snapshots/                      Agent snapshots (persisted work products)
    └── <agent>-<timestamp>.yaml    See Snapshot schema


/services/<svcname>/                Mirrors /users/ structure exactly
│
├── bin/
├── defaults.yaml
├── limits.yaml
├── workspace/
└── snapshots/
```

### 3.5 Crew Shared Tree

Crew directories are owned by the crew, not any individual user. Members access them
via ACL grants defined in `crews.yaml`.

```
/crews/<crew-name>/
├── defaults.yaml                   Crew default overrides
├── limits.yaml                     Crew agent limits
└── shared/                         Shared work products
    └── ...                         Structure is crew-defined — no enforced layout
```

-----

## 4. Persistence Model

### 4.1 What Survives a Reboot

|Content                                          |Survives?        |Notes                                                    |
|-------------------------------------------------|-----------------|---------------------------------------------------------|
|Agent runtime state (`/proc/<pid>/`)             |No               |Agents must be respawned                                 |
|Session state (`/proc/users/<u>/sessions/`)      |No               |Sessions are runtime — reconnect creates a new session   |
|Kernel limits (`/kernel/limits/`)                |No               |Re-initialised from compiled-in values at boot           |
|Kernel defaults (`/kernel/defaults/`)            |Yes (compiled-in)|Always present; not stored on disk                       |
|System config (`/etc/avix/`)                     |Yes              |Restored from backing store                              |
|System agents (`/bin/`)                          |Yes              |Restored from backing store                              |
|Secrets (`/secrets/`)                            |Yes              |Encrypted blobs survive reboot; master key loaded at boot|
|User workspaces (`/users/<username>/`)           |Yes              |Restored from backing store                              |
|Service workspaces (`/services/<svcname>/`)      |Yes              |Restored from backing store                              |
|Crew shared spaces (`/crews/<crew-name>/shared/`)|Yes              |Restored from backing store                              |
|Agent snapshots                                  |Yes              |Under `/users/` or `/services/` — persist normally       |

### 4.2 Crash Recovery

On an unclean shutdown, the kernel performs the following on next boot:

1. Ephemeral trees are wiped and rebuilt from scratch. All session state is lost.
1. For any agent that had `snapshot.restoreOnCrash: true`, the kernel locates the most
   recent snapshot under `/users/<username>/snapshots/` and queues a restore.
1. Snapshot integrity is verified via `checksum` before restore. Corrupt snapshots are
   skipped and logged to `/proc/spawn-errors/`.
1. Restored agents receive `SIGSTART` with a `restored: true` flag so they can resume
   rather than restart.
1. The secrets master key is re-loaded from its configured source (passphrase, key file,
   or cloud KMS) before any agent that requires secrets can be spawned.

### 4.3 Backup Strategy

The minimal backup set for a full Avix instance:

```sh
# System config and agents
/etc/avix/
/bin/

# All user, service, and crew data (no sensitive content — safe to backup freely)
/users/
/services/
/crews/

# Secrets — backup separately with extra access controls
# Note: encrypted blobs are safe to backup but useless without the master key
/secrets/
```

Ephemeral trees (`/proc/`, `/kernel/`) are never backed up.

**Important:** `/secrets/` should be backed up with stricter access controls than the
rest. The encrypted blobs are safe at rest, but limiting access to the backup is good
practice.

-----

## 5. Access Control

### 5.1 Permission Model

Avix uses a three-principal permission model on every filesystem path, directly analogous
to Unix `owner/group/world` bits:

```
owner : rw    read-write for the owning user or service
crew  : r     read-only for members of the owning crew
world : r--   read-only for all other authenticated principals
```

### 5.2 Tool-Gated Path Access

Certain paths require explicit tools in the agent’s `CapabilityToken`:

|Path                           |Required tool                                |Notes                      |
|-------------------------------|---------------------------------------------|---------------------------|
|`/users/<u>/workspace/` (read) |`file_read`                                  |                           |
|`/users/<u>/workspace/` (write)|`file_write`                                 |                           |
|`/users/<u>/snapshots/`        |`snapshot` declared in manifest              |                           |
|`/crews/<c>/shared/`           |`file_read` or `file_write` + crew membership|                           |
|`/users/<u>/bin/` (write)      |`file_write`                                 |User-level agent install   |
|`/bin/` (write)                |Root only                                    |System agent install       |
|`/secrets/` (any)              |None — kernel-only                           |Never accessible via VFS   |
|`/etc/avix/` (write)           |Root only                                    |Cannot be granted to agents|
|`/proc/` (write)               |None — kernel-only                           |Cannot be granted to anyone|
|`/kernel/` (write)             |None — kernel-only                           |Cannot be granted to anyone|

### 5.3 Secrets Access

Secrets are never accessible via the VFS. Agents request a named secret via
`ResourceRequest`:

```yaml
- resource: secret
  name: openai-api-key
  reason: Required for external API call
```

The kernel:

1. Validates the request against the agent’s tool grants and user limits
1. Reads and decrypts the blob from `/secrets/<username>/<name>.enc`
1. Injects the plaintext value directly into the agent’s context
1. Logs the access event (who, which secret, which agent PID, timestamp)
1. Never writes the plaintext to any VFS path

### 5.4 Crew Shared Path Access

|`pipePolicy`      |Effect on shared path access                                 |
|------------------|-------------------------------------------------------------|
|`allow-intra-crew`|Members read and write shared paths without a ResourceRequest|
|`require-request` |Members must submit a ResourceRequest for each access        |
|`deny`            |No agent access; operator-only                               |

### 5.5 Mount-Level Access Control

|Mount option|Effect                                                        |
|------------|--------------------------------------------------------------|
|`readonly`  |No writes permitted via this mount, regardless of ACL         |
|`noexec`    |Agent binaries under this mount may not be spawned            |
|`encrypted` |Kernel enforces encryption at rest via the configured provider|

-----

## 6. Secrets Store

### 6.1 Design

The secrets store solves a problem that `/etc/shadow`-style hashing cannot: agents need
the **actual plaintext value** of credentials to call external APIs and services. Hashing
is not an option. The solution is **encryption at rest with a kernel-held master key**.

Key properties:

- Secrets are stored as AES-256-GCM encrypted blobs on disk — ciphertext at rest
- The master key lives only in kernel memory; never written to the filesystem
- No VFS path exposes plaintext — ever
- Access is logged per-request for auditability
- Secrets survive reboots (encrypted blobs persist); the master key is re-loaded at boot
- Secrets are **instance-scoped** — they do not travel with the user workspace export

### 6.2 Encryption Model

```
At write time (avix secret set <name> <value>):
─────────────────────────────────────────────
plaintext
  → AES-256-GCM encrypt with master key + per-secret nonce
  → write encrypted blob to /secrets/<username>/<name>.enc
  → log write event

At read time (agent ResourceRequest for secret):
────────────────────────────────────────────────
kernel reads /secrets/<username>/<name>.enc
  → AES-256-GCM decrypt with master key + stored nonce
  → inject plaintext into agent context only
  → log read event (pid, username, secret name, timestamp)
  → plaintext never leaves kernel memory
```

### 6.3 Master Key Loading

The master key source is configured in `KernelConfig.secrets.masterKey`. The kernel
loads it once at boot before any secrets-dependent agent can spawn.

|Source      |Description                                                 |Use case             |
|------------|------------------------------------------------------------|---------------------|
|`passphrase`|Operator enters passphrase at boot; key derived via Argon2id|Dev, single-node     |
|`key-file`  |Key material read from a file on a separate volume          |Simple production    |
|`env`       |Key material read from an environment variable              |Container deployments|
|`kms-aws`   |AWS KMS — kernel calls KMS API at boot to decrypt a data key|AWS production       |
|`kms-gcp`   |GCP Cloud KMS                                               |GCP production       |
|`kms-azure` |Azure Key Vault                                             |Azure production     |
|`kms-vault` |HashiCorp Vault Transit secrets engine                      |Multi-cloud / on-prem|

### 6.4 KernelConfig — Secrets Section

```yaml
spec:
  secrets:
    algorithm: aes-256-gcm          # aes-256-gcm | chacha20-poly1305

    masterKey:
      source: passphrase            # passphrase | key-file | env | kms-aws | kms-gcp | kms-azure | kms-vault

      # source: passphrase
      kdfAlgorithm: argon2id        # key derivation function
      kdfMemoryMb: 64
      kdfIterations: 3

      # source: key-file
      # keyFile: /mnt/keyvolume/avix-master.key

      # source: env
      # envVar: AVIX_MASTER_KEY

      # source: kms-aws
      # kmsKeyId: arn:aws:kms:us-east-1:123456789:key/abc-def
      # encryptedDataKey: /etc/avix/secrets/master-key.enc  # KMS-encrypted data key stored locally

      # source: kms-vault
      # vaultAddr: https://vault.internal:8200
      # vaultToken: /etc/avix/secrets/vault-token.enc
      # transitKeyName: avix-master

    store:
      path: /secrets               # root path for encrypted blobs
      provider: local              # local | s3 | gcs | azure-blob (same providers as mounts)
      # For non-local providers, config mirrors fstab.yaml provider config

    audit:
      enabled: true
      logPath: /var/log/avix/secrets-audit.log
      logReads: true               # log every secret read (who, which, when)
      logWrites: true              # log every secret write
```

### 6.5 CLI Operations

Secrets are managed exclusively through the `avix secret` CLI. No direct file access.

```sh
# Set a secret
avix secret set openai-api-key "sk-..."
avix secret set db-password "hunter2" --for-service svc-pipeline

# List secret names (never values)
avix secret list
avix secret list --user alice

# Delete a secret
avix secret delete openai-api-key

# Rotate a secret (atomic: write new, verify, delete old)
avix secret rotate openai-api-key "sk-new-..."

# Check which agents have accessed a secret (from audit log)
avix secret audit openai-api-key
```

### 6.6 Provider Extensibility

The secrets store uses the same provider model as filesystem mounts. The `store.provider`
field in `KernelConfig.secrets` accepts any of the same provider types. For cloud
deployments, secrets blobs can be stored in S3 or GCS rather than local disk:

```yaml
secrets:
  store:
    provider: s3
    config:
      bucket: avix-secrets-prod
      prefix: secrets/
      region: us-east-1
      auth: iam-role
    options:
      encrypted: true              # provider-level encryption in addition to blob encryption
```

This gives you two layers of encryption: the blob is AES-256-GCM encrypted by Avix, and
the object store adds its own server-side encryption on top.

-----

## 7. Mount System

### 7.1 Overview

Every persistent tree in Avix is a mount point. The kernel delegates all I/O to a
storage provider configured per mount. This means:

- `/users/alice/` can be local disk on dev and S3 on production — agent code unchanged
- Individual subtrees can have different providers (fast NVMe for workspace, cold storage
  for snapshots)
- An entire user workspace can be migrated by remounting from a new provider

### 7.2 Mount Granularity

|Granularity   |Example                     |Use case                                                |
|--------------|----------------------------|--------------------------------------------------------|
|Principal root|`/users/alice/`             |Mount entire user workspace from one provider           |
|Subtree       |`/users/alice/workspace/`   |Different provider or policy per subtree                |
|Crew shared   |`/crews/researchers/shared/`|Shared storage for multi-user collaboration             |
|Secrets       |`/secrets/`                 |Separate encrypted store; can use any provider          |
|System        |`/bin/`, `/etc/avix/`       |System config on read-only or version-controlled storage|

### 7.3 Mount Lifecycle

```
Boot
────
1. Kernel reads /etc/avix/fstab.yaml
2. For each mount, initialises the configured storage provider
3. Provider authenticates (credentials from /etc/avix/ or env)
4. Mount registered; path accessible
5. Failed mount → affected paths unavailable; agents requiring them held in 'pending'

Runtime
───────
6. Kernel watches mount health; unhealthy mounts emit SIGUSR1 to affected agents
7. Mounts can be managed at runtime:
   avix mount add <path> --provider <type> --config <file>
   avix mount remove <path>
   avix mount status

Shutdown
────────
8. Kernel flushes all pending writes before shutdown
9. Providers closed cleanly; incomplete writes logged
```

-----

## 8. Storage Providers

### 8.1 Provider Reference

|Provider            |Type string      |Description                               |Best for                    |
|--------------------|-----------------|------------------------------------------|----------------------------|
|Local disk          |`local`          |Standard filesystem path on the host      |Dev, single-node            |
|S3-compatible       |`s3`             |AWS S3, MinIO, Backblaze B2, Cloudflare R2|Cloud, cold storage         |
|Google Cloud Storage|`gcs`            |GCS buckets                               |GCP deployments             |
|Azure Blob Storage  |`azure-blob`     |Azure containers                          |Azure deployments           |
|NFS                 |`nfs`            |Network File System                       |Shared on-prem              |
|SFTP                |`sftp`           |SSH file transfer                         |Remote servers              |
|Encrypted volume    |`encrypted-local`|Local disk with kernel-managed encryption |Legacy encrypted volumes    |
|Git-backed          |`git`            |Git repository as filesystem              |Agent bins, versioned config|
|In-memory           |`memory`         |RAM-backed, non-persistent                |Testing, ephemeral scratch  |

### 8.2 Provider Capabilities

|Provider         |Random read|Random write|Atomic rename|Watch/notify|Encryption at rest|
|-----------------|-----------|------------|-------------|------------|------------------|
|`local`          |✓          |✓           |✓            |✓           |via OS            |
|`s3`             |✓          |✓           |✗            |✗ (poll)    |✓ SSE             |
|`gcs`            |✓          |✓           |✗            |✓ pub/sub   |✓                 |
|`azure-blob`     |✓          |✓           |✗            |✓ event grid|✓                 |
|`nfs`            |✓          |✓           |✓            |✓           |via layer         |
|`sftp`           |✓          |✓           |✗            |✗           |via SSH           |
|`encrypted-local`|✓          |✓           |✓            |✓           |✓                 |
|`git`            |✓          |✓ (commit)  |✓            |✓           |via signing       |
|`memory`         |✓          |✓           |✓            |✓           |✗                 |


> Object stores (S3, GCS, Azure) do not support atomic rename. The kernel serialises
> concurrent writes using optimistic locking (ETag / generation check).

-----

## 9. Portability and Migration

### 9.1 Portable Unit

The portable unit is `/users/<username>/`. It contains no secrets and no runtime state —
it can be copied, exported, and imported without any security consideration.

```sh
# Export user workspace
avix export --user alice --dest s3://avix-backup/alice/

# Import into new instance
avix import --user alice --src s3://avix-backup/alice/

# Full instance clone (workspace + config, not secrets)
rsync -av /users/ /services/ /crews/ /etc/avix/ /bin/ /destination/
```

### 9.2 Secrets Portability

Secrets are **not** included in workspace exports. They must be migrated separately and
re-encrypted with the destination instance’s master key:

```sh
# Export secret names only (not values) — for documentation
avix secret list --user alice

# On destination instance: re-enter secrets manually
avix secret set openai-api-key "sk-..."

# Or: if both instances share a KMS key, secrets blobs can be copied directly
# since they're encrypted with the KMS-derived key, not an instance-specific key
rsync -av /secrets/ destination:/secrets/   # only safe if master key source is shared KMS
```

### 9.3 Migration Scenarios

**Dev to production:**

```
Dev:   /users/alice/  → local disk
Prod:  /users/alice/  → s3://avix-prod/users/alice/

Steps:
1. avix export --user alice --dest s3://avix-prod/users/alice/
2. On prod: add mount entry pointing to S3 path
3. On prod: add alice to users.yaml with matching uid
4. On prod: re-enter alice's secrets via avix secret set
```

**Multi-instance shared workspace:**

```
Both instances mount the same S3 path for /users/alice/workspace/
Secrets are NOT shared — each instance has its own /secrets/alice/
Each instance must have alice's secrets set independently
```

### 9.4 Portability Constraints

|Item                  |Portable?|Notes                                                                |
|----------------------|---------|---------------------------------------------------------------------|
|`/users/<username>/`  |✓ Yes    |Entirely safe to copy; no sensitive content                          |
|`/services/<svcname>/`|✓ Yes    |Same as users                                                        |
|`/crews/<crew-name>/` |✓ Yes    |Shared work products; safe to copy                                   |
|`/secrets/`           |✗ No     |Instance-scoped encryption; must be re-entered or shared KMS required|
|`/proc/` state        |✗ No     |Ephemeral; never exported                                            |
|Capability tokens     |✗ No     |Instance-scoped and short-lived                                      |
|`uid` / `cid` mappings|⚠ Check  |Must match or be remapped on import                                  |

-----

## 10. Mount Configuration Reference

Mounts are defined in `/etc/avix/fstab.yaml`.

### 10.1 Schema

```yaml
apiVersion: avix/v1
kind: Fstab
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  mounts:

    # ── System trees ──────────────────────────────────────────────────────────

    - path: /etc/avix
      provider: local
      config:
        root: /var/avix-data/etc
      options:
        readonly: false

    - path: /bin
      provider: local
      config:
        root: /var/avix-data/bin
      options:
        readonly: false
        noexec: false

    # ── User workspaces ───────────────────────────────────────────────────────

    - path: /users/alice
      provider: local
      config:
        root: /var/avix-data/users/alice
      options:
        maxSizeGb: 50

    # alice's workspace on fast local NVMe
    - path: /users/alice/workspace
      provider: local
      config:
        root: /mnt/nvme0/users/alice/workspace

    # alice's snapshots on cold S3
    - path: /users/alice/snapshots
      provider: s3
      config:
        bucket: avix-snapshots-prod
        prefix: users/alice/snapshots/
        region: us-east-1
        auth: iam-role
      options:
        encrypted: true

    # ── Service workspaces ────────────────────────────────────────────────────

    - path: /services/svc-pipeline
      provider: s3
      config:
        bucket: avix-services-prod
        prefix: svc-pipeline/
        region: us-east-1
        auth: iam-role
      options:
        encrypted: true

    # ── Crew shared spaces ────────────────────────────────────────────────────

    - path: /crews/researchers/shared
      provider: s3
      config:
        bucket: avix-shared-prod
        prefix: crews/researchers/
        region: us-east-1
        auth: iam-role
      options:
        maxSizeGb: 100

    # ── Secrets store — configured via KernelConfig.secrets, not fstab ───────
    # The secrets store uses its own provider config in kernel.yaml.
    # It is listed here for reference only; fstab entries for /secrets/ are ignored.
```

### 10.2 Mount Options Reference

|Option                   |Type  |Default|Description                                              |
|-------------------------|------|-------|---------------------------------------------------------|
|`readonly`               |bool  |`false`|Block all writes via this mount                          |
|`noexec`                 |bool  |`false`|Prevent agent binaries under this path from being spawned|
|`encrypted`              |bool  |`false`|Require encryption at rest from the provider             |
|`maxSizeGb`              |number|null   |Soft quota; kernel warns when exceeded                   |
|`syncIntervalSec`        |number|`0`    |Object store flush interval (0 = immediate)              |
|`retryPolicy.maxAttempts`|number|`3`    |I/O retry attempts on transient failure                  |
|`retryPolicy.backoffSec` |number|`2`    |Backoff between retries                                  |
|`onProviderFailure`      |string|`hold` |`hold` | `readonly` | `error`                            |

### 10.3 Provider Config Reference

#### `local`

```yaml
provider: local
config:
  root: /absolute/path/on/host
```

#### `s3`

```yaml
provider: s3
config:
  bucket: my-bucket
  prefix: optional/prefix/
  region: us-east-1
  endpoint: https://...           # optional — for S3-compatible stores (MinIO, R2 etc.)
  auth: iam-role                  # iam-role | access-key | instance-profile
  accessKeySecret: /etc/avix/secrets/s3-creds   # required if auth: access-key
```

#### `gcs`

```yaml
provider: gcs
config:
  bucket: my-bucket
  prefix: optional/prefix/
  auth: workload-identity         # workload-identity | service-account
  serviceAccountSecret: /etc/avix/secrets/gcs-sa
```

#### `azure-blob`

```yaml
provider: azure-blob
config:
  account: mystorageaccount
  container: my-container
  prefix: optional/prefix/
  auth: managed-identity          # managed-identity | connection-string | sas-token
  connectionStringSecret: /etc/avix/secrets/azure-conn
```

#### `nfs`

```yaml
provider: nfs
config:
  server: nfs.internal
  export: /exports/avix
  version: 4                      # 3 | 4 (default: 4)
  auth: sys                       # sys | krb5 | krb5i | krb5p
```

#### `git`

```yaml
provider: git
config:
  remote: git@github.com:org/repo.git
  branch: main
  auth: ssh-key                   # ssh-key | https-token
  sshKeySecret: /etc/avix/secrets/git-key
  commitMessage: "avix: auto-commit {{timestamp}}"
  commitIntervalSec: 300          # 0 = commit on every write
```

#### `memory`

```yaml
provider: memory
config:
  maxSizeMb: 512
```

-----

## Related Documents

- [Schema README](./schemas/README.md) — full schema index and resolution order
- [KernelConfig](./schemas/kernel-config.md) — secrets store configuration under `spec.secrets`
- [Defaults](./schemas/defaults.md) — layered defaults configuration
- [Limits](./schemas/limits.md) — layered limits configuration
- [Resolved](./schemas/resolved.md) — kernel-derived merged config
- [Snapshot](./schemas/snapshot.md) — agent snapshot format
- [SessionManifest](./schemas/session-manifest.md) — session format (ephemeral, in `/proc/`)
- [Crews](./schemas/crews.md) — crew membership and shared path access
