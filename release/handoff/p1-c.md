# Phase 1, track C: handoff

Track C is the compact form's chain, the reviewer's groups PR13 to PR18,
in order. It is on `claude/supdb-pretag-p1-c`, branched from the working
branch's `8e3f9d71`. Every commit is one cluster (or a fix named as one)
and is signed. Times are UTC.

## Done

| group | commits | gate (`sh scripts/check.sh`) | count (`sh scripts/cost.sh BASE`) |
|---|---|---|---|
| PR13 | `745febd` zpages-recode, `70cbecc` merge-drop, `70ce215` the zpages tests' Linux-only helpers (macOS clippy) | green at `70ce215`: every group, with `lists` rerun after `git fetch origin main` | RED against `8e3f9d71` on `merge`, carried to PR18 (below) |
| PR14 | `1ee67db` zpages-coding, `e4e6e0f` `zpages_hold_after_settle` goes (no setter left), `b52e269` zpages-codec | green at `b52e269`: "all checks passed: build test stride onefile lint wasm bench lists" | green against `70ce215` |

## In progress: PR15 (zpages-read, zpages-walk)

- `607ed7d`: a fix for a bug `1ee67db` (PR14) made. `Db::z_attach`'s
  positional flag tuple lost `zpages_inplace_walk`, and the raw drop kept
  index 3, which became the decode timer's. An attach at the open, at a
  seal or at the gate's give then dropped raw pages wherever decodes are
  timed (on by default), not where `zpages_drop_raw` asks. The default,
  which drops, read the same. The flags are now a struct
  (`ZAttachLines`). Its test fails at `b52e269` and passes at the fix.
  PR14's gate and count read the buggy shape for drop-off stores and
  arms; the default did not differ.
- `69cec32`: zpages-read. `zpages_point_part` stays as a hidden seam,
  and its three arms go.
- Next: zpages-walk (`zpages_scan_admit`, `zpages_bulk_admit`,
  `zpages_bulk_steps`, `zpages_seek_seeds`, `zpages_walk_owns_decodes`;
  arms supdb-zbulkmemo, supdb-zbulksearch, supdb-zscanputs,
  supdb-zseekseed, supdb-zwalkowns; `zpages_scan_admit` is a fuzz draw
  and keeps its call). Then the whole gate, then the count against
  `b52e269`, alone:

      sh scripts/check.sh           # to its last line
      sh scripts/cost.sh b52e269    # nothing else on the machine

## Held back, and why

- `compact_run`, the `merge` probe, against `8e3f9d71`: +3.07 an op net,
  with HOT GAINED `supdb::db::compact_run` at +1.997 an op. Read by
  instruction, it is register allocation in the `merge_ranks` loop that
  is inlined into `compact_run`: about +24 stack loads and -9 stores an
  op, after merge-drop took the per-record `drop_at` closure out. Also
  +0.85 an op of `memcpy` from `realloc` in `Wal::replay_frames` (heap
  placement), and the census's clock work. `#[inline(never)]` on
  `merge_ranks` read +19 an op and was reverted. As agreed with the
  coordinator: at PR18's end, count against `8e3f9d71`. If it still
  gains, make one try at a shape that removes the carried value. If that
  fails, declare it in `scripts/raises.txt` with the instruction reading
  as its reason, and report it.

## After PR15

PR16 is zpages-cursor. PR17 is forms-carry, then hot-raw. PR18 is
settle-filing, then merge-walk. Each follows
`release/ledger-review/consistency-and-order.md` (its rows and its
appendix) and `release/phase1.md`.
