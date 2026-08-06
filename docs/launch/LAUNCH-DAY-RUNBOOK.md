# Launch-day runbook — for the human to execute

One page. Written to be followed, not read. Nothing here is automated and nothing
here should be delegated: every step is a judgement call or a posting action.

## Calendar

**T-0 = Tuesday 11 August 2026, thread window from ~14:00 UTC.**

| when | date | what |
|---|---|---|
| T-5 | Thu 6 Aug | pgrx issues filed ([#2365](https://github.com/pgcentralfoundation/pgrx/issues/2365), [#2366](https://github.com/pgcentralfoundation/pgrx/issues/2366)) — **done**, and now aging |
| T-1 | **Mon 10 Aug** | the T-1 day block below |
| T-0 | **Tue 11 Aug, from ~14:00 UTC** | Release object → PGXN upload → HN |
| T+1 | Wed 12 Aug | r/PostgreSQL |
| T+2 | Thu 13 Aug | r/rust |

**If you prefer Wednesday 12 August instead, every row shifts +1 day** — T-1 becomes
Tue 11 Aug, and r/rust lands Fri 14 Aug, which is the weakest slot of the week. That
is the one argument for Tuesday: it keeps both Reddit follow-ups inside the working
week.

14:00 UTC is 10:00 US Eastern / 07:00 Pacific — HN's front page is decided largely by
US morning traffic, and you need to be awake and free for the two hours after.

## T-minus checklist

Owner column is not decoration — **H** items need your account, your judgement or
your machine, and cannot be delegated. **A** items are verification the agent can run
and report on.

### T-1 day — Mon 10 Aug

| ✓ | owner | item |
|---|---|---|
| ☐ | **A** | Full regression sweep re-run on the tag: fast loop, `cargo pgrx test pg18`, compat (5 clients, positive counts), differential oracle |
| ☐ | **A** | Public-facing pass: no absolute `/home` paths, no stale `TODO`, no `DRAFT`/`PENDING HUMAN REVIEW` banners, no inlined-command CI guards |
| ☐ | **A** | Cold-pull the published `0.1.0` and run the three README commands end to end |
| ☐ | **H** | **Contributor-graph check: the `claudiucarare` chip must have cache-flipped to `CronosClaus`.** GitHub's contributor cache lags a history rewrite by hours to days. **If it is still stale at T-1, escalate to me — do not announce with a work-email chip visible on the repo landing page.** |
| ☐ | **H** | Record the demo clip per [`DEMO-SCRIPT.md`](DEMO-SCRIPT.md); agent reviews the `.cast` timing before it embeds |
| ☐ | **H** | Read the Release notes **and the launch post** (`LAUNCH-POST.md`) end to end as an outsider would |
| ☐ | **H** | Check both pgrx issues for maintainer replies — if either was closed as intended-behaviour, the launch post's wording needs adjusting before it goes out |
| ☑ | **H** | ~~File the two pgrx issues~~ **DONE Thu 6 Aug** — [#2365](https://github.com/pgcentralfoundation/pgrx/issues/2365), [#2366](https://github.com/pgcentralfoundation/pgrx/issues/2366), recorded in `docs/upstream/README.md` and linked from the launch post |
| ☐ | **A** | Validate `META.json` against the **official** PGXN Meta schema (fetch it, or use the `pgxn_meta` validator). Structural validation is already done; this closes it. **No live-fire discovery at T-0** |

### T-1 hour — Tue 11 Aug, ~13:00 UTC

| ✓ | owner | item |
|---|---|---|
| ☐ | **H** | `docker rmi ghcr.io/cronosclaus/pg_resp:0.1.0`, then run the three README commands yourself. **If the quickstart is broken on launch day nothing else matters — it has broken once already.** |
| ☐ | **H** | Open the README on github.com and click **every** intra-repo link. Relative links resolve differently rendered than on disk |
| ☐ | **H** | Confirm the GHCR package still shows **Public** |
| ☐ | **H** | Security inbox (`ccclaudiucarare@gmail.com`) reachable, and mail reaches your phone |
| ☐ | **H** | Actions tab green — a red badge on launch day reads as abandonment |
| ☐ | **H** | Two uninterrupted hours actually available. If not, **postpone** |

### T-0 — Tue 11 Aug, from ~14:00 UTC

| ✓ | owner | item |
|---|---|---|
| ☐ | **H** | Publish the Release object (notes from [`RELEASE-NOTES-v0.1.0.md`](RELEASE-NOTES-v0.1.0.md)) |
| ☐ | **H** | **Upload `dist/pg_resp-0.1.0.zip` to PGXN.** Verify `sha256sum` matches the value in the runbook note below *before* uploading. **The PGXN upload page publishes immediately — there is no draft state**, so this is an announcement surface and belongs here, not in setup |
| ☐ | **H** | Post to HN, alone |
| ☐ | **H** | Verify the two pgrx issue URLs are recorded in `docs/upstream/README.md` — they should already be filed and aging by now |

## Posting order, and why

**1. GitHub Release object first.** Paste `RELEASE-NOTES-v0.1.0.md`. Everything else
links here, so it must exist before any link to it does.

**2. Hacker News second, and alone.** Text: [`LAUNCH-POST.md`](LAUNCH-POST.md).
Title:

> pg_resp: a Redis-protocol cache server inside a Postgres background worker

Do **not** post to Reddit in the same hour. HN's first hour decides the outcome and
you cannot answer two threads at once — and you will need to answer, because the
comments that matter will be methodology challenges.

Post when you can sit with it for **two uninterrupted hours**. If you cannot, post
tomorrow.

**3. r/PostgreSQL, then r/rust — next day, not the same day.** Different framing per
sub: PostgreSQL cares about the operational story and the trigger invalidation;
r/rust cares about pgrx and the bgworker socket architecture. Do not cross-post the
same text.

## First-hour monitoring list

| what | where | why |
|---|---|---|
| the HN thread | your submission | first-hour replies decide reach |
| GitHub issues | repo → Issues | someone will hit the container-loopback flag |
| GitHub Discussions/PRs | repo | drive-by "why not X" questions |
| **security inbox** | `ccclaudiucarare@gmail.com` | the one channel that cannot wait |
| Actions tab | repo → Actions | a red CI badge on launch day reads as abandonment |

Refresh the HN thread every ~10 minutes for the first hour. Reply to substance,
ignore taste.

## Pre-drafted holding replies

Use these as starting text, not verbatim. Adapt to the actual comment — a canned
reply reads as canned.

### Class 1 — benchmark methodology challenge

> Fair challenge, and the raw data is in the repo so you can check me. Every cell is
> 3×60s with an 8% run-to-run spread gate, one arm live at a time, and every table is
> generated from committed raw artifacts by `bench/harness/curve.py` rather than
> typed. The specific thing I'd point you at is `ENV.md` §21–§25: it documents the
> confounds I got wrong first, including a 41 ms artifact that made all four servers
> look identical and turned out to be client-side, and the fact that I couldn't
> reproduce it afterwards. If you think a specific cell is unsound, tell me which and
> I'll re-run it.

**If they are right, say so immediately and fix it in public.** That is worth more
than the benchmark.

### Class 2 — "why not just use Redis"

> You probably should, and the README says so. Redis is faster — Valkey is +19% at
> 1 KB in my own numbers, and a Redis with `io-threads 4` beats pg_resp by 1.79×.
> The pitch isn't speed, it's one fewer stateful service and cache invalidation as a
> schema property instead of an application-discipline problem. If your cache is
> already fine and your invalidation is already correct, pg_resp buys you nothing.

### Class 3 — "useless on RDS / managed Postgres"

> Correct, and it's a real limitation rather than an oversight. pg_resp needs
> `shared_preload_libraries` and a restart, so it cannot run on a managed provider
> that doesn't allow custom extensions. That rules out RDS, Cloud SQL and most
> managed offerings today. It's for self-hosted Postgres — VMs, containers,
> Kubernetes operators. I'd rather say that plainly than imply otherwise.

## Things not to do

- Do not argue about the 64 B cell where pg_resp is ~10% ahead of single-threaded
  Redis. **Concede it fast** — point at the `io-threads 4` row where Redis wins by
  1.79× and move on. That cell is the most attackable number in the project and the
  honest framing is already published.
- Do not promise features in comments. "Logged as an issue" is the whole answer.
- Do not respond to hostility. The paper trail argues better than you will.

## If something is actually broken

1. Reproduce it before believing it, and before fixing it.
2. Say so in the thread within the hour, with what you know and what you don't.
3. Fix forward — `0.1.1` with a note beats a silent force-push, and this repo's
   credibility rests on visible corrections.


## PGXN dist — verify before you upload

The upload page **publishes immediately; there is no draft state.** Treat it exactly
like pressing "post".

```
file    dist/pg_resp-0.1.0.zip
size    0.48 MB   (rebuild with `make pgxn-dist` — reproducible, verified over five builds)
sha256  18941dfcd354b6778f53f26bbe315b1f03b6f38da1396a26c0c643aef9a783a1
```

Check it before uploading, so what reaches PGXN is provably what was built:

```bash
sha256sum dist/pg_resp-0.1.0.zip
```

**What is in it, and one deliberate omission.** The zip is `git archive` of the
`v0.1.0` tag — 159 files, single top-level `pg_resp-0.1.0/`, `META.json` at root —
with **the 5.4 MB of raw benchmark artifacts removed** (291 files of memtier output,
cell summaries, CPU samples, run logs). A PGXN source distribution exists to build
the extension, not to carry a measurement archive. `bench/results/ENV.md` **is**
included, because it is the methodology the numbers depend on, and the dist carries
`bench/results/RAW-ARTIFACTS-NOT-IN-DIST.md` pointing at the tagged tree on GitHub so
nothing is quietly dropped.

`.claude/` (11 files, 112 KB) **is** included, deliberately: `CONTRIBUTING.md` cites
those skill files for the threading rules and Postgres conventions, so a dist without
them would have dangling references.

**`META.json` is not in the `v0.1.0` git tag** — PGXN was approved after tagging. It
is injected into the dist and committed to `master` for the next release. The tag was
not moved; a tag that moves is worse than a dist that carries one extra file.
