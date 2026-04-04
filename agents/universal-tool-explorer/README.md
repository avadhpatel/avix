# Universal Tool Explorer Agent

The first official demonstration agent for Avix.

## Purpose

This agent proves that a single agent can:
- Dynamically discover every tool and syscall in the system
- Use the new `workspace.svc` safely
- Trigger structured history (live snapshots + FileDiff parts)
- Integrate with Sessions and reach the Idle state cleanly

## Installation (once packaging is implemented)

```bash
avix agent install universal-tool-explorer
Or manually (during development):
cp -r agents/universal-tool-explorer /bin/universal-tool-explorer
# or to /users/<username>/bin/universal-tool-explorer
```
## Usage

```bash
# Create a session first (recommended)
avix session create --title "Universal Explorer Demo" --goal "Test all Avix capabilities"

# Spawn the agent
avix agent spawn universal-tool-explorer --session <session_id>

# Watch live progress
avix session get <session_id> --watch
```

After the agent finishes, you will find:
- A new project under `/users/<username>/workspace/universal-exploration-.../`
- `report.md` with full tool exploration
- Rich structured history with FileDiff parts

## Development

Edit `system-prompt.md` and `manifest.yaml` as needed.
The agent is designed to be the canonical integration test for new tools and services.

Created: 2026-04-03
Version: 0.1.0
