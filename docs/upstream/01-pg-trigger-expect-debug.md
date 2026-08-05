# `#[pg_trigger]`'s generated wrapper renders a returned `Err` with `Debug`, making error messages unreachable

**pgrx version:** 0.19.2
**PostgreSQL:** 18.4
**Platform:** Linux x86_64

## Summary

A `#[pg_trigger]` function returns `Result<Option<PgHeapTuple<..>>, E>`. When it
returns `Err(e)`, the generated wrapper surfaces that error to the user as

```
ERROR:  Trigger function panic: NotRowLevel
```

because the wrapper ends in `.expect("Trigger function panic")` and `expect`
formats its payload with `Debug`. The consequences:

1. The error text a user sees is the **`Debug` representation of the error
   type's variant name**, not any message the error type provides. An error type
   that implements `Display` (or `thiserror`) has that implementation ignored.
2. The message is prefixed with the word **"panic"** for an error that is not a
   panic — an ordinary, expected, well-handled validation failure reads to the
   user as an internal crash.
3. There is no way to attach an `errhint`, an `errdetail`, or a specific
   `SQLSTATE`. Everything arrives as the same generic error code.

Net effect: a trigger function cannot report a useful error through its own
return type. The only way to produce a properly styled Postgres error from a
trigger is to bypass the return type entirely and report/panic manually, which
is not obvious and is not mentioned in the trigger docs.

## Reproduction

Full extension, from a fresh `cargo pgrx new`:

```rust
use pgrx::prelude::*;

::pgrx::pg_module_magic!(name, version);

#[derive(Debug)]
pub enum MyTriggerError {
    /// Deliberately given a Display impl to show it is not used.
    MustBeRowLevel,
}

impl core::fmt::Display for MyTriggerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "this trigger must be created FOR EACH ROW; \
             see the extension docs for the CREATE TRIGGER form"
        )
    }
}

impl std::error::Error for MyTriggerError {}

#[pg_trigger]
fn demo_trigger<'a>(
    trigger: &'a pgrx::PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, impl WhoAllocated>>, MyTriggerError> {
    if matches!(trigger.level(), pgrx::PgTriggerLevel::Statement) {
        return Err(MyTriggerError::MustBeRowLevel);
    }
    Ok(trigger.new())
}
```

```sql
CREATE EXTENSION demo;
CREATE TABLE t (id int);

CREATE TRIGGER demo_stmt
AFTER INSERT ON t
FOR EACH STATEMENT EXECUTE FUNCTION demo_trigger();

INSERT INTO t VALUES (1);
```

### Observed

```
ERROR:  Trigger function panic: MustBeRowLevel
```

The `Display` impl — the entire user-facing message — never appears. Note also
that `psql`'s `\errverbose` shows no `DETAIL` or `HINT` fields, and the SQLSTATE
is the generic internal-error code.

### Expected (or at least: hoped for)

Something closer to:

```
ERROR:  this trigger must be created FOR EACH ROW; see the extension docs for the CREATE TRIGGER form
```

i.e. the error's `Display` output, with the option of a real `SQLSTATE`.

## Where it comes from

The `#[pg_trigger]` proc macro's generated wrapper finishes by unwrapping the
user function's `Result` with `.expect("Trigger function panic")`. Because
`Result::expect` requires `E: Debug` and formats with `Debug`, the wrapper both
(a) discards any `Display`/`std::error::Error` implementation and (b) labels the
outcome a panic.

## Suggested fix

In rough order of preference:

1. **Require `E: Into<ErrorReport>`** (or offer a blanket impl for
   `E: std::error::Error`) and `report()` the error at `ERROR` level instead of
   `expect`-ing it. That gives trigger authors the same error-reporting surface
   the rest of pgrx has, including `SQLSTATE`, detail and hint.
2. **Failing that, format with `Display`** where available, and drop the word
   "panic" from the message for the `Err` path — `Err` is a return value, not a
   panic, and the current wording sends users looking for a crash that did not
   happen.
3. **Failing both, document it**: a sentence in the `#[pg_trigger]` docs saying
   "returning `Err` produces a generic error rendered via `Debug`; to control
   the message, `SQLSTATE`, detail or hint, build and `report()` an
   `ErrorReport` yourself" would have saved the debugging time entirely.

## Workaround, for anyone who finds this issue first

Do not return `Err` from a trigger you want to produce a good message. Report
the error directly and let the function diverge:

```rust
fn raise(code: PgSqlErrorCode, message: String, hint: &'static str) -> ! {
    pgrx::pg_sys::panic::ErrorReport::new(code, message, "my_ext")
        .set_hint(hint)
        .report(PgLogLevel::ERROR);
    unreachable!("ERROR-level report does not return")
}
```

This yields a correctly styled Postgres error with a real hint and SQLSTATE. It
is what we ended up doing at every failure path in our trigger function.

## Context

Found while building [pg_resp](https://github.com/OWNER/pg_resp), a
Redis-protocol cache that runs inside a Postgres background worker. Its
`resp.evict()` trigger helper had five carefully written validation messages
that were all invisible to users until we traced this — every one of them
rendered as `Trigger function panic: <VariantName>`.
