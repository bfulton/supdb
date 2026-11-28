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
| PR15 | `607ed7d` the open's attach drops raw pages only where asked (a fix of 1ee67db's positional tuple), `69cec32` zpages-read, `f317848` zpages-walk | green at `f317848`: "all checks passed: build test stride onefile lint wasm bench lists" | RED against `b52e269` by the script's verdict, but it passes under the amended rule: each gain is inlining that moved, and every probe's net falls (below) |

### PR15's count, read by function

`sh scripts/cost.sh b52e269`, at `f317848`, alone:

- The nets: read_z -22.17 an op, scan -101.00, scan_long -101.15,
  scan_z -281.83, scan_long_z -5,440.13, scan_s -18.00 and
  scan_long_s -18.00. merge's +8.09 is `Reader::sweep_retired_states`,
  held to its own bound, as it was at PR14.
- Gains, and what they were inlined from:
  - `ZFile::open_group_with` +11.9 an op on read_z (+101 on scan_z,
    +3,536 on scan_long_z), against `ZMemo::get` -27.1 (-276 on scan_z).
    Once the memo-first branch went (69cec32), `ZMemo::get` was small
    enough to inline. The base's `open_group_with` called it 101,095
    times on read_z and the head's calls it none; the symbol is in
    neither of the head's probes (`nm -C`).
  - `Reader::pwalk_seek` +132.0 on scan_z (+10.0 on scan_s) and
    `ZCursor::stride_seek` +3.0 on scan_s, against `ZCursor::seek`
    -151.0 (-31.0). Its other caller was the seeded seek, which went in
    f317848. The head's `pwalk_seek` calls seek's own callees (`place`,
    `group_of_key`, `open_at`), and the symbol is gone.

### Positional check (the coordinator's ask after 607ed7d)

I read every commit of this track (745febd, 70cbecc, 70ce215, 1ee67db,
e4e6e0f, b52e269, 607ed7d, 69cec32) for an entry removed from a tuple,
array, bit set or positional argument list whose other entries are still
read by position:

- Tuples read by index (`git show C -- src | grep '[a-z_]\.[0-9]'`):
  - In 745febd the only ones are the re-code look's rows (`r.5`, `r.6`,
    `t.0`..`t.2`), removed whole with `Maint::z_recode_look`.
  - 1ee67db's `z_attach` tuple is the bug 607ed7d fixed.
  - b52e269 renumbered the same tuple's tail (cursor_view, group_ahead,
    walk_owns) to match; its one wrong read was the drop's `lines.3`,
    from 1ee67db.
  - Since 607ed7d the tuple is a struct.
- Bit sets and arrays: none of these commits removes a flag bit, a
  constant shift or an array slot. The only removed lines of that shape
  are byte sizes (`32 << 20`, `2 << 20`, `8 << 20`) and the fence tree's
  `ProbeLine([u64; 8])`, which went whole.
- Positional arguments, a removed parameter against each call site:
  - 70cbecc: `Db::layout_of` lost `from_index` after `rebase`, and its
    one call dropped `self.opts.same_blocks_from_index`, the argument in
    that place. `Db::same_blocks` lost `from_index` between `b` and
    `asked`, and its call matches. Both are typed apart from their
    neighbours.
  - 69cec32: `Db::z_attach` lost `trusted` and `memo_first`, and its
    three callers pass `advise, memo, prefetch` in order.
    `zfile::ZFile::attach` lost `memo_first` after `trusted`. Its 14
    callers (10 in zfile.rs, 4 in db.rs), parsed before and after, each
    dropped the `memo_first` argument and kept `trusted`, the fence and
    `memo_ahead`. The store's calls now pass `true`,
    `FenceSearch::Words` and `true`, which are what its defaults gave.
  - Every other removed field is a struct's or an initializer's, named.

## Next: PR16 (zpages-cursor)

`zpages_bulk_part`, `zpages_walk_part`, `zpages_cursor_view`,
`zpages_group_ahead`, `zpages_block_cursor`, `zpages_scan_cursor`,
`zpages_walk_cursor`, `zpages_block_cache`: 9 arms, 12 test functions,
and fuzz draws (`zpages_walk_part`, `zpages_block_cursor`) whose calls
stay. `cursor_view` and `group_ahead` are `ZAttachLines` fields. Then:

    sh scripts/check.sh           # to its last line
    sh scripts/cost.sh f317848    # nothing else on the machine

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

## After PR16

PR17 is forms-carry, then hot-raw. PR18 is
settle-filing, then merge-walk. Each follows
`release/ledger-review/consistency-and-order.md` (its rows and its
appendix) and `release/phase1.md`.
