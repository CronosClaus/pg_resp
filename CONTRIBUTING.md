# Contributing to pg_resp

Thanks for looking. pg_resp is small and opinionated; this file exists so you
can tell in one screen whether a change will be accepted before you write it.

## The one rule that is not negotiable

**Never open, read, copy from, or consult Redis source code.** Not to check a
behaviour, not to settle an argument, not "just to look".

Redis 7.4 and later are RSALv2/SSPL-licensed. pg_resp is under the
[PostgreSQL License](LICENSE), and that grant is the entire trust proposition
of the project — a PostgreSQL extension that a company cannot legally vendor is
worth nothing. Contaminating it is not a style problem, it is fatal, and it is
not something a later cleanup can undo.

The behavioural reference is:

- **Valkey** (BSD-3-Clause) — `docs/refs/valkey-notes.md`
- the **public RESP specification** — `docs/refs/resp2-spec.md`

Both are already digested in-repo, so in practice you should not need to open
either clone. If a divergence from Redis's *documented* behaviour matters,
cite the docs or a black-box observation against a running server, never the
source. This is decision **D8** in `project-bible.md`.

A pull request that cites Redis source will be closed, not rewritten.

## Certificate of origin

Every commit must carry a `Signed-off-by:` line asserting the
[Developer Certificate of Origin](https://developercertificate.org/):

```
git commit -s -m "your message"
```

By signing off you state that you wrote the contribution, or have the right to
submit it under the PostgreSQL License. Given the rule above, please do not
sign off on anything whose provenance you are unsure of.

## Getting a working tree

pg_resp is a Rust extension built with [pgrx](https://github.com/pgcentralfoundation/pgrx),
pinned to `0.19.2`. PostgreSQL 16, 17 and 18 are supported.

```bash
cargo install cargo-pgrx --locked --version 0.19.2
cargo pgrx init --pg18 download     # builds a private PG 18 under ~/.pgrx
```

Two test loops, and the distinction matters because one is 20 seconds and the
other crosses the FFI boundary:

```bash
# fast loop — pure Rust, no PostgreSQL involved
cargo test -p resp-proto -p resp-store -p resp-client

# slow loop — real postmaster, real extension, real background worker
cargo pgrx test pg18
```

Before opening a pull request:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

There is also a dockerised five-client compatibility matrix
(`make compat`: redis-cli, redis-py, node-redis, go-redis, jedis) and a
differential oracle against Valkey (`tests/differential`). Run them for any
change to the RESP parser or to command semantics — a change that passes the
unit tests and breaks a real client is the failure mode those exist to catch.

## What lands, and what does not

**Scope.** v0.1 is a cache: strings, TTLs, eviction, and the SQL/trigger
invalidation surface. Hashes, lists, sorted sets, pub/sub, RESP3, MULTI/EXEC
and TLS are all deliberately out of scope, not overlooked — see the v0.2
backlog in `project-bible.md` §5. A pull request adding a data structure will
be politely declined regardless of quality.

**Threading.** No PostgreSQL function may be called from any thread other than
the main background-worker thread — no `ereport!`, no `log!`, no
`GucSetting::get()`, no SPI, nothing. This is decision **D4** and it is the
rule that keeps the database alive rather than a preference. pgrx enforces some
of it at runtime; the rest is on review. See
`.claude/skills/pgrx-patterns/SKILL.md` for the forbidden list.

**Postgres conventions.** Error messages, GUC naming, the control file and
versioned SQL scripts follow PostgreSQL's own conventions, distilled in
`.claude/skills/pg-conventions/SKILL.md`. Primary error messages are lowercase
with no trailing period; `errdetail`/`errhint` are full sentences. RESP wire
errors follow the Redis/Valkey wire convention instead — the two look similar
and come from different places, so check which surface you are writing for.

**Unsafe code.** Every `unsafe` block needs a `// SAFETY:` comment saying what
invariant makes it sound. FFI stays concentrated where it already is, so an
auditor can read it in one sitting.

**Benchmarks.** If a change claims a performance effect, it ships with the raw
harness artifact and the exact rerun command (`bench/results/`). Numbers
without a runnable verification path are treated as fabricated — see
`bench/results/ENV.md` for the methodology and the acceptance criterion.
Losses get published too; that is the point.

## Reporting bugs

Include the PostgreSQL major version, the pg_resp version, your `pg_resp.*`
GUC settings, and — for a wire-protocol issue — the actual bytes, from
`redis-cli --no-raw` or a packet capture. "Client X fails" is hard to act on;
"client X sends this and gets these bytes back" is usually a fix within the
hour.

Security issues go to the address in [`SECURITY.md`](SECURITY.md), not to the
issue tracker.
