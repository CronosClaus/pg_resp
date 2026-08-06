# Runbook — how this repo is driven with Claude Code

## Model routing
| role | model | set where |
|---|---|---|
| main thread | **opus (Opus 5)** — for all of Phase 4, per amendment A.2 | .claude/settings.json |
| phase planning | opus (Opus 5) | /kickoff command frontmatter |
| bench-runner | sonnet | agent frontmatter |
| differential-triager | sonnet | agent frontmatter |
| compat-runner | haiku (Haiku 4.5) | agent frontmatter |
| ref-digester | haiku (Haiku 4.5) | agent frontmatter |
| escalation | /model opus | manual, on trigger below |

`CLAUDE_CODE_SUBAGENT_MODEL` was **removed** from settings in `4f68e86`: it
forces every subagent to one model, which silently overrode per-agent choices.
Models are pinned in each agent's own frontmatter now, which is why the table
above lists them individually.

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

## Benchmark box session protocol (Phase 4 W5-official / W6 / W10)

The published §10 numbers and the G1 quickstart timing execute on a dedicated
box, never on WSL2 (Phase 4 environment amendment). Box **creation and
destruction are human-only** — the agent never holds a provider token. The
agent's half of the contract is everything between.

### Access
- Connect as **`bench@IP` only, never root.** `Bash(ssh:*)`, `Bash(scp:*)`,
  `Bash(rsync:*)` are in the settings allow list for this (`ef951ef`).
- The box holds **no secret that outlives it**: a freshly generated throwaway
  `pg_resp.password` is created at bootstrap, never the dev literal
  (`secret123`). It may appear in `ENV.md` because the box is destroyed.

### Bootstrap
1. `git clone` the **public** repo. Nothing is hand-copied — not the tree, not a
   binary, not a config.
2. Check out the reference clones at their `docs/refs/PINS.md` commits and
   **verify the hashes**, don't assume the clone landed where intended.
3. The box is **frozen as bootstrapped**: no `apt upgrade`, no new system
   packages. Anything else you need runs in docker or userland. In practice this
   means every arm and every tool is containerised — see `ENV.md` §14.
4. Re-derive the taskset map from `lscpu` + `thread_siblings_list`
   (`ENV.md` §13). **Never port another machine's map.**
5. Attempt the `performance` governor; if `cpufreq` is not exposed, record
   "not exposed (cloud dedicated vCPU)" and proceed. **Never block a run on it.**
6. Bind every server to loopback and run `bench/harness/arms.sh lockdown`.
   Record its output in `ENV.md`.

### Running
- **Anything longer than 5 minutes runs in `tmux`**, logging to `~/logs/`, so a
  dropped SSH connection cannot kill a sweep.
- Order is fixed: **A-smoke → Stage A-official → (human review) → Stage B**.
  A-smoke is one ~10 s throwaway cell per arm, all six; its numbers are never
  published and the harness stamps them unpublishable. See `ENV.md` §18.
- One arm live at a time (`ENV.md` §8), enforced, not intended.

### Finishing — the box is not done until this is true
1. Raw results `rsync` back **and committed**. A result that exists only on a
   box about to be destroyed does not exist (iron rule 7).
2. `ENV.md` complete: topology, image digests, lockdown output, governor verdict.
3. End with an explicit **"SAFE TO DESTROY"** message listing the result commits
   present, ENV.md completeness, and confirmation that no snapshot is needed.
