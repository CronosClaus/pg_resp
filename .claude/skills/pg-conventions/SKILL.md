---
name: pg-conventions
description: Postgres-community conventions for pg_resp's public surfaces — error message style, GUC naming, control file and versioned SQL scripts, docs tone, the contrib-quality reviewer checklist. Consult before writing any user-facing string, SQL object, doc page, or release artifact.
---
# PG conventions

Filled Phase 2 (catch-up from a missed Phase 1 schedule — flagged and
corrected at Phase 2 kickoff). Sourced from `docs/refs/pg_net-notes.md`
(packaging precedent) + the standard, stable Postgres error-message style
guide (community knowledge, not re-derived from a fresh /ref read — this
convention hasn't materially changed across PG versions) + bible §7/§8.

## 1. Error message style

Postgres's own style guide (`doc/src/sgml/sources.sgml` upstream, distilled
here — the convention every core/contrib error message follows):

- **Primary message**: lowercase first letter, no trailing period, no
  "please", no exclamation marks. Complete-enough-to-stand-alone, but
  terse. `"unknown command 'FOOX'"`, not `"Unknown command 'FOOX'!"` or
  `"ERR: unknown command."`.
- **`errdetail`** (optional, additional context): full sentence(s), starts
  capitalized, ends with a period. Used for "why," not "what."
- **`errhint`** (optional, actionable suggestion): also a full capitalized
  sentence ending in a period. What the user should *do* about it.
- Never blame the user ("you forgot") — state the fact ("argument missing").
- pgrx equivalents: `ereport!(ERROR, "message")` /
  `ereport!(ERROR, "message"; errdetail = "..."; errhint = "...")` map
  directly to `errmsg`/`errdetail`/`errhint` — same casing/punctuation rules
  apply to each field independently.

**This does NOT apply to RESP-wire error replies** (`-ERR ...\r\n` etc.) —
those follow the resp-protocol skill's taxonomy (Redis/Valkey convention:
uppercase error-prefix word, no period, e.g. `ERR unknown command 'FOOX'`)
since RESP clients pattern-match on that exact shape. The two conventions
happen to look similar (no trailing period) but come from different
sources and apply to different surfaces — don't conflate "RESP wire reply"
with "Postgres ereport" when writing either.

## 2. GUC naming and documentation

- Prefix every GUC `pg_resp.` (bible §3.5-3.6's own list already does this:
  `pg_resp.bind_address`, `pg_resp.port`, `pg_resp.max_memory`,
  `pg_resp.eviction`, `pg_resp.password`).
- `pg_net`'s convention (docs/refs/pg_net-notes.md): lowercase,
  underscore-separated setting name after the prefix, no abbreviations
  that aren't immediately obvious (`bind_address`, not `bind_addr`).
- Context choice signals intent to an operator reading `SHOW ALL` — **and
  must match what the extension's code actually does, not just what sounds
  right.** Originally this said `max_memory`/`eviction`/`password` should be
  `Suset` (live-changeable) since they're not startup-time socket
  parameters like `bind_address`/`port`. That turned out to be wrong:
  `GucSetting::get()` has the exact same main-thread-only runtime check as
  `log!()`/`ereport!()` (pgrx-patterns skill's trap — confirmed from
  pgrx's own source, `guc.rs`'s `get()` calls
  `thread_check::check_active_thread()`), so none of these GUCs can be
  re-read from the spawned server thread that actually uses them. In this
  architecture **every** GUC is read once, on the main thread, before the
  server thread is spawned — declaring one `Suset` would let an operator
  `SET` it and see no error, while it silently has no effect until the next
  restart. All five of pg_resp's GUCs (`bind_address`, `port`,
  `max_memory`, `eviction`, `password`) are therefore `Postmaster` context,
  matching what's actually implemented. Revisit if a future phase adds a
  SIGHUP-driven re-read on the main thread that then hands the new value to
  the server thread via a channel/atomic.
  - Never `Userset` for anything security- or capacity-relevant.
- Give every GUC a real short *and* long description string — contrib
  modules are judged on `SHOW pg_resp.foo` reading like it belongs, not on
  a placeholder string.
- Unit flags (`GucFlags::UNIT_MB` etc.) wherever a GUC is a byte/time
  quantity — `pg_resp.max_memory` should carry `UNIT_MB` so
  `SHOW pg_resp.max_memory` renders `256MB`, not a bare integer.

## 3. Extension packaging

Per `docs/refs/pg_net-notes.md`'s `pg_net.control.in` precedent:

```
comment = 'pg_resp: Redis-protocol-compatible cache inside a Postgres background worker'
default_version = '0.1.0'
relocatable = false
module_pathname = '$libdir/pg_resp'
```

- `relocatable = false` — pg_resp is a bgworker hard-wired at
  `shared_preload_libraries` time, not a per-session-relocatable set of
  SQL objects (matches pg_net's own reasoning).
- Versioned SQL scripts from `v0.1.0` onward:
  `sql/pg_resp--0.1.0.sql` (base), then
  `sql/pg_resp--0.1.0--0.1.1.sql` style upgrade scripts per bump — pg_net
  has 30+ of these, most just a few bytes for a no-op/comment bump, larger
  ones (thousands of bytes) when real functions/tables are added. Never
  edit an already-released version's base script in place; every change
  after `0.1.0` ships as a new upgrade script.
- `cargo-pgrx`'s own generated control file (`pg_resp.control`, currently
  `default_version = '@CARGO_VERSION@'`, `comment = 'pg_resp: Created by
  pgrx'`) needs its `comment` field replaced with something real before
  release — currently still the scaffold default, tracked here so it isn't
  missed at Phase 4 packaging time.

## 4. SQL object conventions (Phase 3 scope, recorded now for when it lands)

- All `resp.*` SQL functions live in schema `resp`, never `public` — bible
  §3.3/§8.
- Function naming mirrors the RESP command it wraps, lowercase:
  `resp.get(key)`, `resp.set(key, val, ttl)`, `resp.del(key)`,
  `resp.stats()`, `resp.keys(pattern)`.
- `COMMENT ON FUNCTION`/`COMMENT ON SCHEMA` for every public SQL object —
  contrib modules document objects in-database, not just in README.

## 5. Docs structure

Every doc page (this file included) follows: **overview → configuration →
functions/commands → caveats** — the same shape as a real contrib module's
documentation page. `docs/ops.md` (drafted Phase 2) follows this; extend
it, don't fork a second ops doc.

## 6. Contrib-quality reviewer checklist (bible §8, actionable)

Run this before any release tag, and periodically during development:

- [ ] `pg_resp.control` + versioned `sql/pg_resp--X.Y.Z.sql` + upgrade
      scripts present; `CREATE EXTENSION pg_resp` / `DROP EXTENSION` clean
- [ ] Every GUC under `pg_resp.` prefix, registered with units/ranges/docs;
      `SHOW ALL` output reads as native, not bolted-on
- [ ] Error messages follow §1 above via pgrx's `ereport!` equivalents
- [ ] SQL objects in schema `resp`, zero pollution of `public`
- [ ] Docs structured overview → configuration → functions → caveats
- [ ] Supported-versions policy stated (current PG + two back) and
      CI-enforced
- [ ] `SECURITY.md` with a real contact; bind-address and
      superuser-install caveats stated without spin (see `docs/ops.md`'s
      blast-radius section for the bar this should clear)
- [ ] Semver + CHANGELOG (keep-a-changelog format); DCO sign-offs;
      rustfmt+clippy+cargo-deny green in CI
- [ ] No `unsafe` without a `// SAFETY:` comment; FFI concentrated in one
      module, auditable in one sitting
