# Skeptic's check of item "spill"

My copies are in `check/`: `spill.py` (extended), `pfm.py` (unchanged, `cmp`
equal to `work/e4-model/pfm.py`), `names.py` (the name arithmetic, a third
way), and `results/`. Every run was one process, `nice -n 10 timeout 600`.

## Reran

- `spill.py n=4 alpha=CTDPRUXLctdpru`: 0 of 767,174 any / 177,606 proc,
  as reported. `dsrule=False`: 113,899 UNNAMED, first `P`, as reported.
- Name arithmetic, a third way (`check/names.py`): spelling = 1 +
  ceil(4(61 + pad16(n))/3). It agrees: the most inline bytes are 128, a
  spilled field of 19 bytes spells 125 characters, `par-` is 115 (253
  characters) at F=16 and 179 (339, spills) at F=32, and `pcs-` is 115 at
  F=32. Two small corrections. `seg-` is 49 bytes, not 50. `.pfcopy` is
  only ever added to a resumed Log file's name (design.md §4), so
  "`par-` + `.pfcopy`" is moot.
- Constructor list, re-derived from the fence sources rather than from
  `format!`. Every fence comes from `fence_lo` (db.rs:5034, 13026, 13080,
  19954) or from the Slicer's cut at `FENCE_MAX` (db.rs:12964). There is no
  `fence_hi` function: db.rs:13055 names one, but only in a comment. A
  promotion's name (db.rs:40737) copies fields 1 and 2 of the old name and
  takes fences of the same kind. A grep of `with_extension`, `join(&name)`
  and `push_str`/const names found nothing longer than `par-`. The list
  stands.

## Model extension: cut at every barrier (`cutk=k`)

A lowercase op used to die only at its first barrier, and an `L` branches
only on the end-of-op state. So no run went on from a process crash or a
power cut at an op's later barriers. With `cutk=k`, the op dies at its k-th
barrier instead.

**Finding (violation of UNNAMED, i.e. the open refuses a store over a
durable manifest).** The design at `cutk=2` and n=4 gives 5,797 UNNAMED
(subset), first ops `rXC`. At `cutk=3` it gives 3,088. Under prefix
(ext4 order) it gives 324 at both `cutk=2` and `cutk=3`, first ops `CrXC`.

1. `r` runs rename-over to the point where its rename has run and its
   `sync_dir` has not, and the process dies there (`X`).
2. The open finds two entries for the name in the page cache. It keeps the
   higher ns and unlinks the lower (design.md §4 "keeps the higher name_seq
   and unlinks the other"; the spill's §5.5). It syncs neither the kept
   entry nor its record first.
3. A power cut follows.
   - **Subset**: the unlink lands and the rename does not. This is
     design.md's own rename-over, and inline names fail too (`rXC` is
     `n0`).
   - **Prefix**: the rename and the unlink both land, and the kept spilled
     entry's record does not. This one is specific to the spill: under the
     prefix order an inline name survives, a spilled one does not.

Either way, the name a durable manifest holds has no resolving entry. The
work's reported n=5 result misses this, because no run went on past the
first barrier.

**Fix that I checked**: `open_sync=True`. Before it unlinks any duplicate,
the open fdatasyncs the files of the kept spilled entries and then fsyncs
the directory. Results:
- 0 violations at n=4 for `cutk` 1-4, both subset and prefix;
- 0 at n=5 for `cutk` 1, 2 and 3 (subset): 9.2M, 5.2M and 4.3M any
  states.

It costs one `sync_dir` (plus one fdatasync per spilled kept file), and
only at an open that finds duplicates. This is the engine's own rule (the
open removes nothing until what it keeps is durable), applied to the
layer's duplicate unlink.

## Mutants of my own (all at n=4, the fix on)

| mutant | result | reading |
|---|---|---|
| `dslink=False`: a link's record left out of the `sync_dir` rule | caught: 74,681 UNNAMED (prefix 49,960), first `P` | the rule is load-bearing on the link path specifically |
| `keep=low`: the open keeps the lower ns | caught only once (1 AMBIG, `RLXR`) by the work's checks. I added a check, KEPT_OLDER (the entry the open keeps is the newest made): 426 states, first `RLC` | **check gap**: the work's AMBIG judges the max-ns entry and never what the open keeps |
| `rfree=first`: a's slot is freed before b's is chosen in rename | not caught (0 at `cutk` 1 and 2) | §3's "a's slot is still referenced" is held by no property. The only spilled-source rename is a temp (`T`) |
| `ns_open=resolved`: reseed ns over resolved entries only | not caught (0) | with rtag binding, design.md's "max *listed* + 1" is not load-bearing for the spill |

The work's own six mutants: I reran M2 only. The other five I read in the
code (`resolve`, `alloc`, `open_store`) and did not rerun.

## Design gaps found by reading

1. **A finished file has no descriptor** (design.md §4, Handle: once
   finished, a read-only mapping and no descriptor). A promotion's `link`
   of a finished `par-` has to write a name record into that file and
   fdatasync it. The design does not say how the layer gets a writable
   descriptor, nor that `FINAL` does not cover the name pages. Those pages
   lie outside Once trust and should be stated so.
2. **§5.4's safety sentence is stronger than what was shown.** It says
   "record not resolving means no `sync_dir` returned after it was made".
   That is false in two cases:
   - for a ghost whose slot was reused, which §3 itself allows;
   - under damage, where a flipped byte in a durable name page makes the
     entry unresolved.

   In the damage case, the entry is presented as `spill-*.tmp` and swept,
   so the engine's evidence check on unnamed segments
   (`Db::unnamed_refusals`) never judges it. Alone this matches what the
   engine does with an unnamed segment. Combined with a damaged newest
   slot, it removes a merge output that the engine would have refused
   over. This is not modelled: the model has no damage.
3. **The open's cost claim is unshown.** The claim is that the name read
   is "merged with the header read, no extra round trip". But design.md
   §4 Open opens headers lazily, at a file's first open (step 5), and
   names are resolved at listing (step 2), before the manifest is known.
   Merging the two needs eager header reads for spilled entries. The
   `fills <= 6*files` bound is kept by defining name reads as not being
   fills.

## Not checked

- The design at n=5 with `cutk` ≥ 2. n=4 already shows the violation.
- The fix at n=5 with `cutk` ≥ 4, and the fix under prefix at n=5.
- Concurrency, failed writes or syncs, damage, and the `.pfcopy`/Log
  interaction. These are the work's own not-verified list, and I did not
  model them either.
- Whether any engine path names a file in a manifest without a `sync_dir`
  after its directory op. I read the nine `sync_dir` sites in db.rs and the
  direct run's link (db.rs:30405). I did not audit every publish.
- Whether Linux ext4 data=ordered would in practice order the record
  before the rename. The model is deliberately conservative.
