---
name: git-strict
description: Strict git workflow — add only task-related files, no global changes. Use before commits in implementation tasks.
license: MIT
---

# Skill: Strict Git Workflow

Agents MUST follow this for git add/commit to avoid touching unrelated files (e.g. no global \`git add .\`).

## Rules
* Add **only** files created/modified **as part of your specific task**.
* Ignore: logs, tmp, unrelated crates/modules, agent reports from prior tasks.
* Never \`git add .\` or \`git add crates/\` — scope to task path.

## Workflow (before report/commit)

1. **List changes**: \`git status --porcelain\`
   - Note files touched by *your* work (e.g. notification.rs for Gap D).

2. **Add specific**:
   \`\`\`bash
   git add crates/avix-client-core/src/notification.rs
   git add crates/avix-client-core/tests/notification_tests.rs  # if added
   git add .agents/reports/your-gap-report.md  # only your report
   \`\`\`

3. **Review staged**: \`git diff --staged --stat\`
   - Confirm no unrelated (e.g. no CLAUDE.md, prior gaps).
   - Unstage wrong: \`git restore --staged badfile\`.

4. **Commit scoped**:
   \`\`\`bash
   git commit -m "feat(client): gap-D notifications + tests

   - add_increases_unread_count etc.
   Signed-off-by: Grok Agent"
   \`\`\`

## Common Mistakes
* Global add touches .agents/reports/* prior gaps → revert.
* Forgetting git status → add wrong files.
* Commit w/o --staged review → noisy history.

## Verification
- \`git log --oneline -5\`: Clean, scoped msgs.
- \`git status\`: Clean working tree post-task.

Use in every implement-gap task.
