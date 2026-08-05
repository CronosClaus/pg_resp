# WITHDRAWN as a bug report — `ereport!`'s 4th argument is `errdetail`, and pgrx documents it correctly

**Do not file this as written. Read this page first, then decide.**

## Why this file is a withdrawal rather than an issue

The Phase 4 plan listed this as one of three original upstream findings, phrased
as *"`ereport!`'s optional fourth argument being **errdetail**, not errhint,
which silently misfiles every hint."* Checked against pgrx 0.19.2's actual
source before filing, that framing is **wrong**, and filing it would have been
an incorrect bug report submitted under a real name.

What the source actually says:

```rust
// pgrx-pg-sys/src/submodules/elog.rs — doc comment on `ereport!`
/// The argument order is:
/// - `log_level: [PgLogLevel]`
/// - `error_code: [PgSqlErrorCode]`
/// - `message: String`
/// - (optional) `detail: String`
```

and the macro arms match that documentation exactly:

```rust
(ERROR, $errcode:expr, $message:expr $(, $detail:expr)? $(,)?) => {
    $crate::panic::ErrorReport::new(/* ... */)
        $(.set_detail($detail))?
        .report($crate::elog::PgLogLevel::ERROR);
    unreachable!();
};
```

So the parameter is **named** `detail`, **documented** as `detail`, and calls
`set_detail`. Nothing is mislabeled and nothing is silently misfiled. pgrx does
what it says.

The bug was ours: we passed hint text into a parameter that is documented as
detail, without reading the doc comment. Our own code comment in
`crates/pg_resp/src/sql.rs` already states the correct fact — that `ereport!`'s
fourth argument sets errdetail, which is why we go through the `ErrorReport`
builder for hints — so the code is right and only the *characterization of it as
an upstream defect* was wrong.

A second candidate defect was investigated and also died: the doc examples call
`ereport!(PgLogLevel::ERROR, ...)` while the visible arms match a bare `ERROR`
ident, which looked like a non-compiling example. It compiles — there is a
generic `($loglevel:expr, $errcode:expr, ...)` catch-all arm at the end of the
macro. **Verified empirically** by compiling the documented form inside this
extension, not by reading, precisely because the reading had already misled us
once.

## What, if anything, is left to file

One narrow and genuinely true ergonomics observation:

> `ereport!` can set a message and an `errdetail`, but has no way to set an
> `errhint`. Extension authors who want a hint — which the PostgreSQL error
> style guide encourages, since hint is where actionable advice belongs — must
> drop to the `ErrorReport` builder. A sentence in the `ereport!` docs pointing
> at `ErrorReport::set_hint` for that case would close the gap.

That is a documentation nicety, not a bug, and it is a maintainer's judgement
whether it is worth an issue. **Recommendation: do not file it.** It is thin
next to findings 1 and 3, and a thin third issue slightly cheapens the two that
are substantial. If you would rather file all three, file this as the wording
above — a doc suggestion, never as "misfiles hints".

## Consequence for the launch post

The launch post should claim **two** original upstream findings, not three.
Claiming three and having one be a withdrawn misreading is exactly the kind of
detail a hostile reader checks.
