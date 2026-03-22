# Avix Initial Configuration & Security Model v1

> **Purpose:** How Avix is configured before first start, how config files are secured
> at rest, and how the kernel enforces access at runtime.
> **Key decision:** Avix core never configures itself. Config is produced externally
> by `avix config init` before `avix start`.

-----

## Table of Contents

1. [Core Security Principle](#1-core-security-principle)
1. [What Needs OS-Level Protection](#2-what-needs-os-level-protection)
1. [avix config init](#3-avix-config-init)
1. [Deployment Scenarios](#4-deployment-scenarios)
1. [Master Key Storage Guide](#5-master-key-storage-guide)
1. [auth.conf Reference](#6-authconf-reference)
1. [Credential Types](#7-credential-types)
1. [Post-Setup Changes](#8-post-setup-changes)
1. [Responsibility Separation](#9-responsibility-separation)

-----

## 1. Core Security Principle

**Avix core always boots from pre-existing, valid `/etc/avix/` config files.**

If `auth.conf` is absent at boot → Avix exits with:

```
AVIX_BOOT_ERROR: /etc/avix/auth.conf not found.
Run: avix config init --root <path> before avix start.
```

There is no setup wizard, no fallback mode, no unsecured boot. This keeps the core simpler and means every running Avix instance is always authenticated.

The “setup UX” concern lives entirely in the installer layer:

- Desktop app: generates config on first launch via embedded `avix config init`
- Docker: `avix config init` runs in entrypoint before `avix start`
- CLI: user runs `avix config init` manually once
- Remote/CI: provisioning script calls `avix config init --non-interactive`

-----

## 2. What Needs OS-Level Protection

The filesystem protection model splits cleanly into two categories:

### Needs chmod 600 / OS-level protection

|File                        |Why                                                                                       |
|----------------------------|------------------------------------------------------------------------------------------|
|`AVIX_ROOT/etc/auth.conf`   |Contains argon2id / HMAC hashes. Offline cracking risk.                                   |
|`AVIX_ROOT/etc/kernel.yaml` |Contains `masterKey.source` — tells attacker where to find the key                        |
|`AVIX_ROOT/secrets/**/*.enc`|AES-256-GCM ciphertext. Useless without master key, but restrict anyway (defence in depth)|

### Does NOT need OS-level encryption (Avix-layer protection is sufficient)

|Path                      |Why                                                                              |
|--------------------------|---------------------------------------------------------------------------------|
|`AVIX_ROOT/users/`        |No secrets. Freely exportable by design. Avix token + VFS ACL protect at runtime.|
|`AVIX_ROOT/services/`     |Service assets — no credentials.                                                 |
|`AVIX_ROOT/etc/users.yaml`|Usernames, roles, quotas. No credentials.                                        |
|`AVIX_ROOT/etc/crews.yaml`|Crew config. No credentials.                                                     |
|`/proc/**`                |Ephemeral — only meaningful while Avix is running.                               |

### What an attacker gets

|Attacker has                     |Gets                      |Impact                                        |
|---------------------------------|--------------------------|----------------------------------------------|
|`auth.conf` only                 |Argon2id / HMAC hashes    |Must crack offline — computationally expensive|
|`/secrets/*.enc` only            |AES-256-GCM ciphertext    |Useless without master key                    |
|`/users/alice/workspace/`        |Agent work files          |No credentials, no auth bypass                |
|All of `/etc/avix/` + `/secrets/`|Hashes + ciphertext       |Still needs master key                        |
|Master key only                  |A key with nothing to open|Nothing                                       |
|Master key + `/secrets/`         |Decryptable agent secrets |**Significant — protect both**                |

The master key never touches disk in recommended configurations. That’s the architectural guarantee.

### Installer-set permissions

```
AVIX_ROOT/etc/           chmod 700   (only avix process user can list)
AVIX_ROOT/etc/auth.conf  chmod 600
AVIX_ROOT/etc/kernel.yaml chmod 600
AVIX_ROOT/secrets/       chmod 700
AVIX_ROOT/secrets/**     chmod 600
AVIX_ROOT/users/         chmod 755   (no secrets here — permissive)
AVIX_ROOT/services/      chmod 755
```

-----

## 3. avix config init

A pure file generator. No network calls. No Avix process needed. Runs before `avix start`.

### What it writes (atomically)

All three files are written atomically — temp file → fsync → rename. Partial writes are impossible.

```
AVIX_ROOT/etc/auth.conf      (chmod 600)
AVIX_ROOT/etc/boot.conf      (chmod 600)
AVIX_ROOT/etc/users.yaml
AVIX_ROOT/etc/kernel.yaml    (chmod 600)
```

### What it prints to stdout (never written to files)

- The generated API key: `sk-avix-<32 base62 chars>`

The caller is responsible for storing this key (OS keychain, password manager, Docker secret).

### CLI signature

```bash
avix config init [options]

Required:
  --root <path>                    AVIX_ROOT directory
  --user <username>                Admin username
  --credential-type <type>         password | api_key

Credential (one required):
  --api-key <key>                  Use provided key (or omit to generate)
  --password <value>               Plaintext (hashed immediately, never stored)

Master key source (one required):
  --master-key-source env          Read AVIX_MASTER_KEY at runtime
  --master-key-source key-file --master-key-file <path>
  --master-key-source kms-aws --kms-key-id <arn>
  --master-key-source kms-gcp --kms-key-id <id>
  --master-key-source kms-vault --vault-addr <url> --vault-key <name>

Optional:
  --mode gui | cli | headless
  --bind <addr>                    gateway bind address
  --ttl <duration>                 session TTL
  --ip-allowlist <cidr,...>        restrict this credential to source IPs
  --non-interactive                fail on missing required input
  --force                          overwrite existing auth.conf
  --dry-run                        print what would be written
```

### Idempotency

Without `--force`: if `auth.conf` exists, `avix config init` exits cleanly with no changes. Safe to run in Docker entrypoints — subsequent runs are no-ops.

With `--force`: overwrites existing config. Use for credential rotation.

-----

## 4. Deployment Scenarios

### S1 — Desktop App (GUI, “passwordless”)

The user never types a password. The app manages credentials via the OS keychain.

```
First launch:
  app calls avix config init (embedded):
    → generates api_key
    → writes auth.conf with hmac-sha256(api_key)
    → stores api_key in OS keychain (Keychain.app / GNOME Secret Service / DPAPI)
    → derives master_key = HKDF(machine_id + app_bundle_id)
    → writes kernel.yaml with masterKey.source=env

Every subsequent launch:
  app reads api_key from OS keychain
  app derives master_key fresh
  app spawns: avix start --root ~/avix-data
    with AVIX_MASTER_KEY=<derived_key> in env
  app connects ATP: POST /atp/auth/login { identity: "alice", credential: api_key }
  → receives ATPToken
  → normal operation
```

**Machine binding:** The master key derivation uses `machine_id + app_bundle_id`. If someone copies the entire `AVIX_ROOT` to another machine, the `/secrets/` blobs are undecryptable — the derivation inputs differ.

**Adding a password later:** User goes to app settings → enables password → `avix config init --force --credential-type password`. Next launch, app prompts for password instead of using keychain key.

### S2 — Docker (Automated)

Config generated before container starts. Credentials from Docker secrets or env.

**docker-compose.yml:**

```yaml
services:
  avix:
    image: avix:latest
    ports: ["7700:7700"]
    volumes: ["avix-data:/var/avix-data"]
    environment:
      AVIX_SETUP_MODE: docker
      AVIX_MASTER_KEY: ${AVIX_MASTER_KEY}
    secrets: [avix_admin_key]
    entrypoint: |
      /bin/sh -c "
        avix config init \
          --root /var/avix-data \
          --user avix-admin \
          --credential-type api_key \
          --api-key $(cat /run/secrets/avix_admin_key) \
          --master-key-source env \
          --mode headless \
          --non-interactive
        avix install --root /var/avix-data
        avix start --root /var/avix-data
      "

secrets:
  avix_admin_key:
    external: true

volumes:
  avix-data:
```

**Env var security:** `avix config init` reads `AVIX_ADMIN_API_KEY` from env, hashes it, writes the hash to `auth.conf`, then zeroes the env var. After init completes, the plaintext no longer exists in any process environment.

### S3 — Multi-User Web

Standard password setup. Admin creates users via ATP `users.create` post-startup.

```bash
avix config init \
  --root /var/avix-data \
  --user admin \
  --credential-type password \
  --master-key-source kms-aws \
  --kms-key-id arn:aws:kms:us-east-1:... \
  --mode headless \
  --bind 0.0.0.0
```

Users log in via browser at `https://avix.example.com:7700`:

```http
POST /atp/auth/login
{ "identity": "alice", "credential": "her-password" }
```

### S4 — CLI (Personal, No App)

Interactive setup once. API key or password. Key stored in shell profile or password manager.

```bash
# One-time setup
avix config init \
  --root ~/.avix \
  --user alice \
  --credential-type api_key \
  --master-key-source key-file \
  --master-key-file ~/.config/avix/master.key

# Output:
# API key generated (save this — shown once):
#   sk-avix-7f3a9b2c4d5e6f7a8b9c...
#
# Add to ~/.zshrc: export AVIX_API_KEY=sk-avix-7f3a9b2c...

avix install --root ~/.avix
avix start --root ~/.avix
avix connect  # reads AVIX_API_KEY from env automatically
```

### S5 — Remote Machine (API Credentials)

Non-interactive. API key stored in secrets manager. Optional IP allowlist.

```bash
avix config init \
  --root /var/avix-data \
  --user avix-remote \
  --credential-type api_key \
  --api-key "$AVIX_ADMIN_API_KEY" \
  --master-key-source kms-aws \
  --kms-key-id "$KMS_KEY_ARN" \
  --ip-allowlist "10.0.0.0/8,203.0.113.0/24" \
  --non-interactive
```

The IP allowlist means the HMAC hash in `auth.conf` is only usable from the specified source IPs — even if stolen, it cannot be used from an attacker’s machine.

-----

## 5. Master Key Storage Guide

```
Are you on a cloud VM with IAM?
  → kms-aws / kms-gcp / kms-azure
    Zero key material on disk. Best option if available.

Are you in Docker with a secrets manager?
  → env (AVIX_MASTER_KEY from Docker secret / Vault agent sidecar)
    Key never in image or compose file; injected at runtime.

Are you a desktop app?
  → env, derived at launch: HKDF(machine_id + app_bundle_id)
    Machine-bound. App derives fresh on every launch. Never stored.

Are you on a personal machine (CLI)?
  → key-file at ~/.config/avix/master.key (chmod 600)
    Back up to password manager or encrypted drive.

Are you on-prem with HashiCorp Vault?
  → kms-vault
    Vault AppRole or K8s auth. Token auto-renewed by sidecar.
```

-----

## 6. auth.conf Reference

```yaml
apiVersion: avix/v1
kind: AuthConfig

policy:
  session_ttl: 8h                # default session TTL
  require_tls: true              # reject plaintext connections
  failed_auth_lockout_count: 5
  failed_auth_lockout_ttl: 15m
  token_refresh_window: 5m      # push token.expiring this far in advance
  password_min_length: 12
  api_key_min_length: 32

identities:
  - name: alice
    uid: 1001
    role: admin                  # guest | user | operator | admin
    credential:
      type: api_key              # api_key | password
      key_hash: hmac-sha256:$... # for api_key
      # hash: argon2id:$...      # for password
      ip_allowlist: []           # empty = allow all; CIDR list to restrict
    mfa:                         # optional, add post-setup
      type: totp
      secret: /secrets/alice/mfa-totp.enc
```

-----

## 7. Credential Types

### API Key

- Format: `sk-avix-<32 base62 chars>` (~190 bits entropy)
- Storage: HMAC-SHA256 hash in `auth.conf`
- Generated by: `avix config init` (prints once to stdout) or `avix keygen`
- Best for: desktop app, Docker, CI/CD, remote machines

### Password

- Storage: argon2id hash in `auth.conf`
  - Parameters: `m=65536` (64MB memory), `t=3` (iterations), `p=4` (parallelism)
  - OWASP-recommended minimums as of 2025
- Best for: interactive multi-user setups, web login

### Not supported

- `credential.type: none` — removed in v3. Use API key with OS keychain for “passwordless” UX.

-----

## 8. Post-Setup Changes

All via ATP (admin role) or `avix` CLI equivalents:

|Action                         |ATP command                |CLI                                  |
|-------------------------------|---------------------------|-------------------------------------|
|Add password to API key account|`users.update` credential  |`avix passwd alice`                  |
|Rotate API key                 |`users.update` credential  |`avix keygen --user alice`           |
|Enable TOTP MFA                |`users.update` mfa         |`avix mfa enable alice`              |
|Add new user                   |`users.create`             |`avix user add bob --role user`      |
|Change role                    |`users.update` role        |`avix user set-role bob operator`    |
|Lock a user                    |`users.update` locked:true |`avix user lock bob`                 |
|Add IP allowlist               |`users.update` ip_allowlist|`avix user allow-ip alice 10.0.0.0/8`|

-----

## 9. Responsibility Separation

```
┌─────────────────────────────────────────────────────────────────┐
│  OS / Platform Layer                                            │
│  • File permissions (chmod 600 on /etc/avix/)                   │
│  • OS keychain (API key plaintext for desktop app)              │
│  • Disk encryption (LUKS / FileVault / BitLocker) — optional    │
│  • Cloud IAM / KMS (master key for cloud deployments)           │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  Installer Layer  (avix config init / desktop app / Docker)     │
│  • Generates API key, computes hash, writes auth.conf           │
│  • Configures masterKey source in kernel.yaml                   │
│  • Stores API key plaintext in OS keychain or prints once       │
│  • Sets correct file permissions                                │
│  • Injects AVIX_MASTER_KEY at process launch                    │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  Avix Core                                                      │
│  • Reads /etc/avix/ at boot; refuses to start if absent         │
│  • Loads master key from configured source into memory only     │
│  • Zeroes AVIX_MASTER_KEY env var after loading                 │
│  • Validates API key hash on every ATP message                  │
│  • Enforces VFS ACLs via memfs.svc                              │
│  • Encrypts /secrets/ blobs with master key                     │
│  • Knows nothing about how config was generated                 │
└─────────────────────────────────────────────────────────────────┘
```
