"""Is there a steady state in which every Log commit is ONE contiguous stored
write, data pages at fixed 1:1 homes, the commit record inside that write?

Abstract model, page granularity, durable policy (every commit synced), one
writer, no other sync in flight. Positions are stored data positions relative
to the tail page's home after the commit (rel 0). A commit completes m pages:
the old tail and m-1 more. A complete page is HOME (its final image at its
home) or PENDING (its image carried in the record). The previous commit's
record R_prev is the newest durable record: no byte of it may be overwritten
by this commit's write (a torn overwrite destroys the only durable copy of the
old tail and of the open group's tags; unsynced sectors land in any subset).

A commit writes one interval I disjoint from R_prev. Inside I:
  - the record Rec: |pending after| + rec_extra pages, on positions that are
    pending homes or at rel >= 0 (never on a HOME page);
  - every pending page whose home is in I and not under Rec goes home;
  - HOME pages in I are rewritten with identical bytes (allowed, costs bytes);
  - positions at rel >= 0 not under Rec are filler (allowed, costs bytes).
|pending after| <= pmax. The search asks whether an infinite schedule exists
(a cycle in the reachable state graph after pruning dead ends).
Usage: onereq.py m [pmax] [rec_extra] [W]
"""
import sys, itertools

def run(m, pmax, rec_extra, W):
    lo, hi = -W, W
    start = (frozenset(), frozenset())          # (pending, protected record)
    succ = {}
    todo = [start]
    seen = {start}
    while todo:
        st = todo.pop()
        pend0, prot0 = st
        # shift by m: positions relative to the new tail
        pend = {p - m for p in pend0} | set(range(-m, 0))
        prot = {q - m for q in prot0}
        if min(pend, default=0) < lo or min(prot, default=0) < lo - m:
            succ[st] = []
            continue
        outs = []
        for a in range(lo, hi + 1):
            for b in range(a, hi + 1):
                I = set(range(a, b + 1))
                if I & prot:
                    break  # extending b only adds more overlap
                cand = sorted(p for p in I if p in pend or p >= 0)
                # choose which candidate positions hold the record
                for k in range(1, len(cand) + 1):
                    for rec in itertools.combinations(cand, k):
                        rec = set(rec)
                        after = {p for p in pend if not (p in I and p not in rec)}
                        if len(after) > pmax:
                            continue
                        if k != len(after) + rec_extra:
                            continue
                        nxt = (frozenset(after), frozenset(rec))
                        outs.append((nxt, (a, b, tuple(sorted(rec)))))
                        if nxt not in seen:
                            seen.add(nxt)
                            todo.append(nxt)
        succ[st] = outs
    # prune states with no surviving successor
    alive = set(succ)
    changed = True
    while changed:
        changed = False
        for s in list(alive):
            if not any(n in alive for n, _ in succ[s]):
                alive.discard(s)
                changed = True
    return start in alive, len(seen), alive, succ

if __name__ == "__main__":
    m = int(sys.argv[1])
    pmax = int(sys.argv[2]) if len(sys.argv) > 2 else 3
    rec_extra = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    W = int(sys.argv[4]) if len(sys.argv) > 4 else max(8, 2 * m + 6)
    ok, n, alive, succ = run(m, pmax, rec_extra, W)
    print(f"m={m} pmax={pmax} rec_extra={rec_extra} W={W}: states={n} "
          f"infinite one-write schedule exists: {ok}")
    if ok:
        # print one cycle
        s = (frozenset(), frozenset())
        path = []
        for _ in range(12):
            for nxt, act in succ[s]:
                if nxt in alive:
                    path.append((sorted(s[0]), sorted(s[1]), act))
                    s = nxt
                    break
        for p in path:
            print("  pending", p[0], "prot", p[1], "write", p[2])
