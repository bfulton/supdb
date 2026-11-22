# T14: "the builds gave nothing back" (tests/cipher.rs), and T25: the mutant it uncovered

## The flake (task 14), fixed in 620e4c5a
the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit read the cache's
evictions after its fill and after `zpages_settle`, and required growth. Seen red twice (the
second time in a gate's stride group at the upstream merge M1; counts in
scratch/plan/build/M1/cipherflake-red.txt, evicted_units 690).
- Trace first (a print of the build and eviction counts at the first reading; trace-natural.log):
  30 to 40 of 93 builds had already run at the reading in six runs of six, 9 to 19 of 73 form
  files present. `ZPages::Always` builds as partitions land (`Options::zpages_eager`, default
  on). The totals after all builds, 661 to 695 evicted units, are the red's 690.
- State forced (the test's thread four seconds late to its reading; forced-before.log): red
  3 of 3, the gate's message, 93 of 93 builds done at the reading.
- Fix: the test runs twice: `zpages_eager` off (every build inside the settle; the delta is
  exact; a build before the reading is itself a failure) and on (builds beside the fill's
  merges; held to having been made, the cache to having turned over). fixed-runs.log: natural
  3 of 3 green, forced 3 of 3 green.
- Mutants (mutants.log): eager on in the variant that should have none: red at "a build
  before the form's settle". A form build that never rests: NOT caught (also not by the 14
  cipher-named tests of the cipher and zpages binaries, m2-wide.summary.log) -> T25.
- Scratch removed; tests/cipher.rs = keep copy by sha256; src/db/zfile.rs restored by sha256.

## The mutant (T25), held in the commit after
See scratch/plan/build/T25/: explore.py (the first shape, six partitions: peak 2.2 MiB with
the rest, 6.2 MiB without, too thin a margin against the cache's own waiting lines at 6 and 8
MiB), newtest.py (one ordered run's segment of 27.7 MiB, built by a reopened store: 2.2 MiB
with the rest, 23.2 MiB without), mutant.log (red 3 of 3), verify.summary.log.
