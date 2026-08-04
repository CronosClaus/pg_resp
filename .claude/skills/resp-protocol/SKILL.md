---
name: resp-protocol
description: RESP2 wire protocol ground truth for pg_resp — framing rules, inline commands, error taxonomy, canonical byte-level test vectors, SET-option decision table. Consult for ANY question about how bytes on the wire must look or how a command must respond.
---
# RESP2 protocol facts

**STATUS: STUB — Phase 0 task 3 fills this** from the vendored spec pointer in `docs/refs/resp2-spec.md` and Valkey behavior (never Redis source — bible D8).

Required contents when filled (bible §7):
1. Framing: the 5 RESP2 types (+ - : $ *), CRLF rules, null bulk string vs empty, nested arrays.
2. Inline command handling (what redis-cli sends when you type without protocol).
3. Error taxonomy: -ERR vs -WRONGTYPE, exact error-string conventions clients pattern-match on.
4. ~30 canonical byte-level test vectors (request bytes → exact response bytes), covering every T0 command incl. edge cases: GET missing key, INCR on non-integer, SET with EX+NX combined, TTL on key without expiry (-1) vs missing key (-2).
5. SET option decision table: EX/PX/EXAT/NX/XX/KEEPTTL/GET interactions — which combinations error, which are silent.
6. Client handshake probes: what redis-py / node-redis / go-redis / jedis send on connect (HELLO, CLIENT SETINFO, COMMAND DOCS, INFO) and the minimal non-breaking stub response for each.
