# Working in this repository

Supdb is a read-optimized embedded key-multivalue store: the engine, its
reader, and under `bench/` the suite that measures it against LMDB, RocksDB
and FASTER. This file is the rules for working on it. `bench/CLAUDE.md` has
the suite's, `bench/DESIGN.md` says how a thing is measured, `ROADMAP.md`
holds the open work, and `docs/engine.md` the design and its measurements.

The incident behind each rule is in this file as it stood at `09cadec0`. A
pointer elsewhere of the form (`CLAUDE.md`, *entry*) names an entry there.

## Writing

- Cite a measurement as the command that reproduces it at the commit it ran
  at (`bench/scripts/pair.sh A B SIZE [options]`), never as a log. Main is
  merged by squash, so a cited commit lives in its pull request's history:
  `git fetch origin '+refs/pull/*/head:refs/remotes/pull/*'`.
- `README.md`, pull request descriptions and the crate docs are factual,
  current and simple: no standing figures, no history, no narrative of the
  change. A figure belongs in a row; a figure attached to a change belongs in
  the pull request that made it, rounded.
- Open work goes in `ROADMAP.md` and nowhere else. A cell a comparator wins
  is an open target there. Nothing is a standing limitation: a quantity the
  design is willing to lose is stated in `bench/DESIGN.md` as an acceptable
  loss, a price paid for something, never a verdict that it cannot be won.
- Quote the ordered load and the shuffled load together, never one alone.
- A duration nobody timed is not written down as measured, in a commit
  message least of all.

## Checks

- In a fresh container run `sh scripts/deps.sh` first and gate on its
  `all deps ready` line. Every install goes through `scripts/apt.sh`.
- `sh scripts/check.sh` is the one definition of the checks (build, test,
  stride, onefile, lint, wasm, bench, lists), and CI calls it by the same
  group names. Never
  write a second definition. `quick` is a timing run, outside the default
  set, and needs the machine to itself. Nothing edits the tree until a
  gate's last line: later groups compile the tree as it stands.
- Every gate here has broken the same way: a check that was not running, or
  one reporting a verdict it had not earned. A script's verdict is its own
  exit, "differs" is told apart from "did not run", and both fail.
- Every group denies warnings. Only the door's seven modules carry a
  blanket allowance, of the door's two lints; any other allowance is an
  item's, with its reason, counted in `tests/data/door.txt`. Nothing is
  exempt from the format gate. The suite's own lint runs in the `bench`
  group, not `lint`.
- The toolchain is `rust-toolchain.toml`, installed and enforced by
  `scripts/toolchain.sh`; `RUSTUP_TOOLCHAIN` is refused. Moving it is one
  commit, with what the new clippy asks for fixed and the whole gate run.
  The manifests' `rust-version` is the oldest supported and does not move
  with it.
- Tests run under `--profile checked` (release with overflow checks and
  debug assertions). The wasm module and the suite's timed binaries stay on
  `release`.
- A change to an engine default lands only once the suite has read it
  better and `sh scripts/check.sh` has passed with it in place. Before a
  default's quick row, open the whole roster at the smallest rung with one
  rep.
- The gate is of the tree, not of the default: a commit that adds a test,
  an arm's included, runs the groups that run it. The `stride` group runs
  every test again with a form file serving the store.
- Outside the default set: `cost` (`deps.sh profile`), `enc` (`deps.sh
  openssl`), `cross-arm`, `machine` and `fscrypt`, on the filesystem `sudo
  sh scripts/fscrypt.sh up DIR "$(id -u):$(id -g)"` makes.
- `lists` holds `tests/data`'s lists, which only go down, and `cost` counts
  a store in the clear by function, both against `scripts/base.sh`'s base;
  a raise is declared in `scripts/raises.txt`. A new toolchain or
  `bench/.cargo/config.toml` is a new compiler: count again.
- `lint` type-checks every target for macOS from Linux. A Linux-only call,
  and a test that makes a store under a cipher, go behind
  `cfg(target_os = "linux")`.
- A switch's green (`SUPDB_TEST_STRIDE`, `SUPDB_TEST_ONEFILE`) is read after
  a count that its mechanism fired. The fuzz runs past the gate's ranges
  under `ZFUZZ_CIPHER=1` and `ZFUZZ_ONEFILE=1` before a change is called
  done.
- A merge clean by text is read for where one side's mechanism answers an
  ask the other waits on or skips a count the other moves, and then every
  binary runs whole, with `lists`, `cost` and the fuzz.
- `cross-arm` runs under qemu, which answers `madvise` without a kernel. A
  test that reads the kernel's answer to advice (`smaps` flags, `mincore`
  after an ask) puts that part behind `cfg(supdb_emulated)`, and is run
  under qemu before it is pushed, with `RUST_BACKTRACE=0`.
- Every CI job has a `timeout-minutes`. An overdue hosted job is read by its
  groups after it ends before it is called a hang.
- Use `--no-fail-fast`. A test binary that ends with 101 and no
  `test result` line is a watchdog's exit if it printed the watchdog's
  line, and a full disk until `df` says otherwise if it did not. The dev
  profile is for one test at a time, never the whole `db` binary.
- A test hands its callback to a generic read call through `tests/common`'s
  `kv` or `vf`, never a closure straight in: each closure compiles another
  copy of the read path. A slow compile is measured (`cargo build
  --timings`, `nm -C`) before it is accommodated.
- A worktree gets a target directory of its own and starts with
  `git switch -C <branch> <base>`; a result is believed only after `--list`
  shows the tests it names.
- Every commit carries a verified signature, which `lists` holds for every
  commit after the base (`scripts/signed.sh`): `git commit`, `merge` and
  `cherry-pick`, never `git commit-tree`. Mutants run in a worktree of their
  own, and a mutant's verdict is taken with nothing else on the machine.

## Disk

- Tests leave stores at `$TMPDIR/supdb-next-<name>-<pid>`; remove them
  between runs. On a shared box give each run a `TMPDIR` of its own, since a
  glob takes other runs' stores mid-test, spelled as the kernel resolves it
  (`pwd -P`). A killed `bench ab` leaves
  `$TMPDIR/supdb-ab-<pid>`; remove it by name.
- `ld terminated with signal 7 [Bus error]` means the disk allowance is
  spent, whatever `df` says. Clear old bench binaries, test stores, the
  checked profile's examples and `~/.debug` (`perf record -N` keeps
  nothing there). Read `df` of the temp volume before a timing run.

## Measuring

- Callgrind counts instructions exactly and is blind to waiting; `perf
  record -e cpu-clock` gives time. This is a Firecracker guest with no
  hardware counters. `apt-get install linux-tools-6.8.0-31` gives a perf.
- Toggle callgrind on an exactly named function, count what the toggle
  collects, read a recursive function by its self rows, and divide a phase's
  instructions by the phase's own operations. Valgrind runs one thread at a
  time on a slow clock: a difference between its store and the native one
  is established by counting both.
- Test a hypothesis about where time goes with a count before a fix is
  timed. Count first-touch pages per key before instructions per key; a
  fresh page's price is the host's and moves between sittings.
- Price work by the sum of the phases, across threads: work moved off a path
  is paid by whoever inherits its pages, its waits or its faults.
- Price a sync by who issues it and when, read `strace -f -tt -T -y` before
  reasoning about syscalls, and stamp each hand-off and device wait before
  pricing either. Freeing an inode (rename over, unlink, truncate) is a
  device round trip here.
- A pass beside other threads is profiled cut to its window and its thread
  (`SUPDB_LAG_MARK`, `perf record -k CLOCK_MONOTONIC`), with the thread's CPU
  clock beside its wall.
- Time a mix's kinds of operation apart, and a short pass's first operation
  on its own, before naming what is slow. A cell that reads two shapes is
  two shapes: find the count that separates the reps before averaging, and
  read a cell by its reps before its median.
- Give a probe the suite's shape (value size, key shape, the `sync`, the
  points before it) before believing it disagrees with the suite, and read
  nothing between the phases the suite times.
- Time a decoder, or any branchy walk, over more distinct inputs than the
  branch predictor holds.
- Before a lever on misses, lay the samples on the loads and ask which
  earlier load each address waited for; price a load by its self time over
  its count against a miss's latency.
- An option the suite sets from the environment is set for both arms:
  check that the arms differ in the option before reading a verdict. A cell
  against a comparator, and a profile, are taken with the counters off.
- Read a pairing's stderr by the arm each line names, never by line parity.
- Price a structure by taking it away in the suite's shape, and price an
  instrument in the measured path the same way.
- An option's verdict is read only after the count that says its mechanism
  fired in its arm. A pairing's control is the cells the option cannot
  reach: an effect no larger than their lean is unresolved.
- A cell under a tenth is not resolved by one pairing here: the host's two
  speeds and memory placement move cells by that much. A rate is the median
  of alternated runs of two binaries, and a verdict needs a second sitting.
- Under a bound, read memory by owner and by phase (`lagcensus`, the group's
  `memory.stat`, `smaps` sampled from outside the process); what the census
  leaves unexplained includes the kernel's own memory.
- An ask of the kernel (`WILLNEED`, a drop) is held to the kernel's count
  after it (`mincore`, `pagemap`) and repeated where it fell short. Resident
  is not mapped.
- A stall or a kill is named by who waits on whom before any lever: each
  thread's state and kernel stack sampled (`scripts/stallwatch.py`), and
  the group's `memory.stat` every 10 ms up to a kill. Flat memory at the
  end is a wait.
- A count that moved with its source unchanged is read by instruction
  (`--dump-instr=yes`) beside its callers and inlined callees, then
  bisected, before it is called the compiler's.
- A bar that names a competitor is paired against it before the design is
  fixed, and a choice made off is retaken when a mechanism it stood on
  changes.

## The suite gates this repository

- The comparison arms in `Options` are not dead code: the suite prices a
  new shape against the old one in one process. Check `bench/src/engines.rs`
  before removing one.
- A fix for a wrong answer is not an arm.
- An option sound only under another option's value is checked where the
  store opens. An arm that keeps an old shape keeps that shape's other
  settings.
- A second form of a structure may accompany the records only if the other
  paths never touch it. It is built only when what the store measures calls
  for it, and taken back when the measure turns.
- An arm kept for pricing is a shape a store can be opened in: its crash
  states are built as the default's are, under every mode it can be set
  beside.
- A feature switched off under a mode is a claim about today's code: the
  part the mode breaks is given another way before the rest is given up.
  Two options answering one question are one.

## Invariants a change must not break

- **The read path is synchronous.** No `async` under `blob`, `flatindex` or
  `bytes`: the `await` lives in JavaScript. Format knowledge stays in Rust;
  the one exception, the envelope's layout in `web/crypt.mjs`, is held to
  the record file below.
- **Two publishers, one pointer.** The writer and the segment work publish
  one `State` by compare-and-swap. Nothing a reader follows moves, except
  what an epoch licenses. Every reader pins; a replaced state is freed only
  past every pin, and nobody waits for a reader, except under a cipher,
  where the evictor waits out pins before it frees. A read takes the state
  once. A `#[cfg(test)]` check holds `State: Send + Sync` and `Reader: Send`,
  which keeps cells out of shared structures.
- **The writer's upkeep is lent, never shared.** One word says which thread
  holds it; every hand-over is one operation on that word. The writer never
  takes a lock the upkeep thread holds. `settle` joins the upkeep before
  anything else. A pass publishes its forms at the commit the writer named.
- **`Blob::zero_copy()` stays true on the native path** (`tests/blob.rs`).
- **The three readers** (`Blob` mapped, `Blob` copying, `SparseBlob`) are
  tested against each other, never against themselves.
- **The magic moves when an old reader would misread rather than error**,
  and moves `STORE_FORMAT` with it; `STORE_FORMAT_OLDEST` says what a
  release still reads. Decide which case a change is before writing it.
- **A checksum must be able to see the corruption it is for.** A row named
  with an impossible shift is damage, not absence.
- **Both callers of `SegmentWriter` emit the same shape** unless a
  measurement says otherwise.
- **`count_fixed` claims a count only when two independent quantities
  agree.**
- **Crash discipline is an order.** A commit is a WAL append and one
  fdatasync. A seal writes a temp name, renames, publishes, fsyncs, then
  writes the manifest slot; no manifest is written while a seal's segments
  are published and unsynced, and seals land in the order handed. Replay
  applies a batch whole or not at all. A run of ordered ingest takes
  sequence numbers, the open walks frames and runs as one history, a sync
  that acknowledges covers everything written before it, a run's temp name
  is behind a directory barrier before its first acknowledged commit, and a
  recovery makes durable what stands before it first.
- **The held log** (`SyncPolicy::Explicit`): a held batch counts as an
  unsynced frame, goes to the file it committed under, and is dropped
  unwritten only below a sequence a manifest on the device covers.
- **One open at a time.** The `LOCK` is taken under `flock` before the open
  reads the directory and given up last, by an unlock from the pid that took
  it. `Db::crash` returns only once nothing of the store runs.
- **A frozen table is one nothing writes.** A shared table is closed to
  writes and its writers waited out before it is published frozen. While a
  shared table stands, nothing is built that a handle's write cannot reach.
- **A file a store names never changes.** New bytes go to a new file renamed
  over the old one; checkpoints share inodes.
- **A store in the clear pays nothing for encryption**: not an instruction
  on any read path or form, counted by function (`cost`). Cipher-only
  knobs live in `StoreCipher`.
- **The door and the wall.** No file, mapping, advice, sync or `/proc` read
  in the library outside `vault`, `bytes`, `pager`, `crypt`, `dirkey`,
  `blocks` and `onefile` (`scripts/door/`), and no form module names the
  encryption or the reverse (`tests/data/couplings.txt`).
- **The envelope is on record.** `tests/crypt.rs` and `web/crypt.test.mjs`
  hold the writer and the browser's reader to `tests/data/crypt-v1.bin`; a
  layout change is a new version and a new file beside it.
- **Under a cipher, pins hold the cache.** The evictor frees a unit only
  past every pin and `pager::Hold` taken before its mark, so each wait
  under one says it holds nothing (`Reader::unlent`, `Hold::idle`).
- **A failure is kept, never retried.** A failed sync, barrier or seal
  step, or a write that cannot be put back, is kept (`Db::lost`) and
  answered to every commit, `sync` and `flush` after it until reopen.
- **An open decides before it removes.** It takes nothing it replayed as
  durable, reads a zeroed first sector as an empty log, and removes or
  writes nothing until its slot is checked against the directory.

## Rules the bugs left

Identity and position

- A cache derived from an object is keyed by that object's identity, never
  by its position, range or address: a freed object's place is the next
  object's, and a thing of zero length shares an address.
- A position taken against an object is taken again when the object is
  replaced, and so is every claim a carried structure makes about it,
  flags included.
- An order a rule rests on is the order of the quantity the rule is about,
  asserted where it is made.

Publishing and copies

- Two copies, one kept and one handed out, need a tombstone when the kept
  one is dropped. Whatever changes the facts an absence is about, by
  dropping or by adding, drops the claim that gives the absence its meaning,
  in the state it holds.
- Quantities a reader must take together are published as one mark and
  taken as one. A structure kept current by a log is current to a position,
  and a reader reads to the position its isolation names.
- What a publish is to derive is derived before the publish is visible.
- What a reader that runs behind will need is kept by the side that retires
  it, until the reader says it has passed it; the reader frees it when done.
- A flag that says a structure exists is set by the code that makes it.

Concurrency

- A word one thread writes on every operation lives on a cache line no other
  thread writes. A statistic of a shared structure is kept by whoever counts
  and summed where it is read.
- A hand-off where each side publishes and then reads the other's is the
  store-buffering shape: it needs a sequentially consistent fence and a
  litmus test that fails without it.
- When a second writer comes to a structure, list everything the one writer
  maintained by being the only one and give it to the second.
- A worker reports progress for work it did, never for a turn it took. Work
  a step leaves to the next step is recorded as owed where it is left.
- A thread that serialises several publishes blocks on none of them. When
  joining several workers where one join resets another, join that one
  first.
- Where work is handed to another thread and the giver goes on, whatever
  orders the giver's later effects is finished before the hand-off.
- A bound on how long a sleeper sleeps is enforced by whoever changes the
  condition the bound is for.

Durability

- A sync is issued by the policy, never by the shape of the call that
  happens to need the write. Every sync's result is checked by the path that
  issued it.
- Read back is not synced: a flag that says durable is set only by a sync
  this process issued.
- A recovery path is profiled when a workload first takes it, and tested at
  each of its own crash windows.
- What the kernel keeps on an open file (readahead, advice, a writeback
  error) is shared by everything made through it: each reader or syncing
  thread gets an open of its own, and a file is named only after its
  writer's handle synced.
- Of two copies kept so one stands while the other is written, the one
  written is the one not known durable by a sync that returned, never one
  chosen by a counter.
- A durability call is gated on what is unsynced in every log a commit can
  be in (the WAL, the runs' files), never on which structure is live.
- A quantity guarding an order between two logs (the greatest key) is kept
  by every write that reaches either, a delete's included.
- A step that can be refused is asked before anything it consumes is
  taken, and a call that fails part way puts back what it changed.
- Under a cipher a file being written is a memory file, whose fsync makes
  nothing durable: durability comes from the log, the manifest and an
  envelope written before a manifest names it.

Performance shapes

- Price a sort, search or merge over keys behind pointers by its misses,
  and make a batch of searches in the structure's order.
- A walk over two sorted sides stops when the other side is spent.
- In a loop measured in cycles, read the assembly for divides, indirect
  calls and loads from bytes just stored in pieces; keep hot results in
  scalars.
- A fast path is gated on the property that makes it valid. A fast path
  keyed on a property of the suite's keys (`whole_key_at`, `uniform`) is a
  slow path the suite has never run: list such sites and run a second key
  shape over every pass.
- A structure kept for readers is gated on readers being there, on every
  path that builds it.
- A per-pass cost is priced at the rate passes come, and an ask ahead is
  sized by the work between the ask and the touch.
- When a change fixes a shape on one path, check every path that shares the
  shape the same day. A change to what a record holds is priced on every
  path that reads one.
- When a step moves earlier or a saving lands, list what the old placement
  or the old cost was triggering.
- A threshold moved for one cell is priced on every cell that crosses it,
  and a cell is read against every fixed count on its path.
- A new question on a hot path rides a test it already makes. A cold arm
  is priced in the hot loop beside it, and moved out.
- An advice reaches every cache its meaning covers, and a reader past a
  cache asks the cache first.

Bounds and memory

- A budget is held against the allocator's bytes in use by owner and the
  process's anonymous bytes, kept at every site that grows what it bounds,
  and drawn from what the other tenants measure to need. A cache bound below
  its working set is slower than no cache.
- A limit kept as a count is a limit in bytes only while the things counted
  keep their size.
- Whatever owns a pointer it will free is a type with a `Drop`.
- A library gives its large transient buffers back itself.
- A budget near a limit is steered by the platform's reading of what is
  limited, not the store's own count, and a writer that knows what it will
  write says so before its first byte (`vault::expect`).

Tests

- A test that needs a state holds it through a seam. A state it only needs
  to have happened is waited for by the count it leaves, with the baseline
  read before the event is caused, covering the whole turn the test reads.
- An injected clock moves only once no job of the system's is pending.
- A fix for a race is held to the race made wide before it is believed. A
  flake is reproduced by choosing its phase, beside the load it failed
  under, not by repeating the run.
- A clean result proves nothing about a path the test never took. A test's
  premise is asserted at the fixture.
- A seam that stands in for an event leaves what the event leaves.
- A check against a defect the code cannot produce is held to firing by
  damaging its input; a check behind a checksum is tested with a forged
  checksum; a compare a hash reaches only on a collision is tested with the
  collision built. A check is written once.
- When two mechanisms can reach one end state, assert at the moment only the
  first has run.
- A test that counts a process-wide effect gets a binary of its own.
- A call site no real input reaches red is a function a test can hand a
  liar.
- Fuzz draws from the whole of the type: the empty key, the empty value,
  `0xff` keys. Two functions that hand out the same resource take the same
  guard.
- A controller is tested with the inputs it moves fed back, every constant
  moved both ways, and mechanical mutants run beside the author's list. A
  surviving mutant is first a question about the code.
- A model's prediction is printed beside the measurement of the same
  quantity.
- A test that passes at its first run is believed once its mutant fails
  it. A hang is no kill until the test, and every thread it parks, has an
  end.
- A failure path's test fails every step of it, those after the publish
  above all, and a refusal is tested by its words and a directory left as
  found.

Interfaces

- A wasm `u32` arrives in JavaScript signed: normalize to unsigned at the
  boundary. Host imports name `wasm_import_module = "env"`. The host ABI
  refuses an object of 4 GiB or more at open.
- A slice that may be empty is not handed to a byte compare without an
  `is_empty` check: an empty `Vec`'s dangling pointer costs a failed page
  walk.
- A quantity with a name is used under that name, and a quantity derived
  from a container goes through the accessor that knows which container
  holds it.
- A word whose meaning a change moved gets a type, so that old uses stop
  compiling.
- A measured library is built with the features it ships with, and the
  binary says which it linked.
