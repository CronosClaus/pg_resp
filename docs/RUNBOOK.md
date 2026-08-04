# Runbook — how this repo is driven with Claude Code

## Model routing
| role | model | set where |
|---|---|---|
| main thread default | sonnet (Sonnet 5) | .claude/settings.json |
| phase planning | opus (Opus 5) | /kickoff command frontmatter |
| subagent floor | haiku (Haiku 4.5) | CLAUDE_CODE_SUBAGENT_MODEL in settings env |
| escalation | /model opus | manual, on trigger below |

Escalation trigger (bible §7): a gate fails twice for the same cause → /model opus,
fix the model of the problem, /model sonnet, continue. Escalate the model, never
the effort setting. Fable 5 is not part of this project's routing.

## Per-phase ritual
1. Fresh session (context is empty or /clear'ed).
2. `/kickoff N` — plans on Opus from bible §5 + last report. Review, approve.
3. `/model sonnet` — execute. Fast loops (PG-free crates) before slow loops (pgrx).
4. Bulk/noisy work goes to subagents: compat-runner, bench-runner, ref-digester,
   differential-triager. They run on their pinned models automatically.
5. `/phase-report N` — writes the handoff report, updates CLAUDE.md phase line, commits.
6. `/clear`. Never rely on auto-compact mid-phase; reports are the memory.

## Quota hygiene
- /usage (subscription) or /cost (API) mid-phase if burn feels hot; first suspect is
  a subagent inheriting the main model — check its `model:` frontmatter survived.
- Sonnet 5 intro pricing runs through Aug 31, 2026 — heavy phases benefit from
  starting inside that window.

## Session boot (any session, any phase)
Read: bible §0 + current-phase §5 section + latest reports/phase*.md + the one
relevant skill. Nothing else by default. CLAUDE.md enforces this.
