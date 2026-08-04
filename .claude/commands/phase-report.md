---
description: Close out a phase — write the handoff report, update the phase pointer
---
Close out **Phase $ARGUMENTS** per bible §0.2.

Write `reports/phase$ARGUMENTS.md` containing:
1. **Gate table** — every gate from bible §5 for this phase, with the raw measured number/result next to the pass condition. No prose substitutes for numbers.
2. **What broke and why** — failures encountered, root causes, what was changed.
3. **Decisions** — any new `D<n>` entries added to bible §12 this phase, restated in one line each.
4. **Open threads** — anything the next phase must know that is not in the bible.
5. **Verdict** — PASS (proceed) / FAIL (stop, human decision needed), and the single next action.

Then:
- Update the "Current phase" line in `CLAUDE.md`.
- Commit everything with message `phase $ARGUMENTS: <verdict> — <one-line summary>`.
- Remind the human: `/clear` before starting the next phase.

The report must be readable by a fresh session with zero context from this one — that is its entire purpose.
