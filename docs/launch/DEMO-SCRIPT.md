# Demo recording script — for the human to record

The 30-second clip that goes at the top of the README, under the code block. Three
quickstart commands, then one extra beat that lands the moat.

**Rehearse it once before recording.** The clip's value is that it looks effortless,
and typing latency is visible in a `.cast`.

---

## (a) The command sequence

### Stage — do this BEFORE you start recording

```bash
docker rm -f pg_resp >/dev/null 2>&1
docker rmi ghcr.io/cronosclaus/pg_resp:0.1.0 >/dev/null 2>&1   # so the pull is real
docker pull -q redis:8.2-alpine                                # pre-pull the CLIENT only
clear
```

Pre-pulling `redis:8.2-alpine` is deliberate: the clip should show **our** image being
pulled, not nine layers of somebody else's redis. That is honesty about what the
viewer is watching, not staging — the pg_resp pull stays cold and real.

### Record — exactly these six commands, nothing else

```bash
# 1 — start it (the pull is real; ~8s)
docker run -d --name pg_resp -e POSTGRES_PASSWORD=postgres \
  -p 127.0.0.1:6379:6379 -p 127.0.0.1:5432:5432 \
  ghcr.io/cronosclaus/pg_resp:0.1.0 -c pg_resp.bind_address=0.0.0.0

# 2 — wait for it
docker exec pg_resp pg_isready -U postgres

# 3 — it speaks Redis
redis-cli -p 6379 SET greeting hello
redis-cli -p 6379 GET greeting
```

Pause about a second after the `GET` returns `"hello"`. That is the first payoff and
it needs a beat.

```bash
# 4 — THE MOAT: set up a cached row and a trigger that evicts it
docker exec -i pg_resp psql -U postgres <<'SQL'
CREATE EXTENSION IF NOT EXISTS pg_resp;
CREATE TABLE products (id int primary key, price numeric);
INSERT INTO products VALUES (42, 9.99);
CREATE TRIGGER products_cache_evict
AFTER UPDATE OR DELETE ON products
FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
SQL

# 5 — cache the row, and show it is cached
redis-cli -p 6379 SET product:42 '{"price":9.99}'
redis-cli -p 6379 GET product:42

# 6 — change the row in SQL. The cache key evicts itself.
docker exec pg_resp psql -U postgres -c "UPDATE products SET price = 11.50 WHERE id = 42;"
redis-cli -p 6379 GET product:42
```

**The last line must print `(nil)`.** That is the whole clip: an `UPDATE` in SQL
invalidated a Redis key, with no application code involved. Hold two seconds on it,
then stop recording. Do not type anything after it — no `exit`, no `clear`.

### Verify before you keep the take

`GET product:42` returned `(nil)` **after** the `UPDATE`, and `"hello"` earlier. If
the last line shows the JSON instead of `(nil)`, the trigger did not fire — check
that `CREATE EXTENSION` succeeded in step 4 and re-record. **Do not edit the cast to
fix it.**

---

## (b) Terminal prep

| setting | value | why |
|---|---|---|
| window size | **100 × 28** | the SQL heredoc lines fit without wrapping; taller wastes embed height |
| font size | 16–18 pt | legible at ~900 px README embed width; 14 pt is unreadable when scaled down |
| prompt | `PS1='$ '` | a two-line powerline prompt eats a third of the frame and dates the clip |
| theme | dark, high contrast | GitHub renders on both themes; dark travels better |
| colour | leave `redis-cli` colour on | the `(nil)` is easier to spot |
| history | `unset HISTFILE` first | avoids a stray reverse-search suggestion appearing mid-type |

Also: widen nothing after starting — resizing mid-recording corrupts the geometry in
the `.cast`.

---

## (c) Install and record (userland, no system packages)

```bash
# install
pipx install asciinema || pip install --user asciinema
# agg (the cast -> GIF renderer) is a single static binary:
curl -sSL -o ~/.local/bin/agg \
  https://github.com/asciinema/agg/releases/latest/download/agg-x86_64-unknown-linux-gnu
chmod +x ~/.local/bin/agg

# record
asciinema rec --cols 100 --rows 28 --idle-time-limit 2 docs/launch/pg_resp-demo.cast
#   ... run the six commands ...
#   Ctrl-D to stop

# render at README-embed dimensions
agg --cols 100 --rows 28 --font-size 18 --speed 1.3 --theme asciinema \
    docs/launch/pg_resp-demo.cast docs/launch/pg_resp-demo.gif
```

`--idle-time-limit 2` truncates your thinking pauses to 2 s without touching the
command output — it is the one edit that is honest, because it compresses *your*
latency and not the software's. `--speed 1.3` takes the edge off typing speed. **Do
not raise `--speed` past ~1.5**: the 8-second pull is a real measured number and
speeding it up misrepresents it.

Expect a GIF of roughly **1.5–3 MB**. If it exceeds ~5 MB, drop `--font-size` to 16
before dropping frames.

## Then

Commit the `.cast` alongside the GIF — the cast is the auditable source, the GIF is
the artifact. I will review the cast's timing before it embeds, checking that the
pull duration is intact, no take was spliced, and the final `(nil)` is on screen long
enough to read.

Embed goes at the top of the README, directly under the existing code block:

```markdown
![pg_resp: redis-cli against Postgres, and a SQL UPDATE evicting a cache key](docs/launch/pg_resp-demo.gif)
```
