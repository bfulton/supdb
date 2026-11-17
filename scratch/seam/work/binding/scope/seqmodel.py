#!/usr/bin/env python3
"""Sequence accounting (M2) against no binding (NONE), bounded model.

M2: a direct run takes WAL sequences without writing a frame. Its temp's
name carries s_open (wal.seq at the run's first commit, db.rs:28695 creates
the temp); close_run (db.rs:28917) does wal.seq = s_open + k before end_seq
is read (28919), k = commits (one record a commit here). The next WAL
record therefore carries s_open + k, and the run's segment is named at it.
The open replays a chain: WAL records and the run temps not yet covered,
by sequence, from the manifest's covered (today's continuity check,
db.rs:4661-4677, extended); a gap with something after it ends the log
there (END) or refuses (M2R). The recovering open keeps a temp it does not
need to rebuild: its records go into the replayed memtable in sequence
order and the temp retires by coverage, as an older WAL file does
(db.rs:27912-27919, retiring_tmps 37925).

NONE: today's plain shape (= Pagefile decision 1(a)): run takes no
sequence, segment named at s_open; recovering open rebuilds a temp not
named by id into a piece named at covered and removes the temp
(27717-27770); WAL replayed from covered with the continuity check.

Commits are 1..n in program order; a held state is {1..m}.
Policies: always (every commit syncs its own file: run 28800-28807, WAL
28510) and buf (no sync at commit; SW = sync_wal, SR = a run's due sync).
Fixes for buf (M2 variants, flags):
  A  SR first syncs the WAL when it holds unsynced frames
  B  SW first syncs every closed run file with unsynced commits
  C  the recovering open syncs the files it replayed (open_append counts
     replayed bytes synced, db.rs:4355-4358)
"""
import sys
import itertools
from collections import Counter

VARIANTS = {
    'NONE': dict(m2=False, end=False, keep=False, A=False, B=False, C=False),
    'M2': dict(m2=True, end=True, keep=True, A=True, B=True, C=True),
    'M2R': dict(m2=True, end=False, keep=True, A=True, B=True, C=True),
    'M2noA': dict(m2=True, end=True, keep=True, A=False, B=True, C=True),
    'M2noB': dict(m2=True, end=True, keep=True, A=True, B=False, C=True),
    'M2noC': dict(m2=True, end=True, keep=True, A=True, B=True, C=False),
    # M2 with today's recovering open (rebuild + remove the temp)
    'M2rebuild': dict(m2=True, end=True, keep=False, A=True, B=True, C=True),
    # NONE with only the buffered sync order (no binding): what the
    # plain store's EveryN hole needs, with no damage
    'NONEsync': dict(m2=False, end=False, keep=False, A=True, B=True, C=True),
    # mutant: the temp's name does not carry s_open; the open assumes a temp
    # starts where the chain stopped
    'M2noname': dict(m2=True, end=True, keep=True, A=True, B=True, C=True, noname=True),
    # mutant: the segment named at s_open, the bump after end_seq is read
    'M2late': dict(m2=True, end=True, keep=True, A=True, B=True, C=True, late=True),
}

# state:
# n, promised, wal (tuple of (seq,cid)), wsync (records synced), wknown
# (records the engine believes synced), wseq, mem (tuple cids),
# runs: tuple of (rid, s_open, cids tuple, synced, state, kept)  state:
#   'open' | 'closed' | 'fin' ; kept=True for a temp an M2 open kept
# ropen: rid or -1
# queue: tuple of (kind, sid, end, content frozenset, fin)
# segs: tuple sorted of (sid, end, content frozenset) -- named in newest slot
# slots: tuple of (pub, covered, segs)  (newest last)
# retire: frozenset of rids whose temps go at the next RT
# nid, crs, mgs


def init():
    slot = (0, 0, ())
    return (0, 0, (), 0, 0, 0, (), (), -1, (), (), (slot,), frozenset(), 0, 0, 0)


def unpack(s):
    return list(s)


def covered_of(s):
    return s[11][-1][1]


def close_run(st, v):
    (n, prom, wal, ws, wk, wseq, mem, runs, ropen, queue, segs, slots,
     retire, nid, crs, mgs) = st
    runs = list(runs)
    i = [r[0] for r in runs].index(ropen)
    rid, s_open, cids, syn, state, kept = runs[i]
    end = wseq
    if v['m2']:
        wseq = s_open + len(cids)
        if not v.get('late'):
            end = wseq
    runs[i] = (rid, s_open, cids, syn, 'closed', kept)
    queue = queue + (('run', rid, end, frozenset(cids), False),)
    return (n, prom, wal, ws, wk, wseq, mem, tuple(runs), -1, queue, segs,
            slots, retire, nid, crs, mgs)


def sync_wal(st, v, policy_call):
    (n, prom, wal, ws, wk, wseq, mem, runs, ropen, queue, segs, slots,
     retire, nid, crs, mgs) = st
    runs = list(runs)
    if v['B']:
        for i, r in enumerate(runs):
            if r[4] != 'open' and r[3] < len(r[2]):
                runs[i] = (r[0], r[1], r[2], len(r[2]), r[4], r[5])
    ws = len(wal)
    wk = len(wal)
    return (n, prom, wal, ws, wk, wseq, mem, tuple(runs), ropen, queue, segs,
            slots, retire, nid, crs, mgs)


def successors(st, v, policy, maxn, maxruns, maxcr, maxmg):
    (n, prom, wal, ws, wk, wseq, mem, runs, ropen, queue, segs, slots,
     retire, nid, crs, mgs) = st
    out = []
    # RC
    if n < maxn and (ropen >= 0 or (not mem and len(runs) < maxruns)):
        s = st
        if ropen < 0:
            rid = nid
            newrun = (rid, wseq, (), 0, 'open', False)
            s = (n, prom, wal, ws, wk, wseq, mem, runs + (newrun,), rid, queue,
                 segs, slots, retire, nid + 1, crs, mgs)
        (n2, prom2, wal2, ws2, wk2, wseq2, mem2, runs2, ropen2, queue2, segs2,
         slots2, retire2, nid2, crs2, mgs2) = s
        runs2 = list(runs2)
        i = [r[0] for r in runs2].index(ropen2)
        r = runs2[i]
        cids = r[2] + (n2 + 1,)
        syn = r[3]
        if policy == 'always':
            syn = len(cids)
            if v['A'] and False:
                pass
        runs2[i] = (r[0], r[1], cids, syn, r[4], r[5])
        n2 += 1
        if policy == 'always':
            prom2 = n2
        out.append(('RC', (n2, prom2, wal2, ws2, wk2, wseq2, mem2, tuple(runs2),
                           ropen2, queue2, segs2, slots2, retire2, nid2, crs2,
                           mgs2)))
    # WC
    if n < maxn:
        s = st
        if ropen >= 0:
            s = close_run(s, v)
        (n2, prom2, wal2, ws2, wk2, wseq2, mem2, runs2, ropen2, queue2, segs2,
         slots2, retire2, nid2, crs2, mgs2) = s
        wal2 = wal2 + ((wseq2, n2 + 1),)
        wseq2 += 1
        mem2 = mem2 + (n2 + 1,)
        n2 += 1
        s = (n2, prom2, wal2, ws2, wk2, wseq2, mem2, runs2, ropen2, queue2,
             segs2, slots2, retire2, nid2, crs2, mgs2)
        if policy == 'always':
            s = sync_wal(s, dict(v, B=False), True)
            s = s[:1] + (n2,) + s[2:]
        out.append(('WC', s))
    # SE
    if ropen >= 0:
        out.append(('SE', close_run(st, v)))
    elif mem:
        sid = nid
        queue2 = queue + (('mem', sid, wseq, frozenset(mem), False),)
        out.append(('SE', (n, prom, wal, ws, wk, wseq, (), runs, ropen, queue2,
                           segs, slots, retire, nid + 1, crs, mgs)))
    # SF: oldest unfinished seal finishes and is durable
    for qi, q in enumerate(queue):
        if not q[4]:
            queue2 = list(queue)
            queue2[qi] = (q[0], q[1], q[2], q[3], True)
            runs2 = runs
            if q[0] == 'run':
                runs2 = list(runs)
                i = [r[0] for r in runs2].index(q[1])
                r = runs2[i]
                runs2[i] = (r[0], r[1], r[2], len(r[2]), 'fin', r[5])
                runs2 = tuple(runs2)
            out.append(('SF', (n, prom, wal, ws, wk, wseq, mem, runs2, ropen,
                               tuple(queue2), segs, slots, retire, nid, crs,
                               mgs)))
            break
    # MAN: the oldest seal lands if finished
    if queue and queue[0][4]:
        q = queue[0]
        cov = max(covered_of(st), q[2])
        segs2 = tuple(sorted(segs + ((q[1], q[2], q[3]),)))
        slot = (slots[-1][0] + 1, cov, segs2)
        retire2 = retire | ({q[1]} if q[0] == 'run' else set())
        out.append(('MAN', (n, prom, wal, ws, wk, wseq, mem, runs, ropen,
                            queue[1:], segs2, slots + (slot,), frozenset(retire2),
                            nid, crs, mgs)))
    # RT: retire WAL records below covered, temps of landed runs (and, M2,
    # kept temps whose span end is covered)
    cov = covered_of(st)
    wal2 = tuple(r for r in wal if r[0] >= cov)
    drop = len(wal) - len(wal2)
    runs2 = tuple(r for r in runs
                  if not (r[0] in retire or
                          (v['keep'] and r[5] and r[1] + len(r[2]) <= cov)))
    if drop or len(runs2) != len(runs):
        out.append(('RT', (n, prom, wal2, max(0, ws - drop), max(0, wk - drop),
                           wseq, mem, runs2, ropen, queue, segs, slots,
                           frozenset(x for x in retire if x in [r[0] for r in runs2]),
                           nid, crs, mgs)))
    # MG: fold every named segment into one; covered unchanged
    if mgs < maxmg and len(segs) >= 2 and not queue:
        content = frozenset().union(*[c for (_, _, c) in segs])
        end = max(e for (_, e, _) in segs)
        segs2 = ((nid, end, content),)
        slot = (slots[-1][0] + 1, cov, segs2)
        out.append(('MG', (n, prom, wal, ws, wk, wseq, mem, runs, ropen, queue,
                           segs2, slots + (slot,), retire, nid + 1, crs,
                           mgs + 1)))
    if policy == 'buf':
        # SW: a sync by policy at a WAL commit (sync_wal)
        if ropen < 0 and (len(wal) > ws or (v['B'] and any(r[4] != 'open' and r[3] < len(r[2])
                                              for r in runs))):
            s = sync_wal(st, v, True)
            s = s[:1] + (n,) + s[2:]
            out.append(('SW', s))
        # SR: a run's due sync (d.w.sync), fix A syncs the WAL first
        if ropen >= 0:
            runs2 = list(runs)
            i = [r[0] for r in runs2].index(ropen)
            r = runs2[i]
            if r[3] < len(r[2]):
                ws2, wk2 = ws, wk
                if v['A']:
                    # the chain: every loose predecessor first -- the WAL's
                    # unsynced frames and every closed run's unsynced tail
                    if len(wal) > wk:
                        ws2, wk2 = len(wal), len(wal)
                    for j, q in enumerate(runs2):
                        if q[4] != 'open' and q[3] < len(q[2]):
                            runs2[j] = (q[0], q[1], q[2], len(q[2]), q[4], q[5])
                runs2[i] = (r[0], r[1], r[2], len(r[2]), r[4], r[5])
                out.append(('SR', (n, n, wal, ws2, wk2, wseq, mem, tuple(runs2),
                                   ropen, queue, segs, slots, retire, nid, crs,
                                   mgs)))
    # CR: process crash and recovering open
    if crs < maxcr:
        r = recover(st, v)
        if r is not None:
            out.append(('CR', r))
    return out


def open_store(slot, wal_recs, temps, v):
    """temps: list of (rid, s_open, recovered cids tuple).
    Returns ('REFUSE', why) or ('OK', frozenset of cids, detail)."""
    pub, cov, segs = slot
    content = set()
    named_ids = set()
    for (sid, end, c) in segs:
        content |= c
        named_ids.add(sid)
    if not v['m2']:
        for (rid, s_open, cids) in temps:
            if rid in named_ids:
                continue
            content |= set(cids)
        nxt = cov
        for (seq, cid) in wal_recs:
            if seq < cov:
                continue
            if seq != nxt:
                return ('REFUSE', 'gap')
            content.add(cid)
            nxt += 1
        return ('OK', frozenset(content), nxt)
    # M2 chain walk
    items = {}
    for (seq, cid) in wal_recs:
        if seq >= cov:
            items[seq] = ('w', cid)
    spans = []
    for (rid, s_open, cids) in temps:
        if s_open + len(cids) <= cov or not cids:
            continue
        if s_open < cov:
            return ('REFUSE', 'span-straddles-covered')
        spans.append((s_open, cids))
    spanat = {s: c for (s, c) in spans}
    pending = sorted(spans)  # by s_open == by rid order of creation
    nxt = cov
    while True:
        if v.get('noname') and nxt not in items and pending and nxt not in spanat:
            # the temp's start unknown: the oldest temp left is taken to
            # begin where the chain stopped
            s0, c0 = pending[0]
            del spanat[s0]
            spanat[nxt] = c0
        if nxt in spanat:
            pending = [p for p in pending if p[1] is not spanat[nxt]]
        if nxt in items:
            content.add(items[nxt][1])
            nxt += 1
        elif nxt in spanat:
            content |= set(spanat[nxt])
            nxt += len(spanat[nxt])
        else:
            more = any(s > nxt for s in items) or any(s > nxt for s in spanat)
            if more:
                if v['end']:
                    return ('OK', frozenset(content), nxt)
                return ('REFUSE', 'gap')
            return ('OK', frozenset(content), nxt)


def recover(st, v):
    """Process crash (every write kept) and the recovering open; the history
    goes on from the opened state. None when the open refuses (counted as a
    P1 failure separately by the end-state check)."""
    (n, prom, wal, ws, wk, wseq, mem, runs, ropen, queue, segs, slots,
     retire, nid, crs, mgs) = st
    slot = slots[-1]
    temps = [(r[0], r[1], r[2]) for r in runs]
    res = open_store(slot, wal, temps, v)
    if res[0] != 'OK':
        return None
    pub, cov, sg = slot
    named_ids = {x[0] for x in sg}
    if not v['m2'] or not v['keep']:
        # rebuild every temp not named by id (M2rebuild: not covered) into a
        # piece named at covered; the temp goes
        newsegs = list(sg)
        for r in runs:
            if v['m2']:
                live = r[1] + len(r[2]) > cov and r[2]
            else:
                live = r[0] not in named_ids and r[2]
            if live:
                newsegs.append((r[0], cov, frozenset(r[2])))
        newsegs = tuple(sorted(newsegs))
        slots2 = slots
        if len(newsegs) != len(sg):
            slots2 = slots + ((pub + 1, cov, newsegs),)
        nxt = res[2]
        wal2 = tuple(r for r in wal if r[0] < nxt)
        mem2 = tuple(c for (s, c) in wal2 if s >= cov)
        ws2 = len(wal2) if v['C'] else min(ws, len(wal2))
        return (n, prom, wal2, ws2, len(wal2), nxt, mem2, (), -1, (), newsegs,
                slots2, frozenset(), nid, crs + 1, mgs)
    # M2 keep: temps not covered are kept, their records replayed in order
    kept = []
    for r in runs:
        if r[1] + len(r[2]) > cov and r[2]:
            syn = len(r[2]) if v['C'] else r[3]
            kept.append((r[0], r[1], r[2], syn, 'fin', True))
    content = res[1]
    segc = set()
    for x in sg:
        segc |= x[2]
    mem2 = tuple(sorted(content - segc))
    nxt = res[2]
    wal2 = tuple(r for r in wal if r[0] < nxt)
    kept = [k for k in kept if k[1] < nxt]
    ws2 = len(wal2) if v['C'] else min(ws, len(wal2))
    return (n, prom, wal2, ws2, len(wal2), nxt, mem2, tuple(kept), -1, (), sg,
            slots, frozenset(), nid, crs + 1, mgs)


def landings(st, v):
    """Every end state: process crash, then power-loss landings."""
    (n, prom, wal, ws, wk, wseq, mem, runs, ropen, queue, segs, slots,
     retire, nid, crs, mgs) = st
    yield ('proc', wal, [(r[0], r[1], r[2]) for r in runs])
    walopts = [wal[:L] for L in range(ws, len(wal) + 1)]
    runopts = []
    for r in runs:
        opts = [r[2][:c] for c in range(r[3], len(r[2]) + 1)]
        if r[3] == 0:
            opts.append(None)
        runopts.append(opts)
    for w in walopts:
        for combo in itertools.product(*runopts):
            temps = [(r[0], r[1], c) for r, c in zip(runs, combo) if c is not None]
            yield ('power', w, temps)


def damages(slots, wal, temps):
    yield ('none', slots[-1], wal, temps)
    for L in range(0, len(wal)):
        yield ('walcut', slots[-1], wal[:L], temps)
    for i, (rid, s_open, cids) in enumerate(temps):
        for c in range(0, len(cids)):
            t2 = list(temps)
            t2[i] = (rid, s_open, cids[:c])
            yield ('runcut', slots[-1], wal, t2)
        t2 = temps[:i] + temps[i + 1:]
        yield ('rundel', slots[-1], wal, t2)
    for sl in slots[:-1]:
        yield ('oldslot', sl, wal, temps)


def classify(res, n, prom):
    if res[0] == 'REFUSE':
        return 'REFUSE'
    c = res[1]
    m = len(c)
    if c != frozenset(range(1, m + 1)):
        return 'NEVER'
    if m == n:
        return 'WHOLE'
    if m < prom:
        return 'LOSTDUR'
    return 'ROLLBACK'


def run(variant, policy, depth, maxn=5, maxruns=3, maxcr=1, maxmg=1, examples=3):
    v = VARIANTS[variant]
    seen = set()
    stats = Counter()
    ex = {}
    stack = [(init(), ())]
    while stack:
        st, hist = stack.pop()
        if st in seen:
            continue
        seen.add(st)
        n, prom = st[0], st[1]
        slots = st[11]
        for (kind, wal, temps) in landings(st, v):
            for (dk, slot, w, t) in damages(slots, wal, temps):
                res = open_store(slot, w, t, v)
                cl = classify(res, n, prom)
                if dk == 'none':
                    key = ('P1', kind, cl)
                    bad = cl in ('NEVER', 'REFUSE', 'LOSTDUR') or (kind == 'proc' and cl != 'WHOLE')
                else:
                    key = ('P2', dk, cl)
                    bad = cl == 'NEVER'
                stats[key] += 1
                if bad and key not in ex:
                    ex[key] = (' '.join(hist), kind, dk,
                               'slot', slot[:2], 'wal', w,
                               'temps', t, 'n', n, 'prom', prom, '->', res[:2])
        if len(hist) < depth:
            for (op, s2) in successors(st, v, policy, maxn, maxruns, maxcr, maxmg):
                stack.append((s2, hist + (op,)))
    return seen, stats, ex


def main():
    variant, policy, depth = sys.argv[1], sys.argv[2], int(sys.argv[3])
    maxn = int(sys.argv[4]) if len(sys.argv) > 4 else 5
    maxcr = int(sys.argv[5]) if len(sys.argv) > 5 else 1
    maxmg = int(sys.argv[6]) if len(sys.argv) > 6 else 1
    maxruns = int(sys.argv[7]) if len(sys.argv) > 7 else 3
    seen, stats, ex = run(variant, policy, depth, maxn=maxn, maxcr=maxcr,
                          maxmg=maxmg, maxruns=maxruns)
    print(f'== {variant} {policy} depth {depth} maxn {maxn} cr {maxcr} mg {maxmg} runs {maxruns}: {len(seen)} states')
    p1bad = sum(c for k, c in stats.items() if k[0] == 'P1' and
                (k[2] in ('NEVER', 'REFUSE', 'LOSTDUR') or (k[1] == 'proc' and k[2] != 'WHOLE')))
    p2never = sum(c for k, c in stats.items() if k[0] == 'P2' and k[2] == 'NEVER')
    print(f'   P1 failures (no damage: never/refuse/lostdur, or proc not whole): {p1bad}')
    print(f'   P2 NEVER (one damage): {p2never}')
    for k in sorted(stats):
        print('   ', k, stats[k])
    for k in sorted(ex):
        print('  EX', k, ex[k])


if __name__ == '__main__':
    main()
