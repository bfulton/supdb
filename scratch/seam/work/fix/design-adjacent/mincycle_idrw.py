"""The cheapest steady state of onereq.py's model: minimum mean pages written
per commit over all infinite schedules, with `nint` intervals a commit
(1 = one request, 2 = the record may go in an interval of its own).
The record is a contiguous run of pages inside one interval.
Positions relative to the tail page's home after the commit. Durable policy:
the previous commit's record is protected. Cost of a commit = pages written
(home writes + record + identical rewrites + filler).
Variant: identical rewrites of the protected record allowed.
Usage: mincycle_idrw.py m pmax rec_extra W nint
"""
import sys, itertools

def actions(pend, prot, lo, hi, pmax, rec_extra, nint):
    """Yield (after, rec, cost, desc)."""
    def intervals():
        for a in range(lo, hi + 1):
            for b in range(a, hi + 1):
                yield (a, b)
    ivs = list(intervals())
    combos = [(iv,) for iv in ivs]
    if nint == 2:
        combos += [(x, y) for x, y in itertools.combinations(ivs, 2) if x[1] + 1 < y[0]]
    for combo in combos:
        cover = set()
        for a, b in combo:
            cover |= set(range(a, b + 1))
        cost = len(cover)
        for a, b in combo:
            for ra in range(a, b + 1):
                for rb in range(ra, b + 1):
                    rec = set(range(ra, rb + 1))
                    if any(not (p in pend or p >= 0) or p in prot for p in rec):
                        break
                    after = {p for p in pend if not (p in cover and p not in rec and p not in prot)}
                    if len(after) > pmax:
                        continue
                    if len(rec) != len(after) + rec_extra:
                        continue
                    yield frozenset(after), frozenset(rec), cost, (combo, (ra, rb))

def build(m, pmax, rec_extra, W, nint):
    lo, hi = -W, W
    start = (frozenset(), frozenset())
    edges = {}
    todo = [start]
    seen = {start}
    while todo:
        st = todo.pop()
        pend0, prot0 = st
        pend = {p - m for p in pend0} | set(range(-m, 0))
        prot = {q - m for q in prot0}
        out = []
        if min(pend, default=0) >= lo:
            for after, rec, cost, desc in actions(pend, prot, lo, hi, pmax, rec_extra, nint):
                nxt = (after, rec)
                out.append((nxt, cost, desc))
                if nxt not in seen:
                    seen.add(nxt)
                    todo.append(nxt)
        edges[st] = out
    return start, edges

def min_mean_cycle(nodes, edges):
    # Karp on the subgraph of nodes; returns (mean, cycle)
    idx = {v: i for i, v in enumerate(nodes)}
    n = len(nodes)
    INF = float("inf")
    D = [[INF] * n for _ in range(n + 1)]
    P = [[None] * n for _ in range(n + 1)]
    for i in range(n):
        D[0][i] = 0
    for k in range(1, n + 1):
        Dk, Dk1 = D[k], D[k - 1]
        for v in nodes:
            iv = idx[v]
            if Dk1[iv] == INF:
                continue
            for w, c, desc in edges[v]:
                if w in idx:
                    iw = idx[w]
                    if Dk1[iv] + c < Dk[iw]:
                        Dk[iw] = Dk1[iv] + c
                        P[k][iw] = (iv, c, desc)
    best, bv = INF, None
    for i in range(n):
        if D[n][i] == INF:
            continue
        worst = max((D[n][i] - D[k][i]) / (n - k) for k in range(n) if D[k][i] < INF)
        if worst < best:
            best, bv = worst, i
    if bv is None:
        return None, []
    # walk back from bv to extract a cycle
    walk = []
    v, k = bv, n
    while k > 0 and P[k][v] is not None:
        u, c, desc = P[k][v]
        walk.append((nodes[u], c, desc, nodes[v]))
        v, k = u, k - 1
    walk.reverse()
    seen = {}
    for i, (u, c, d, w) in enumerate(walk):
        if u in seen:
            return best, walk[seen[u]:i]
        seen[u] = i
    return best, walk

if __name__ == "__main__":
    m, pmax, rec_extra, W, nint = (int(x) for x in sys.argv[1:6])
    start, edges = build(m, pmax, rec_extra, W, nint)
    alive = set(edges)
    ch = True
    while ch:
        ch = False
        for s in list(alive):
            if not any(n in alive for n, _, _ in edges[s]):
                alive.discard(s)
                ch = True
    # keep only nodes reachable from start inside alive
    reach, todo = set(), [start] if start in alive else []
    while todo:
        s = todo.pop()
        if s in reach:
            continue
        reach.add(s)
        todo += [n for n, _, _ in edges[s] if n in alive]
    nodes = sorted(reach, key=lambda s: (sorted(s[0]), sorted(s[1])))
    if not nodes:
        print(f"m={m} pmax={pmax} rec_extra={rec_extra} W={W} nint={nint}: no infinite schedule")
        sys.exit()
    mean, cyc = min_mean_cycle(nodes, edges)
    print(f"m={m} pmax={pmax} rec_extra={rec_extra} W={W} nint={nint}: states={len(edges)} "
          f"live={len(nodes)} min pages/commit={mean:.3f} (data {m} + record floor {rec_extra})")
    for u, c, d, w in cyc:
        print(f"   pend={sorted(u[0])} prot={sorted(u[1])} -> write {d[0]} rec {d[1]} cost {c}")
