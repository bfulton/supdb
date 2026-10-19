# The hunt branches, as patch series

The two hunts of the last session ran on branches of their own, with
forcing hooks in `src/db.rs` behind environment variables and test files
that are loop targets rather than tests expected to be green. Their
author marked the hooks "for a hunt branch and not for a merge", so the
engine on the mainline carries none of them; the series here carry every
commit of those branches, message and all, so that the branches can be
deleted and the hunts resumed on any commit.

| series | base | what it is |
|---|---|---|
| `wip-hunt-stress0/` (10) | `779b2a2` | the first hunt: a Snapshot handle reads a write made after it, with the block cache on; the forced schedules, the minimal forms, the oracle, the event ring, the trace reader and the two-by-two of the two scratch fixes |
| `wip-hunt-bisect0/` (6) | `779b2a2` | the same hunt's bisect line: reproducers over the public surface alone, the hooks and the two take-aways behind `HUNT_*`, and the tools to run them at any commit |
| `wip-hunt-stress0-fixexp/` (2) | `c52ea63` (patch 5 of `wip-hunt-stress0`) | the two scratch experiments as commits; `wip-hunt-stress0` carries the same two as `apply_fix_b.py` and `apply_fix_c.py` |

To rebuild a branch exactly (each series was applied this way and its
tree compared with the branch's: identical):

    git switch -c hunt-stress0 779b2a2 && git am scratch/hunt/patches/wip-hunt-stress0/*.patch
    git switch -c hunt-bisect0 779b2a2 && git am scratch/hunt/patches/wip-hunt-bisect0/*.patch

The briefs the hunts were run from are `scratch/hunt/ctx.txt` and
`scratch/hunt2/ctx.txt`; the second hunt's fuzz is merged as
`tests/fuzz.rs` and needs no patch.
