# KernelConfig

← [Back to Schema Index](./README.md)

**Kind:** `KernelConfig`  
**Location:** `/etc/avix/kernel.yaml`  
**Direction:** Config (static)

Master configuration for the Avix kernel. Reload with `avix reload` — no restart
required for most fields. The `ipc` section and `models.kernel` require a full restart.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: KernelConfig
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  scheduler:
    algorithm: priority_deadline   # priority_deadline | round_robin | fifo
    tickMs: 100
    preemption: true               # allow kernel to pause lower-priority agents
    maxConcurrentAgents: 50

  memory:
    defaultContextLimit: 200000    # tokens; per-agent unless overridden in manifest
    evictionPolicy: lru_salience   # lru | lru_salience | manual
    maxEpisodicRetentionDays: 30
    sharedMemoryPath: /shared/

  ipc:
    transport: unix-socket         # unix-socket | grpc (future) — requires restart to change
    socketPath: /var/run/avix/kernel.sock
    maxMessageBytes: 65536
    timeoutMs: 5000

  safety:
    policyEngine: enabled
    hilOnEscalation: true          # pause and surface to human when escalation detected
    maxToolChainLength: 10
    blockedToolChains:
      - pattern: "email + code_exec"
        reason: high risk of data exfiltration
      - pattern: "db + web"
        reason: potential data leak to external endpoints

  models:
    default: claude-sonnet-4       # used for agent spawns lacking a modelPreference
    kernel: claude-opus-4          # used for kernel-internal reasoning — requires restart to change
    fallback: claude-haiku-4       # used when quota is near limit or primary unavailable
    temperature: 0.7

  observability:
    logLevel: info                 # debug | info | warn | error
    logPath: /var/log/avix/kernel.log
    metricsEnabled: true
    metricsPath: /var/log/avix/metrics/
    traceEnabled: false            # structured trace per agent turn; high storage cost

  secrets:
    algorithm: aes-256-gcm         # aes-256-gcm | chacha20-poly1305

    masterKey:
      source: passphrase           # passphrase | key-file | env | kms-aws | kms-gcp | kms-azure | kms-vault
      kdfAlgorithm: argon2id       # key derivation function (passphrase source only)
      kdfMemoryMb: 64
      kdfIterations: 3
      # For key-file:   keyFile: /mnt/keyvolume/avix-master.key
      # For env:        envVar: AVIX_MASTER_KEY
      # For kms-aws:    kmsKeyId: arn:aws:kms:us-east-1:123:key/abc
      #                 encryptedDataKey: /etc/avix/secrets/master-key.enc
      # For kms-vault:  vaultAddr: https://vault.internal:8200
      #                 transitKeyName: avix-master

    store:
      path: /secrets               # root path for encrypted blobs on disk
      provider: local              # local | s3 | gcs | azure-blob — same providers as mounts
      # For non-local providers, add a config: block matching fstab provider config

    audit:
      enabled: true
      logPath: /var/log/avix/secrets-audit.log
      logReads: true               # log every secret read (pid, username, secret name, timestamp)
      logWrites: true              # log every secret write
```

-----

## Reload Behaviour

|Section            |Requires restart?                          |
|-------------------|-------------------------------------------|
|`scheduler`        |No                                         |
|`memory`           |No                                         |
|`ipc`              |**Yes**                                    |
|`safety`           |No                                         |
|`models.default`   |No                                         |
|`models.kernel`    |**Yes**                                    |
|`models.fallback`  |No                                         |
|`observability`    |No                                         |
|`secrets.masterKey`|**Yes** — master key is loaded once at boot|
|`secrets.store`    |**Yes**                                    |
|`secrets.audit`    |No                                         |

-----

## Related

- [Signal](./signal.md) — `safety.hilOnEscalation` triggers `SIGPAUSE` + `SIGESCALATE`
- [ResourceRequest](./resource-request.md) — `safety.blockedToolChains` affects grant decisions
- [AgentManifest](./agent-manifest.md) — `models.default` fills in when manifest omits `modelPreference`
- [Filesystem — Secrets Store](../filesystem.md#6-secrets-store) — full secrets model and CLI reference

-----

## Field Defaults

|Field                            |Default            |Notes            |
|---------------------------------|-------------------|-----------------|
|`scheduler.algorithm`            |`priority_deadline`|                 |
|`scheduler.tickMs`               |`100`              |                 |
|`scheduler.preemption`           |`true`             |                 |
|`scheduler.maxConcurrentAgents`  |`50`               |                 |
|`memory.defaultContextLimit`     |`200000`           |Tokens per agent |
|`memory.evictionPolicy`          |`lru_salience`     |                 |
|`memory.maxEpisodicRetentionDays`|`30`               |                 |
|`ipc.transport`                  |`unix-socket`      |                 |
|`ipc.maxMessageBytes`            |`65536`            |                 |
|`ipc.timeoutMs`                  |`5000`             |                 |
|`safety.policyEngine`            |`enabled`          |                 |
|`safety.hilOnEscalation`         |`true`             |                 |
|`safety.maxToolChainLength`      |`10`               |                 |
|`models.temperature`             |`0.7`              |                 |
|`observability.logLevel`         |`info`             |                 |
|`observability.metricsEnabled`   |`true`             |                 |
|`observability.traceEnabled`     |`false`            |High storage cost|

KernelConfig is not subject to the user/crew defaults resolution chain — it is a
system-level file edited directly by root. System defaults are compiled in and documented
here for reference.
