#!/usr/bin/env python3
# Scratch experiment, not a proposal for the engine: `same_blocks` compares
# every key of the two partitions, not the keys at the blocks' first ranks and
# the last, so a rewrite that changed keys inside a block (one inserted before a
# filed key, one removed after it, the count and the ends unchanged) is not
# taken for the same blocks and the table built over the old blob is dropped.
import sys

p = sys.argv[1]
s = open(p).read()


def rep(old, new, count=1):
    global s
    assert s.count(old) == count, (old[:80], s.count(old))
    s = s.replace(old, new)


rep(
    """        (0..n)
            .step_by(CACHE_BLOCK)
            .chain(n.checked_sub(1))
            .all(|r| matches!((a.blob.key_at(r), b.blob.key_at(r)), (Some(x), Some(y)) if x == y))""",
    """        // SCRATCH EXPERIMENT: every rank, not the blocks' first and the last.
        (0..n).all(|r| matches!((a.blob.key_at(r), b.blob.key_at(r)), (Some(x), Some(y)) if x == y))""",
)
open(p, "w").write(s)
print("applied")
