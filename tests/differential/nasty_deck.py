#!/usr/bin/env python3
"""Hand-written adversarial differential deck (bible §5/§9, Phase 2 closeout).

`generate_and_compare.py`'s random_t0_stream() is a *randomized* generator,
not an *adversarial* one: it only ever emits syntactically well-formed
commands drawn from a small, ASCII-only key pool (k0-k19) and a small value
pool (empty string, "v", "hello world", "12345", "-7", 64 x's). It never
probes:
  - SET's option combinations (it does add EX and NX/XX independently at low
    probability, but never GET, never KEEPTTL, and never two mutually
    exclusive options together to check the syntax-error path)
  - integer boundaries (i64::MAX/MIN overflow/underflow)
  - EXPIRE with zero or negative TTLs
  - binary payloads (embedded CRLF, NUL, or the full 0-255 byte range) in
    either keys or values
  - the empty string as a key (only as a value)
  - wrong-arity / malformed-syntax error replies (every generated command is
    well-formed by construction)
  - GETEX's bare-call semantics (must NOT clear TTL) vs PERSIST (must clear
    it) as a deliberate side-by-side
  - glob pattern edge cases for KEYS/SCAN's MATCH (empty pattern, unterminated
    class, escaped wildcard, negated class, multi-star)

This script is the fixed, hand-written complement: every entry below is a
specific edge case chosen for a specific reason (see inline comments), run
once through the same byte-for-byte oracle harness as generate_and_compare.py
(imported from it, not reimplemented). Order matters — later entries depend
on earlier ones' state (e.g. TTL checks depend on the preceding SET/EXPIRE),
so this is a single ordered sequence, not independent commands. Results and
final counts are recorded in reports/phase2.md.

Usage:
  python3 nasty_deck.py --candidate-port 6379 --oracle-port 6380
"""
import argparse
import sys

from generate_and_compare import build_command, read_one_reply, run_stream

I64_MAX = 9223372036854775807
I64_MAX_MINUS_1 = 9223372036854775806
I64_MIN = -9223372036854775808
I64_MIN_PLUS_1 = -9223372036854775807

# Full 0-255 byte range in one blob, including NUL (0x00) and CRLF (0x0d0x0a).
ALL_BYTES = bytes(range(256))

# Each entry: (label, [command args as str|int|bytes]).
DECK = []


def add(label, *args):
    DECK.append((label, list(args)))


# --- setup: deterministic starting state ---------------------------------
add("flushall for a clean slate", "FLUSHALL")

# --- 1. SET option combinations -------------------------------------------
add("set base key for combo tests", "SET", "nd:combo", "v0")
add("set EX+NX+GET on existing key (NX blocks, GET still returns old val)",
    "SET", "nd:combo", "v1", "EX", "100", "NX", "GET")
add("get after EX+NX+GET-on-existing (value must be unchanged)", "GET", "nd:combo")
add("ttl after EX+NX+GET-on-existing (NX blocked -> no new EX applied)", "TTL", "nd:combo")
add("set EX+NX+GET on a missing key (NX succeeds, GET returns nil)",
    "SET", "nd:combo:new", "v2", "EX", "50", "NX", "GET")
add("get after EX+NX+GET-on-missing", "GET", "nd:combo:new")
add("ttl after EX+NX+GET-on-missing (NX succeeded -> EX 50 applied)", "TTL", "nd:combo:new")
add("set XX+GET on a missing key (XX blocks, GET returns nil, no key created)",
    "SET", "nd:combo:absent", "v3", "XX", "GET")
add("exists after XX+GET-on-missing (must still not exist)", "EXISTS", "nd:combo:absent")
add("set with EX then a second SET KEEPTTL (must preserve the TTL)",
    "SET", "nd:keepttl", "a", "EX", "100")
add("ttl before KEEPTTL overwrite", "TTL", "nd:keepttl")
add("overwrite value with KEEPTTL", "SET", "nd:keepttl", "b", "KEEPTTL")
add("get after KEEPTTL overwrite (value changed)", "GET", "nd:keepttl")
add("ttl after KEEPTTL overwrite (TTL preserved, not reset/cleared)", "TTL", "nd:keepttl")
add("set with EX and KEEPTTL together -> syntax error (mutually exclusive)",
    "SET", "nd:bad1", "v", "EX", "10", "KEEPTTL")
add("set with NX and XX together -> syntax error (mutually exclusive)",
    "SET", "nd:bad2", "v", "NX", "XX")
add("set with EX 0 -> invalid expire error", "SET", "nd:bad3", "v", "EX", "0")
add("set with EX -5 -> invalid expire error", "SET", "nd:bad4", "v", "EX", "-5")
add("set with PX 0 -> invalid expire error", "SET", "nd:bad5", "v", "PX", "0")
add("set with unknown option -> syntax error", "SET", "nd:bad6", "v", "BOGUS")
add("set with EX non-integer arg -> not-an-integer error", "SET", "nd:bad7", "v", "EX", "abc")
add("set with dangling EX (no number follows) -> syntax error", "SET", "nd:bad8", "v", "EX")
add("set with only one arg -> wrong-arity error", "SET", "nd:onearg")
add("set with zero args -> wrong-arity error", "SET")
add("set EXAT in the past -> sets then is immediately expired", "SET", "nd:exat", "v", "EXAT", "1")
add("get after EXAT-in-the-past (must be nil, already expired)", "GET", "nd:exat")

# --- 2. INCR/DECR at i64 boundaries ----------------------------------------
add("set i64::MAX-1 for boundary incr", "SET", "nd:incr:hi", str(I64_MAX_MINUS_1))
add("incr one below MAX succeeds, lands exactly on MAX", "INCR", "nd:incr:hi")
add("incr again at exactly MAX -> overflow error", "INCR", "nd:incr:hi")
add("incrby +1 at exactly MAX -> overflow error", "INCRBY", "nd:incr:hi", "1")
add("set i64::MIN+1 for boundary decr", "SET", "nd:decr:lo", str(I64_MIN_PLUS_1))
add("decr one above MIN succeeds, lands exactly on MIN", "DECR", "nd:decr:lo")
add("decr again at exactly MIN -> overflow error", "DECR", "nd:decr:lo")
add("decrby -1 (i.e. +1 via negation) at MIN would overflow the positive side too",
    "DECRBY", "nd:decr:lo", "-1")
add("decrby i64::MIN itself -> special negation-overflow error, any current value",
    "DECRBY", "nd:decr:special", str(I64_MIN))
add("incr on a non-numeric string value -> not-an-integer error",
    "SET", "nd:incr:nan", "hello world")
add("incr on non-numeric value, confirm error", "INCR", "nd:incr:nan")
add("incr on an empty-string value -> not-an-integer error",
    "SET", "nd:incr:empty", "")
add("incr on empty-string value, confirm error", "INCR", "nd:incr:empty")
add("incrby with a non-integer delta arg -> not-an-integer error (arg, not stored value)",
    "SET", "nd:incrby:arg", "5")
add("incrby non-integer arg, confirm error", "INCRBY", "nd:incrby:arg", "abc")
add("incr on a brand-new key starts at 1", "INCR", "nd:incr:fresh")
add("decr on a brand-new key starts at -1", "DECR", "nd:decr:fresh")

# --- 3. EXPIRE 0 and negative ----------------------------------------------
add("set a key to expire-test with 0", "SET", "nd:expire:zero", "v")
add("expire with 0 seconds -> key deleted immediately, reply 1", "EXPIRE", "nd:expire:zero", "0")
add("get after expire-0 (must be nil)", "GET", "nd:expire:zero")
add("exists after expire-0 (must be 0)", "EXISTS", "nd:expire:zero")
add("set a key to expire-test with negative", "SET", "nd:expire:neg", "v")
add("expire with -100 seconds -> key deleted immediately, reply 1", "EXPIRE", "nd:expire:neg", "-100")
add("get after expire-negative (must be nil)", "GET", "nd:expire:neg")
add("expire on a key that doesn't exist -> reply 0", "EXPIRE", "nd:expire:missing", "10")
add("expire with a non-integer seconds arg -> not-an-integer error",
    "SET", "nd:expire:badarg", "v")
add("expire non-integer arg, confirm error", "EXPIRE", "nd:expire:badarg", "abc")

# --- 4. binary-safe keys/values (embedded CRLF, NUL, full byte range) -----
add("set a value with embedded CRLF (must not be mistaken for a protocol delimiter)",
    "SET", "nd:bin:crlf", b"line1\r\nline2\r\n")
add("get after embedded-CRLF value (exact roundtrip)", "GET", "nd:bin:crlf")
add("set a value with an embedded NUL byte", "SET", "nd:bin:nul", b"before\x00after")
add("get after embedded-NUL value (exact roundtrip, no C-string truncation)",
    "GET", "nd:bin:nul")
add("set a KEY (not value) containing an embedded NUL byte",
    "SET", b"nd:bin:key\x00suffix", "v")
add("get using the same embedded-NUL key (exact roundtrip)", "GET", b"nd:bin:key\x00suffix")
add("set a KEY containing embedded CRLF",
    "SET", b"nd:bin:key\r\nsuffix", "v")
add("get using the same embedded-CRLF key", "GET", b"nd:bin:key\r\nsuffix")
add("set a value spanning the full 0-255 byte range", "SET", "nd:bin:allbytes", ALL_BYTES)
add("get after full-byte-range value (exact roundtrip)", "GET", "nd:bin:allbytes")

# --- 5. empty keys/values ---------------------------------------------------
add("set with both key and value empty", "SET", "", "")
add("get the empty key", "GET", "")
add("set with empty key, non-empty value", "SET", "", "nonempty")
add("get the empty key after overwrite", "GET", "")
add("exists on the empty key", "EXISTS", "")
add("del the empty key", "DEL", "")
add("incr on the empty key (fresh, no prior value) starts at 1", "INCR", "")
add("del the empty key again for cleanup", "DEL", "")

# --- 6. wrong-type / malformed-usage ops -----------------------------------
add("type of a normal string key", "SET", "nd:type:str", "v")
add("type reply for existing string key", "TYPE", "nd:type:str")
add("type reply for a missing key", "TYPE", "nd:type:missing")
add("getdel on a missing key -> nil, no error", "GETDEL", "nd:type:missing")
add("getex on a missing key -> nil, no error", "GETEX", "nd:type:missing")
add("persist on a missing key -> 0, no error", "PERSIST", "nd:type:missing")
add("ttl on a missing key -> -2 (Valkey sentinel: key does not exist)",
    "TTL", "nd:type:missing")
add("pttl on a missing key -> -2", "PTTL", "nd:type:missing")
add("wrong-arity: get with no args", "GET")
add("wrong-arity: get with too many args", "GET", "a", "b")
add("wrong-arity: incr with no args", "INCR")
add("wrong-arity: expire with only one arg", "EXPIRE", "nd:onearg")
add("wrong-arity: del with no args", "DEL")
add("wrong-arity: keys with no args", "KEYS")
add("wrong-arity: keys with too many args", "KEYS", "a", "b")

# --- 7. GETEX-no-args vs PERSIST: the TTL-mutation contrast ----------------
add("set key A with a TTL for the bare-GETEX check", "SET", "nd:getex:bare", "v", "EX", "100")
add("ttl before bare getex", "TTL", "nd:getex:bare")
add("bare getex (no options) -> returns value, MUST NOT clear the TTL",
    "GETEX", "nd:getex:bare")
add("ttl after bare getex (must be essentially unchanged, still ~100)",
    "TTL", "nd:getex:bare")
add("set key B with a TTL for the getex-PERSIST check", "SET", "nd:getex:persist", "v", "EX", "100")
add("getex with PERSIST -> clears the TTL", "GETEX", "nd:getex:persist", "PERSIST")
add("ttl after getex-PERSIST (must be -1, TTL cleared)", "TTL", "nd:getex:persist")
add("set key C with a TTL for the plain-PERSIST-command check", "SET", "nd:persist:cmd", "v", "EX", "100")
add("plain PERSIST command -> clears the TTL, replies 1", "PERSIST", "nd:persist:cmd")
add("ttl after plain PERSIST command (must be -1)", "TTL", "nd:persist:cmd")
add("persist on a key with no TTL -> replies 0 (nothing to persist)",
    "PERSIST", "nd:persist:cmd")
add("getex with EX 0 -> invalid expire error", "GETEX", "nd:getex:bare", "EX", "0")
add("getex with an unknown option -> syntax error", "GETEX", "nd:getex:bare", "BOGUS")

# --- 8. SCAN/KEYS MATCH glob edge patterns ---------------------------------
add("set up a small fixed keyspace for glob tests", "FLUSHALL")
add("set glob:a", "SET", "glob:a", "1")
add("set glob:ab", "SET", "glob:ab", "1")
add("set glob:b", "SET", "glob:b", "1")
add("set literal star key", "SET", "glob:*", "1")
add("set empty-string key for empty-pattern test", "SET", "", "1")
add("keys with pattern '*' (matches everything, including the empty key)",
    "KEYS", "*")
add("keys with empty pattern '' (matches ONLY the literal empty-string key)",
    "KEYS", "")
add("keys with pattern 'glob:?' (single char after glob:)", "KEYS", "glob:?")
add("keys with pattern 'glob:*' (prefix match, includes literal-star key too)",
    "KEYS", "glob:*")
add("keys with escaped-star pattern 'glob:\\\\*' (matches ONLY the literal-star key)",
    "KEYS", "glob:\\*")
add("keys with class pattern 'glob:[ab]'", "KEYS", "glob:[ab]")
add("keys with negated class pattern 'glob:[^a]'", "KEYS", "glob:[^a]")
add("keys with range pattern 'glob:[a-b]'", "KEYS", "glob:[a-b]")
add("keys with reversed-range pattern 'glob:[b-a]' (order-corrected per digest)",
    "KEYS", "glob:[b-a]")
add("keys with an unterminated class 'glob:[ab' (malformed, must not crash)",
    "KEYS", "glob:[ab")
add("keys with a dangling trailing backslash 'glob:a\\\\' (malformed escape)",
    "KEYS", "glob:a\\")
add("keys with a double-star pattern 'glob:**' (redundant stars, same as single)",
    "KEYS", "glob:**")
add("keys with a multi-star pattern '*g*b*' (multiple wildcards)", "KEYS", "*g*b*")
add("scan with MATCH 'glob:*' and a large COUNT (single-page; cursor not compared, "
    "only the matched-key set is)", "SCAN", "0", "MATCH", "glob:*", "COUNT", "1000")
add("scan with MATCH '*' and large COUNT over the full glob keyspace",
    "SCAN", "0", "MATCH", "*", "COUNT", "1000")


def normalize(cmd, reply):
    """KEYS and SCAN both carry implementation-defined ordering/cursor state
    that must not be byte-compared (KEYS: hash iteration order; SCAN: bible
    D10 says pg_resp's cursor encoding is its own, never meant to match
    Valkey's). Everything else in this deck is compared byte-for-byte,
    including error message text — a wording mismatch is a real finding."""
    if not cmd:
        return reply
    name = cmd[0].upper() if isinstance(cmd[0], str) else cmd[0].upper().decode()
    if name == "KEYS" and reply.startswith(b"*") and reply != b"*-1":
        parts = reply.split(b"\r\n")
        elements = [parts[i + 1] for i in range(1, len(parts) - 1, 2)]
        return b"KEYS(unordered):" + b",".join(sorted(elements))
    if name == "SCAN" and reply.startswith(b"*2"):
        # Reply shape: *2 \r\n $cursor-len \r\n cursor \r\n *N \r\n (bulk pairs).
        # Strip the cursor entirely; compare only the sorted key array.
        first_nl = reply.index(b"\r\n")
        rest = reply[first_nl + 2 :]
        cursor_len_end = rest.index(b"\r\n")
        cursor_len = int(rest[1:cursor_len_end])
        cursor_start = cursor_len_end + 2
        keys_blob = rest[cursor_start + cursor_len + 2 :]
        if keys_blob == b"*-1" or keys_blob == b"*0":
            return b"SCAN(unordered):"
        parts = keys_blob.split(b"\r\n")
        elements = [parts[i + 1] for i in range(1, len(parts) - 1, 2)]
        return b"SCAN(unordered):" + b",".join(sorted(elements))
    return reply


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--candidate-host", default="127.0.0.1")
    ap.add_argument("--candidate-port", type=int, required=True)
    ap.add_argument("--oracle-host", default="127.0.0.1")
    ap.add_argument("--oracle-port", type=int, required=True)
    args = ap.parse_args()

    commands = [c for _, c in DECK]
    labels = [label for label, _ in DECK]

    candidate = run_stream(args.candidate_host, args.candidate_port, commands)
    oracle = run_stream(args.oracle_host, args.oracle_port, commands)

    mismatches = []
    for i, (label, cmd, c, o) in enumerate(zip(labels, commands, candidate, oracle)):
        if normalize(cmd, c) != normalize(cmd, o):
            mismatches.append((i, label, cmd, c, o))

    print(f"{len(commands)} nasty-deck commands replayed, {len(mismatches)} mismatches")
    for i, label, cmd, c, o in mismatches:
        print(f"  #{i} [{label}]")
        print(f"      cmd={cmd!r}")
        print(f"      candidate={c!r}")
        print(f"      oracle={o!r}")

    sys.exit(1 if mismatches else 0)


if __name__ == "__main__":
    main()
