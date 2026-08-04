# Valkey T0 Command Set: Behavioral Semantics

**Branch:** unstable (pin 0fc8cdafcba8)  
**Scope:** PING, ECHO, GET, SET, DEL, EXISTS, TTL, PTTL, EXPIRE, INCR, DECR, INCRBY, DECRBY, MGET, MSET

## Command Reference

### PING  
- Signature: `PING [message]`  
- **0 args:** reply `+PONG`  
- **1 arg:** echo back the argument as bulk string  
- **>1 arg:** error (wrong number of arguments)  
- **File:** server.c:5059-5079

### ECHO  
- Signature: `ECHO message`  
- Returns argument as bulk string (`$N...`)  
- **File:** server.c:5081-5083

### GET  
- Signature: `GET key`  
- **Missing key:** `$-1` (nil bulk)  
- **String value:** `$N...` (bulk string)  
- **Wrong type (non-string):** error `WRONGTYPE Operation against a key holding the wrong kind of value`  
- **File:** t_string.c:302-318

### SET  
- Signature: `SET key value [NX|XX|IFEQ value] [GET] [EX sec|PX ms|EXAT sec|PXAT ms|KEEPTTL]`  
- **Returns:**  
  - `+OK` for successful set (no GET flag)  
  - `$-1` (nil) if set rejected (NX when exists, XX when missing, IFEQ mismatch)  
  - Old value (bulk string) if GET flag and key exists  
  - `$-1` if GET flag and key missing or wrong type  
- **EX:** TTL in seconds, converted to milliseconds internally  
- **PX:** TTL in milliseconds  
- **EXAT:** absolute Unix timestamp in seconds  
- **PXAT:** absolute Unix timestamp in milliseconds  
- **KEEPTTL:** preserve existing TTL on overwrite  
- **IFEQ:** only set if existing value matches comparison (requires XX implicitly)  
- **NX + XX:** error (mutually exclusive)  
- **NX + IFEQ:** error  
- **GET + IFEQ + XX/NX:** error  
- **EXAT/PXAT in past:** key stored but logically expired (active or lazy delete)  
- **Negative/zero expire:** error `invalid expire time`  
- **File:** t_string.c:76-207 (setGenericCommand), 251-265 (setCommand)

### DEL  
- Signature: `DEL key [key ...]`  
- Returns count of deleted keys (integer)  
- Keys not existing: counted as 0  
- **File:** db.c:888-890

### EXISTS  
- Signature: `EXISTS key [key ...]`  
- Returns count of existing keys (integer)  
- Duplicate keys in args: each counts separately (e.g., `EXISTS x x` with one key x returns 2)  
- **File:** db.c:898-906

### TTL  
- Signature: `TTL key`  
- **Key missing:** `-2` (integer)  
- **Key exists, no expiry:** `-1` (integer)  
- **Key with expiry:** TTL in seconds, rounded up from milliseconds (`(ms + 500) / 1000`)  
- **File:** expire.c:898-920, 923-925

### PTTL  
- Signature: `PTTL key`  
- **Key missing:** `-2` (integer)  
- **Key exists, no expiry:** `-1` (integer)  
- **Key with expiry:** TTL in milliseconds (absolute expire time - now)  
- **File:** expire.c:898-930, 928-930

### EXPIRE  
- Signature: `EXPIRE key seconds [NX|XX|GT|LT]`  
- **Returns:**  
  - `1` if expiry was set  
  - `0` if key doesn't exist OR condition flag failed  
- **NX:** only set if key has no current expiry  
- **XX:** only set if key has current expiry  
- **GT:** only set if new expiry > current expiry (fails if current is -1 / infinite)  
- **LT:** only set if new expiry < current expiry (fails if current is -1)  
- **NX + XX:** error  
- **NX + GT/LT:** error  
- **GT + LT:** error  
- **Negative seconds:** allowed (key expires immediately); may be clipped to 0  
- **File:** expire.c:763-880

### INCR  
- Signature: `INCR key`  
- **Missing key:** create with value `0`, return `1`  
- **Non-missing string:** parse as 64-bit integer, increment, return new value  
- **Non-integer string (e.g., spaces, "abc"):** error `ERR value is not an integer or out of range`  
- **Overflow (oldval > 0 and incr > 0 and sum > LLONG_MAX):** error `increment or decrement would overflow`  
- **Wrong type (list, set, etc.):** error `WRONGTYPE Operation against a key holding the wrong kind of value`  
- **File:** t_string.c:697-733

### DECR  
- Signature: `DECR key`  
- **Missing key:** create with value `0`, return `-1`  
- **Non-missing string:** parse as 64-bit integer, decrement, return new value  
- **Non-integer string:** error (same as INCR)  
- **Underflow (oldval < 0 and decr > 0 and sum < LLONG_MIN):** error `increment or decrement would overflow`  
- **File:** t_string.c:735-737

### INCRBY  
- Signature: `INCRBY key increment`  
- **Increment not integer:** error `value is not an integer or out of range`  
- Otherwise: same semantics as INCR, but by arbitrary amount  
- **File:** t_string.c:739-744

### DECRBY  
- Signature: `DECRBY key decrement`  
- **Decrement == LLONG_MIN:** error `decrement would overflow` (special case: negation overflow)  
- **Decrement not integer:** error `value is not an integer or out of range`  
- Otherwise: same as DECR, but by arbitrary amount  
- **File:** t_string.c:746-756

### MGET  
- Signature: `MGET key [key ...]`  
- Returns array of bulk strings  
- **Missing key in list:** `$-1` (nil element)  
- **Wrong type key in list:** `$-1` (nil element)  
- **File:** t_string.c:530-546

### MSET  
- Signature: `MSET key value [key value ...]`  
- Requires **odd number of total arguments** (command name + even pairs)  
- Returns `+OK` on success  
- **Even arg count:** error `wrong number of arguments for 'mset' command`  
- Atomically overwrites all keys  
- **File:** t_string.c:592-599

## Test Vectors (from tests/unit/type/string.tcl and tests/unit/type/incr.tcl)

| Command | Input | Expected Output | Notes |
|---------|-------|-----------------|-------|
| SET | `SET x foobar` | `+OK` | Basic string set |
| GET | `GET x` | `foobar` | Basic string get |
| GET (missing) | `GET nonexist` | `$-1` | Nil for missing key |
| SET NX (exists) | `SET x 1; SET x 2 NX` | `$-1` (nil) | NX fails when key exists |
| SET NX (new) | `DEL x; SET x 1 NX` | `+OK` | NX succeeds when missing |
| SET XX (missing) | `DEL x; SET x 1 XX` | `$-1` (nil) | XX fails when missing |
| SET XX (exists) | `SET x 1; SET x 2 XX` | `+OK` | XX succeeds when exists |
| SET GET | `SET x bar; SET x baz GET` | `bar` | Returns old value |
| SET GET (missing) | `DEL x; SET x bar GET` | `$-1` (nil) | GET returns nil if missing |
| INCR | `INCR novar` | `1` | Creates key at 0, returns 1 |
| INCR (again) | `INCR novar` | `2` | Increments existing |
| DECR | `DEL x; DECR x` | `-1` | Creates key at 0, returns -1 |
| INCRBY | `SET novar 100; INCRBY novar 10` | `110` | Increment by amount |
| INCR (overflow) | `SET x 9223372036854775807; INCR x` | Error: `overflow` | LLONG_MAX + 1 |
| MGET | `SET x BAR; SET y FOO; MGET x z y` | `[BAR, nil, FOO]` | Array with nils for missing |
| MSET | `MSET k1 v1 k2 v2` | `+OK` | Sets multiple pairs |
| MSET (odd args) | `MSET k1 v1 k2` | Error: `wrong number` | Requires even args |
| TTL (no expiry) | `SET x y; TTL x` | `-1` | No expiry = -1 |
| TTL (missing) | `DEL x; TTL x` | `-2` | Missing key = -2 |
| TTL (with expiry) | `SET x y EX 10; TTL x` | `[5..10]` | Returns TTL in seconds |
| EXPIRE | `SET x y; EXPIRE x 10` | `1` | Returns 1 on success |
| EXPIRE (missing) | `DEL x; EXPIRE x 10` | `0` | Returns 0 if missing |
| EXISTS | `SET x 1; EXISTS x y` | `1` | Count of existing keys |
| DEL | `SET x 1; DEL x` | `1` | Returns count deleted |

## Traps & Edge Cases

1. **SET with IFEQ + GET:** Returns old value only if condition passed; nil if condition failed or key missing. Condition is checked before GET evaluation.

2. **SET with expired EXAT/PXAT:** If timestamp is in past, key is stored but immediately logically expired. Active expiry will delete it; lazy expiry (on access) will treat as missing. `+OK` is still returned.

3. **TTL rounding:** PTTL returns raw milliseconds; TTL rounds milliseconds up to nearest second (`(ms + 500) / 1000`), so a key with 1ms remaining shows TTL=1, not 0.

4. **EXPIRE NX/XX/GT/LT:**  
   - GT treats no-expiry (-1) as infinite: GT always fails vs. no-expiry key  
   - LT allows setting on no-expiry key if new value is finite  
   - Condition failure returns 0, not error  

5. **INCR/DECR on non-numeric strings:** "   11" and "11   " both error; parsing is strict (no leading/trailing whitespace). Any non-digit (except single leading ±) causes error.

6. **INCR on LLONG_MIN+1 to LLONG_MAX range:** Success; only overflow at boundaries.  
   - `DECR LLONG_MIN` → overflow error  
   - `DECRBY key LLONG_MIN` → special error `decrement would overflow` (negation overflow)

7. **MGET with wrong-type keys:** Returns nil (not error) for each non-string key in the list; rest of array proceeds normally.

8. **SET + multiple options order-independence:** `SET k v EX 10 NX` and `SET k v NX EX 10` are equivalent. Parser is case-insensitive.

9. **GETEX, GETSET, GETDEL:** Not in T0 set; included in source for completeness. GET is read-only; these are write variants.

10. **SET KEEPTTL with no prior TTL:** KEEPTTL on non-existing key or key with no expiry results in no expiry on new value; equivalent to `SET k v` with no EX/PX.

---

**Source Clone:** Valkey unstable, commit 0fc8cdafcba8, BSD-3-Clause licensed.  
Behavioral reference only; see Valkey source for implementation details.
