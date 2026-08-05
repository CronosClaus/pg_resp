# pg_resp — agent entrypoint

**Current phase: 3** (update this line at every phase boundary via /phase-report)
Spec: `project-bible.md`. §0 agent contract is **binding**. Method: `docs/RUNBOOK.md`.

## Session boot ritual
Read, in order, nothing else by default:
1. `project-bible.md` §0 (contract) + the section for the current phase (§5)
2. The latest `reports/phase*.md` (it is the memory of everything before this session)
3. The skill relevant to the task (pointer map below)

## Commands
- fast loop (no Postgres needed): `cargo test -p resp-proto -p resp-store -p resp-client`
- slow loop (crosses FFI): `cargo pgrx test pg18`
- client compat matrix: `make compat`
- reference clones: `bash scripts/clone-refs.sh` (Phase 0, once)

## Pointer map
- RESP framing / command semantics → `.claude/skills/resp-protocol`
- bgworker / GUC / shmem / xact-callback recipes → `.claude/skills/pgrx-patterns`
- error style, naming, control file, docs tone → `.claude/skills/pg-conventions`
- memtier flags, bench arms, env checklist → `.claude/skills/bench-harness`

## Iron rules
1. PG FFI from the **main bgworker thread only**. Never from server/loop threads.
2. Never read `/ref` trees directly once digests exist; use `docs/refs/*-notes.md`. Digests are created once in Phase 0 and updated only when a gate failure traces to a wrong fact.
3. A gate fails twice for the same cause → stop patching, write it into the report, re-read the relevant digest, fix the model of the problem.
4. **Never open Redis source.** Behavioral reference is Valkey + the RESP spec only (bible D8 — licensing, non-negotiable).
5. Bulk/noisy work (compat runs, bench sweeps, log triage, /ref digestion) goes to the subagents in `.claude/agents/`, not the main thread.
6. Benchmarks: publish every number, including losses (bible §0.5).
