# Lead: the forms' memory over a store a piece spans (pre-registration draft)

CELL. The process's resident memory during the lag sweep over a store a shuffled load left: 960 MB
(default), 840 (cache256), 953 (noadvice) against a 400 MB store at 3M keys; the arm with no cache 417.
Under a 1 GiB bound every supdb arm but nocache is killed at 3M; at 10M the forms would be ~3 GB.
The bounded rows cannot carry the default until this moves, and a user with a store near memory
pays twice for it.

DECOMPOSITION (to run, SUPDB_LAG_COUNTERS=1 at 3M, whole scan counts, one process per arm):
 1. Accounted vs resident: `block_cache_bytes()` (the handle's accounted contents) beside the group's
    rss at each lag point; the gap is slack (Vec capacity), the published copies (State.forms, one per
    handle's publish), and the arenas' retention.
 2. Blocks built per lag pass (`built up/eng/rd`) and the forms' kind: how many blocks are dense copies
    (`Cached::Block`, > CACHE_DENSE=16 overlay keys) after the drain's spanning piece -- expected: all.
 3. Why a piece spans the store after the drain: the shuffled load's drain leaves its last seal as a
    piece (flush_schedules hands the merge to the background, published at the writer's next commit);
    until then every block has overlay keys and the first scans copy the whole store.
 4. Per-key bytes of a dense form: keys 16 + vals 104 + ents 16 = 136 B/key before slack.

CANDIDATE CHANGES (price separately, each an arm):
 A. The budget counts memory: capacity not len, and the published copies counted against the
    publisher's budget; shed by the same rule. (Makes cache256 mean 256 MB.)
 B. Shrink to fit: forms built with exact capacities (`Vec::with_capacity` from the counts the
    build knows), no doubling slack -- the fresh-page rule from the copies (`Options::copy_exact`) again.
 C. The drain merges its spanning piece before answering (or the first scan after a drain triggers
    the scheduled merge rather than waiting for a commit), so the store after a load is Clean and
    the forms hold nothing until writes come; priced on the load's sync phase and the lag0 pass.
 D. A default budget as a share of the cgroup/machine memory (what LMDB gets for free from the
    page cache): the forms are a second-level cache and should yield to the page cache under pressure.

PREDICTIONS: A+B halve the resident forms at 3M (960 -> ~500); C takes the lag0 pass's forms to
zero and moves the drain by the merge's cost (a 400 MB rewrite ~ 0.5-1 s at 3M); the quick rows'
read cells unchanged (in-core); the bounded rows carry every arm.

VERDICT: the memory probe by phase (deterministic), the lag counters, and `bench ab` at 300k/1M for
the rates, two sittings. STOP RULE: a change that moves a quick cell by more than its CI in both
sittings against the default stops and is reconsidered.
