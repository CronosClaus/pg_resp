# `GucSetting::get()` panics off the active thread, and nothing on the method says so

**pgrx version:** 0.19.2
**PostgreSQL:** 18.4
**Platform:** Linux x86_64

## Summary

`GucSetting::get()` begins with a thread check:

```rust
// pgrx/src/guc.rs
/// A safe wrapper around a global variable that can be edited through a GUC
pub struct GucSetting<T: GucValue> { /* ... */ }

impl<T: GucValue> GucSetting<T> {
    pub fn get(&self) -> T {
        pg_sys::submodules::thread_check::check_active_thread();
        unsafe { GucValue::from_raw(self.value.get()) }
    }
    // ...
}
```

`check_active_thread()` panics when called from any thread other than the one
that first entered Postgres. That behaviour is defensible — it is the same
soundness rule the rest of pgrx enforces. The problem is discoverability:

- **`get()` carries no doc comment at all.** Not a line.
- The struct-level doc is *"A safe wrapper around a global variable that can be
  edited through a GUC"*, which reads as an unconditionally safe accessor.
- Nothing in the type's name, signature (`&self -> T`, no `unsafe`, no
  `Result`), or documentation suggests a thread affinity. `GucSetting<T>` even
  implements `Sync`, which actively signals "shareable across threads" — it is
  `Sync` because the *type* is safe to share, but the natural inference from
  `Sync` is that reading it from another thread is fine, and it is not.

The result is a trap for exactly one audience: background-worker authors. A
bgworker that spawns any thread — an event loop, a listener, a poller — will
find that the obvious design ("re-read the GUC where I use it, so operators can
`SET` it live") panics at runtime, with a message about threads rather than
about GUCs, and only once that code path is actually exercised.

## Reproduction

```rust
use pgrx::bgworkers::*;
use pgrx::prelude::*;
use std::time::Duration;

::pgrx::pg_module_magic!(name, version);

static MY_PORT: GucSetting<i32> = GucSetting::<i32>::new(6379);

#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    GucRegistry::define_int_guc(
        "demo.port",
        "port",
        "port",
        &MY_PORT,
        1,
        65535,
        GucContext::Postmaster,
        GucFlags::default(),
    );
    BackgroundWorkerBuilder::new("demo_worker")
        .set_function("demo_worker_main")
        .set_library("demo")
        .enable_spi_access()
        .load();
}

#[pg_guard]
#[no_mangle]
pub extern "C-unwind" fn demo_worker_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGTERM);

    // Reading on the worker's own (active) thread: fine.
    let _ok = MY_PORT.get();

    // The natural bgworker shape: hand the work to a thread.
    let handle = std::thread::spawn(|| {
        // PANICS here, before any Postgres function is knowingly called.
        MY_PORT.get()
    });
    let _ = handle.join();

    while BackgroundWorker::wait_latch(Some(Duration::from_secs(1))) {}
}
```

### Observed

The spawned thread panics inside `check_active_thread()`. Because the panic is
caught at the `std::thread::spawn` boundary, the worker process itself stays
alive and the failure can be easy to miss depending on how the thread's result
is handled.

### Expected

Either a documented contract, or a clearer failure. Concretely, any of:

1. **A doc comment on `get()`** — one sentence would be enough: *"Must be called
   from the thread that owns the Postgres connection (the main/active thread);
   panics otherwise. In a background worker that spawns threads, read GUC values
   on the worker's main thread and pass the values to the thread."*
2. **A note on the `GucSetting` type doc**, since `Sync` invites the opposite
   inference.
3. **A mention in the background-worker docs / `bgworkers` module**, which is
   where the affected audience actually reads.

## Why this is worth a doc line rather than a code change

The check is correct and should stay. The cost here is purely that the
constraint is invisible until runtime, and the workaround is trivial *once
known*: read every GUC once on the main thread before spawning, and pass plain
values (`String`, `usize`, `Option<Vec<u8>>`, …) into the thread.

There is a second-order consequence worth naming, because it affects API
design rather than just ergonomics: if a value can never be re-read from the
thread that uses it, then **every GUC in a threaded worker is effectively
read-once-at-startup regardless of its declared `GucContext`**. Declaring such a
GUC `Suset` — implying an operator's `SET` takes effect without a restart —
becomes misleading to users. We changed all five of our GUCs to `Postmaster`
context once we understood this, because that is what the architecture actually
delivers. A reader who hits the panic first has to work that chain of reasoning
out for themselves.

## Precise wording note

`check_active_thread()` remembers *the first thread to call into Postgres* and
panics for any other, so "main-thread-only" is a good approximation but not
literally the rule. In a background worker the two coincide, because the
worker's main thread necessarily enters Postgres first (signal handlers, SPI
setup) before any thread it spawns can run.

## Context

Found while building [pg_resp](https://github.com/OWNER/pg_resp), a
Redis-protocol cache server that runs inside a Postgres background worker. Its
network loop lives on a spawned thread, so every one of its five GUCs has to be
read on the main thread and passed down — a pattern we arrived at by hitting
this panic, not by reading about it.
