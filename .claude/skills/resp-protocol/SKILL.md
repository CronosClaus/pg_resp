---
name: resp-protocol
description: RESP2 wire protocol ground truth for pg_resp — framing rules, inline commands, error taxonomy, canonical byte-level test vectors, SET-option decision table. Consult for ANY question about how bytes on the wire must look or how a command must respond.
---
# RESP2 protocol facts

Source: vendored spec pointer `docs/refs/resp2-spec.md` (redis.io RESP protocol page,
read directly — the spec page itself is not Redis source code, bible D8 only
forbids the Redis *implementation* source) + `docs/refs/valkey-notes.md` (behavioral
ground truth for command semantics — Valkey source is BSD-3, the only implementation
we may read). pg_resp implements **RESP2 only** in v0.1; RESP3 types (`_ # , ( ! = % | ~ >`)
are out of scope — never emit them, never negotiate `HELLO 3` (see §6).

## 1. Framing — the 5 RESP2 types

First byte identifies the type. Every scalar/simple type is `<sigil><payload>\r\n`.
Aggregate types are `<sigil><count-or-length>\r\n<payload>`.

| type | sigil | encoding | example |
|---|---|---|---|
| Simple String | `+` | `+<string, no CR/LF>\r\n` | `+OK\r\n` (5 bytes) |
| Error | `-` | `-<error text, no CR/LF>\r\n` | `-ERR unknown command\r\n` |
| Integer | `:` | `:[+\|-]<digits>\r\n`, signed 64-bit range | `:1000\r\n` |
| Bulk String | `$` | `$<len>\r\n<len bytes of data>\r\n` | `$5\r\nhello\r\n` |
| Null Bulk String | `$` | `$-1\r\n` (no trailing data, no extra CRLF) | `$-1\r\n` (5 bytes) |
| Array | `*` | `*<count>\r\n<elem1>...<elemN>` | `*2\r\n$5\r\nhello\r\n$5\r\nworld\r\n` |
| Null Array | `*` | `*-1\r\n` | `*-1\r\n` (used by e.g. blocking-pop timeout; pg_resp T0 has no blocking cmds, kept for completeness) |
| Empty Array | `*` | `*0\r\n` | `*0\r\n` |
| Empty Bulk String | `$` | `$0\r\n\r\n` (len 0, then empty data, then CRLF) — **distinct from null** | `$0\r\n\r\n` (4 bytes) |

**Rules:**
- CRLF (`\r\n`, bytes `0x0D 0x0A`) terminates every line. Never bare `\n`.
- Simple strings/errors must never contain `\r` or `\n` in their payload (undefined if they do — parser may reject or truncate; pg_resp never emits one that could).
- Bulk string length is unsigned base-10; length prefix has no upper bound in the spec text but Redis default caps at 512MB (`proto-max-bulk-len`) — pg_resp should reject/error on an absurd inbound length before allocating (DoS-adjacent parse-time control, not a behavior gate but a parser-hygiene requirement).
- Arrays nest arbitrarily: element N of an array can itself be `*`, and RESP has no depth limit in-spec — pg_resp's parser must bound recursion/iteration (implementation concern, not a protocol fact, but flagged because an unbounded-depth parser is the textbook fuzz crash).
- **Null vs empty is the #1 correctness trap**: `$-1\r\n` (null/missing) vs `$0\r\n\r\n` (empty string value) are different values a client must distinguish. `GET` on a missing key returns null (`$-1\r\n`), never empty bulk.
- Clients send commands as a RESP **Array of Bulk Strings only** — never simple strings, never mixed types, in a request. Server replies may be any type.

## 2. Inline commands

If the first byte of a request is **not** `*`, treat the whole line up to CRLF as
space-separated arguments (a raw-text command), used by `telnet`/manual testing.
`redis-cli` itself uses RESP arrays once connected non-interactively, but typing
directly into a raw socket (e.g. `PING\r\n`) must work.

- Split on ASCII space (no quoting/escaping support required by the spec text — keep
  the inline parser dead simple, it exists for humans with telnet, not for programs).
- Terminated by CRLF like everything else.
- pg_resp must support this for T0 — it is a common first health-check ("can I telnet
  in and type PING") and appears in demo-app READMEs.
- Detection is unambiguous: RESP arrays always start with `*`; no valid command name
  starts with `*`, so the branch is a single byte peek.

## 3. Error taxonomy

RESP has one error *type* (`-`); Redis/Valkey convention layers a prefix word on top —
**not part of the protocol**, but clients pattern-match on it, so pg_resp must match it
exactly:

| prefix | when | pg_resp exact string |
|---|---|---|
| `ERR` | generic: unknown command, wrong arg count, syntax error, non-integer where integer expected | `ERR unknown command '<cmd>'`, `ERR wrong number of arguments for '<cmd>' command`, `ERR syntax error`, `ERR value is not an integer or out of range` |
| `WRONGTYPE` | operation against a key holding a different conceptual type | pg_resp v0.1 stores only strings (T0/T1 scope, D9) — WRONGTYPE is effectively unreachable until T3 data structures exist; do not emit it prematurely |
| `NOAUTH` | command issued before required `AUTH` when `pg_resp.password` is set | `NOAUTH Authentication required.` (Valkey wording, keep exact for client-lib string matching) |
| `NOPROTO` | client sends `HELLO` with a version pg_resp does not support | pg_resp v0.1 only speaks RESP2; if `HELLO` is stubbed at all, unsupported versions get `NOPROTO sorry, this protocol version is not supported.` |

**Rules:**
- The prefix is the first *space-delimited, uppercase* word after `-`, so it must never
  contain a space itself and must be genuinely uppercase (client libraries key off this).
- Every error is single-line (simple error type only, RESP2 has no bulk-error type)
  — never emit multi-line error text.
- Unknown command: **always** reply with the `ERR unknown command` error, **never** hang,
  **never** drop the connection (bible §3.4). This is a Phase 1 compat-matrix landmine —
  client libs probe unimplemented commands (e.g. `COMMAND`, `CLIENT INFO`) on connect and
  must get a well-formed error back, not a stall.

## 4. SET option decision table

`SET key value [EX seconds | PX milliseconds | EXAT ts | PXAT ts | KEEPTTL] [NX | XX] [GET]`

| combo | behavior |
|---|---|
| bare `SET k v` | sets value, **clears any existing TTL** (SET without KEEPTTL always resets expiry), replies `+OK\r\n` |
| `EX`/`PX`/`EXAT`/`PXAT` | sets value + TTL; mutually exclusive with each other **and** with `KEEPTTL` — combining any two is `ERR syntax error` |
| `KEEPTTL` | sets value, preserves existing TTL if any (no-op TTL-wise on a key with none) |
| `NX` | only set if key does not exist; if it exists, no-op, reply `$-1\r\n` (nil) instead of `+OK\r\n` — **not an error** |
| `XX` | only set if key already exists; if missing, no-op, reply `$-1\r\n` (nil) |
| `NX` + `XX` together | mutually exclusive — `ERR syntax error` |
| `GET` | return the *old* value (or nil if none) instead of `+OK\r\n`, **and still perform the set** unless combined with a failed `NX`/`XX` guard, in which case return old value (or nil) and skip the write |
| `GET` on a non-string old value | would be `WRONGTYPE` in full Redis (unreachable in pg_resp v0.1 per §3 above) |
| expired-but-not-yet-swept key as the "existing" check for NX/XX | must be treated as **not existing** (lazy-expiry-on-access semantics, bible §3.5) |

Confirm exact edge-case wording (e.g. does `EX 0` / negative TTL error immediately vs
silently expire) against `docs/refs/valkey-notes.md` once written — flagged as a
verify-not-assume item, not guessed here.

## 5. TTL/EXPIRE return-value contract

| command | key missing | key exists, no TTL | key exists, has TTL |
|---|---|---|---|
| `TTL` | `:-2\r\n` | `:-1\r\n` | `:<seconds>\r\n` |
| `PTTL` | `:-2\r\n` | `:-1\r\n` | `:<ms>\r\n` |
| `EXPIRE` | `:0\r\n` (no-op) | `:1\r\n` (TTL set) | `:1\r\n` (TTL replaced) |

`-2` vs `-1` is a frequently-flubbed-by-memory pair — **missing key is -2, key-with-no-expiry
is -1** (not the reverse). Verify against valkey-notes.md; treat as unconfirmed-by-source
until that digest lands (recorded here from spec/changelog knowledge, not yet source-checked
per bible §7's "verify, don't assume" discipline).

## 6. Client handshake probes — minimal non-breaking stubs

Real client libraries (redis-py, node-redis, go-redis, jedis) send discovery commands on
connect before the app ever issues a real command. pg_resp must answer all of these
without hanging or erroring in a way the client library treats as fatal:

| probe | typical sender | minimal pg_resp response |
|---|---|---|
| `HELLO` (no args or `HELLO 2`) | some clients probe RESP3 support even when they'll fall back | if pg_resp stubs `HELLO` at all: reply `NOPROTO` for `HELLO 3`+, or simply treat `HELLO` as unknown-command (`ERR unknown command`) and rely on the client's RESP2 fallback path — **do not** implement a real RESP3 map reply in v0.1 |
| `CLIENT SETINFO lib-name/lib-ver ...` | redis-py, node-redis on connect | reply `+OK\r\n` unconditionally (bible §3.4: `CLIENT (subset)`) |
| `CLIENT GETNAME` / `CLIENT SETNAME` | various | `GETNAME` → nil if unset; `SETNAME` → `+OK\r\n` (can be a no-op store) |
| `COMMAND DOCS` / `COMMAND COUNT` / bare `COMMAND` | go-redis, jedis introspect the command table | bible §3.4 says `COMMAND (stub)` — safest minimal stub is an **empty array** (`*0\r\n`) for `COMMAND`/`COMMAND DOCS`, `:0\r\n` for `COMMAND COUNT`; never error, some clients gate features on this not erroring |
| `INFO` (bare or with a section arg) | connection-pool health checks | bible §3.4: `INFO (subset)` — a minimal bulk string with a few real `# Server`/`# Memory` lines is safer than empty, since some clients regex-parse specific fields (e.g. `redis_version:`) before proceeding |
| `SELECT 0` | most clients select db 0 by default | `+OK\r\n`; `SELECT` with any other db number → error (bible §3.4: "accept db 0 only") |
| `PING` (as a connection-pool keepalive, not user-invoked) | connection pool implementations | `+PONG\r\n`, or `+<message>\r\n` if `PING message` given (echo form) |

Exact byte sequences these clients send should be confirmed empirically in Phase 1's
compat-matrix work (dockerized 5-client harness) — this table is the pre-registered
expectation to debug against, not a substitute for running the real clients.

## 7. Canonical byte-level test vectors (T0 commands)

Format: `request bytes → response bytes`. `\r\n` written literally for readability;
actual bytes are `0x0D 0x0A`. These are the parser/handler unit-test fixtures — keep
them byte-exact in `resp-proto`'s test suite.

**Connection basics**
1. `PING` (inline) → `+PONG\r\n`
2. `*1\r\n$4\r\nPING\r\n` → `+PONG\r\n`
3. `*2\r\n$4\r\nPING\r\n$5\r\nhello\r\n` → `$5\r\nhello\r\n` (PING with message echoes as bulk string, not simple string)
4. `*2\r\n$4\r\nECHO\r\n$5\r\nhello\r\n` → `$5\r\nhello\r\n`
5. unknown command `*1\r\n$4\r\nFOOX\r\n` → `-ERR unknown command 'FOOX'\r\n` (never hang/disconnect)

**GET/SET core**
6. `SET k v` then `GET k` → `+OK\r\n` then `$1\r\nv\r\n`
7. `GET missing` (key never set) → `$-1\r\n`
8. `SET k ""` (empty value) then `GET k` → `+OK\r\n` then `$0\r\n\r\n` (empty, not null — trap from §1)
9. `SET k v1` then `SET k v2` then `GET k` → `+OK\r\n`, `+OK\r\n`, `$2\r\nv2\r\n` (plain SET always overwrites)
10. `SET k v EX 10` then `TTL k` → `+OK\r\n` then `:10\r\n` (approx, ≤10 immediately after)
11. `SET k v NX` on fresh key → `+OK\r\n`
12. `SET k v NX` on existing key → `$-1\r\n` (nil, not error — decision table §4)
13. `SET k v XX` on missing key → `$-1\r\n`
14. `SET k v XX` on existing key → `+OK\r\n`
15. `SET k v NX XX` (both) → `-ERR syntax error\r\n`
16. `SET k v EX 10 PX 5000` (both TTL forms) → `-ERR syntax error\r\n`
17. `SET k old` then `SET k new GET` → `+OK\r\n` then `$3\r\nold\r\n` (GET returns prior value, still sets)
18. `SET missing v GET` (no prior value) → `$-1\r\n` (nil old value, still sets new)
19. `SET k v EX 10` then `SET k v2 KEEPTTL` then `TTL k` → `+OK\r\n`, `+OK\r\n`, `:10\r\n` (approx — TTL preserved)

**DEL/EXISTS**
20. `DEL k` (k exists) → `:1\r\n`
21. `DEL k` (k missing) → `:0\r\n`
22. `DEL k1 k2 k3` (2 of 3 exist) → `:2\r\n` (count of keys actually removed)
23. `EXISTS k` (exists) → `:1\r\n`; (missing) → `:0\r\n`
24. `EXISTS k k k` (same existing key 3×) → `:3\r\n` (counts repeats, not distinct keys)

**TTL family**
25. `TTL missing` → `:-2\r\n`
26. `SET k v` (no EX) then `TTL k` → `:-1\r\n` (exists, no expiry)
27. `EXPIRE missing 10` → `:0\r\n`
28. `SET k v` then `EXPIRE k 10` → `:1\r\n`
29. `PTTL missing` → `:-2\r\n`

**INCR/DECR family**
30. `INCR newkey` (missing key, treated as 0) → `:1\r\n`
31. `SET k 10` then `INCR k` → `+OK\r\n` then `:11\r\n`
32. `SET k notanumber` then `INCR k` → `-ERR value is not an integer or out of range\r\n`
33. `SET k 5` then `INCRBY k 3` → `+OK\r\n` then `:8\r\n`
34. `SET k 5` then `DECR k` → `+OK\r\n` then `:4\r\n`

**MGET/MSET**
35. `MSET k1 v1 k2 v2` → `+OK\r\n`
36. `MGET k1 missing k2` → `*3\r\n$2\r\nv1\r\n$-1\r\n$2\r\nv2\r\n` (nil for missing, in-place, array shape preserved)
37. `MSET k1 v1 k2` (odd arg count) → `-ERR wrong number of arguments for 'mset' command\r\n`

**Inline edge case**
38. bare inline `EXISTS somekey` (spec's own example) → `:0\r\n` on fresh store

Numbers 30/31/32 need confirmation against valkey-notes.md for the *exact* wording of
the non-integer error and whether a fresh `INCR` on a missing key is universally treated
as starting from 0 (believed yes — Redis/Valkey documented behavior — mark CONFIRMED
once the digest is cross-checked, not before).

## 8. Traps

- Null (`$-1`) vs empty (`$0\r\n\r\n`) bulk string — the single most common
  first-implementation bug (§1).
- `-2`/`-1` TTL ordering is easy to transpose from memory — always verify, never assume (§5).
- `SET ... NX` failing is a **nil reply, not an error** — a naive implementation is tempted
  to error here; don't.
- Inline commands must be detected before attempting array parsing (peek first byte for `*`).
- Unbounded array-nesting / bulk-length parsing is the textbook fuzz-crash surface — cap
  recursion depth and reject absurd length prefixes at parse time, before allocating.
- Never implement RESP3 types or `HELLO 3` upgrade in v0.1 — scope creep vector, and every
  RESP3 sigil (`_ # , ( ! = % | ~ >`) is explicitly out of bible §3.4's T0–T2 scope.
