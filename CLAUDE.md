# pg_resp — agent entrypoint

**Current phase: 4** (update this line at every phase boundary via /phase-report)
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

## Standing rule: everything here is public-facing

**Every artifact in this repository is public-facing material** — the phase
reports, `bench/results/ENV.md`, the docs, these skills, and this file. The repo is
public and the paper trail is deliberately part of the credential.

Write everything as if it will be quoted in a comment thread, because it will be.
**This changes the tone nowhere.** The honesty is the value: the withdrawn
measurements, the refuted hypotheses and the corrections are the most credible
content in here, and dressing them up would destroy exactly what makes them worth
reading. It adds one duty rather than a filter.

**The duty: a PUBLIC-FACING PASS before the `v0.1.0` tag.**

- sweep for stale personal artifacts — absolute `/home/...` paths in docs,
  machine-specific hostnames, leftover `TODO`/`FIXME`
- remove the `DRAFT` / `PENDING HUMAN REVIEW` banners that exist only because
  review was outstanding (`README.md`, `docs/BENCHMARKS.md`,
  `demos/3-rate-limiter/README.md`) — **this is an explicit W14 checklist item, not
  a judgement call at tag time**
- verify every intra-repo link resolves as GitHub renders it (relative paths from
  the file's own directory, not from the repo root)
- keep every `PENDING` that is still genuinely unmeasured; the pass removes stale
  *banners*, never inconvenient *facts*

## Standing rule: a guard must EXECUTE the thing it guards

A CI job that checks a documented procedure must **run the committed script or the
documented command**, never carry its own copy of it. Two definitions of the same
commands is how a green CI and a broken user path coexist: the `quickstart-check`
job kept an inline copy of the three quickstart commands, that copy received the
container-loopback fix, `scripts/g1-quickstart-timing.sh` did not, and the job
stayed green while a real fresh host failed G1 at command 3.

The divergence is invisible by construction — both copies look correct in
isolation. The only structural defence is one definition.

**Sweep for this pattern once during the pre-tag public-facing pass:** any workflow
step that inlines commands which also exist as a committed script or documented
procedure gets pointed at the committed version instead.

## Standing rule: absence of a failure token is not a pass

**A check passes only on POSITIVE evidence** — a count, an explicit verdict, the
expected output. "No `FAIL` in the log" is not evidence; it is the absence of one
kind of evidence.

**Any pipeline containing `|| true`, `2>/dev/null`, or output filtering must prove
it can still fail.** If you cannot say how the check would report a failure, it is
not a check.

This is the **same structural disease** as the CI-guard-copy rule above: both
produce things that look like verification but cannot fail. The guard-copy bug made
a green job that tested a different artifact; this one makes a green log that tested
nothing. Neither is caught by reading the check — only by asking what its failure
would look like.

It nearly shipped a tag. `make compat` runs each of five clients with `|| true`, and
the first pass was filtered to a grep that dropped the per-client verdicts, so a
client failure would have surfaced in neither the exit code nor the log. It was
re-run with full capture and passed on real counts (27/27, 30/30, 28/28, 31/31, and
redis-cli's "all checks passed") — but the first result was indistinguishable from
a pass and was not one.

**Sweep the Makefile and scripts for `|| true` + filtering combinations during the
pre-announce pass.**

## Iron rules
1. PG FFI from the **main bgworker thread only**. Never from server/loop threads.
2. Never read `/ref` trees directly once digests exist; use `docs/refs/*-notes.md`. Digests are created once in Phase 0 and updated only when a gate failure traces to a wrong fact.
3. A gate fails twice for the same cause → stop patching, write it into the report, re-read the relevant digest, fix the model of the problem.
4. **Never open Redis source.** Behavioral reference is Valkey + the RESP spec only (bible D8 — licensing, non-negotiable).
5. Bulk/noisy work (compat runs, bench sweeps, log triage, /ref digestion) goes to the subagents in `.claude/agents/`, not the main thread.
6. Benchmarks: publish every number, including losses (bible §0.5).
7. **Subagent deliverables containing numbers MUST ship the committed raw artifact** (`bench/results/`, logs, harness output) **and the exact rerun command. Numbers without artifacts are treated as fabricated.** A claim of "complete and correct" requires a runnable verification path, executed by the main thread, before acceptance. Origin: the Phase 3 demo-2 incident — a subagent reported "infrastructure is complete and correct" with a table of *expected* measurements it had never run, a misattributed root cause (blamed WSL2 DNS; actually a stale compose network), and an arm that faked the feature under test by calling `cache.Del()` from application code to "simulate" a trigger it never created. Every one of those survived a plausible-sounding summary and died on first contact with an actual run.
