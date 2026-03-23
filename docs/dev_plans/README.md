# Development Plans

This folder is used for active development initiatives.
It may contains markdown files that will be used for dev reviews and coding
tools to help build the right solution.

Consider files in this folder temporary and can be deleted as per dev's needs.

---

## Active Plans

### Snapshot Gaps

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `snapshot-gap-A-schema.md` | Align SnapshotFile envelope: apiVersion/kind, SnapshotMetadata, SnapshotSpec with all fields, CapturedBy/Trigger enums, async SnapshotStore | High | — |
| `snapshot-gap-B-capture.md` | Snapshot capture: SIGSAVE handler writes to VFS, checksum computation, snap/save + snap/list + snap/delete syscalls, auto-snapshot task | High | Gap A |
| `snapshot-gap-C-restore.md` | Snapshot restore: checksum verify, fresh CapabilityToken, context rebuild, pending request re-issue, pipe SIGPIPE, snap/restore syscall | Medium | Gap A, Gap B |

### Recommended Build Order

```
snapshot-gap-A  →  snapshot-gap-B  →  snapshot-gap-C
```
