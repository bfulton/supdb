#!/usr/bin/env python3
"""Skeptic check: syncs in flight completing out of issue order.

The fix model's runner ends only the OLDEST sync in flight ('E' ->
sync_end_oldest).  Two fdatasyncs of one file from two threads (the writer's
commit sync and the segment writer's helper, db.rs:5750-5752 -> landed() ->
WriterHelper, db.rs:6214-6216; or the close's presync, db.rs:29020-29035)
can complete in either order.  This adds 'N' = end the NEWEST sync in flight,
and runs JRP with its sync rule as design-free/design.md sec. 5.4 states it
("At completion, synced_seq := issued_seq") beside the model's own rule
(max), and ADJ as written (A_sync := r if newer).

usage: python3 -I ooo.py <model dir> jrp|adj assign|max FAMILY [k=v ...]
"""
import sys, itertools
sys.path.insert(0, sys.argv[1])
import cand
from cand import NoResumeRunner, well_formed

cname, rule, fam = sys.argv[2], sys.argv[3], sys.argv[4]
o = {}
for a in sys.argv[5:]:
    k, v = a.split('=', 1)
    try:
        v = int(v)
    except ValueError:
        pass
    o[k] = v
maxlen = o.pop('maxlen', 5)
cap = o.pop('cap', None)

if cname == 'jrp':
    import jrp as mod
    H = mod.JRP
    if rule == 'assign':
        def sync_end(s, tok):
            s.W.dev.sync_end(tok)
            iss, idx = s.infl.pop(tok)
            if iss is not None:
                s.synced_seq = iss          # design-free sec. 5.4, literally
            s.orc.acked(idx)
        H.sync_end = sync_end
else:
    import adj as mod
    H = mod.ADJ


def end_newest(h):
    if h.infl:
        h.sync_end(max(h.infl))


class R2(NoResumeRunner):
    def run(s, ops):
        # 'N' handled by rewriting it to a call on the handle; everything else as before
        W = cand.World()
        orc = cand.Orc()
        h = s.Hcls(W, s.o, orc)
        try:
            for i, op in enumerate(ops):
                last = i == len(ops) - 1
                if isinstance(op, int):
                    h.write(op, mid=(lambda view, new: s.mid(W, h, orc, ops, view, new)) if last else None)
                elif op == 'S':
                    h.sync()
                elif op == 'B':
                    h.sync_begin()
                elif op == 'E':
                    h.sync_end_oldest()
                elif op == 'N':
                    end_newest(h)
                else:
                    raise ValueError(op)
        except cand.Blocked:
            s.T.blocked += 1
            return
        s.T.runs += 1
        s.T.events.update(W.events)
        v = W.dev.pc()
        res = s.recover(W, v, s.o, h)
        s.T.add('proc', cand.judge(res, {orc.hist[-1]}, orc.hist, len(orc.hist) - 1),
                lambda: {'ops': ops, 'view': cand.short(v), 'rec': res[0]})
        n = W.dev.n_any()
        if cap is not None and n > cap:
            s.T.sampled += 1
        legal = orc.legal()
        for v in W.dev.any_views(cap=cap):
            res = s.recover(W, v, s.o, h)
            s.T.add('any', cand.judge(res, legal, orc.hist, orc.ack),
                    lambda: {'ops': ops, 'view': cand.short(v), 'rec': res[0],
                             'legal': sorted(legal, key=len)})


def wf(seq):
    k = 0
    for x in seq:
        if x == 'B':
            k += 1
            if k > 2:
                return False
        elif x in ('E', 'N'):
            if k == 0:
                return False
            k -= 1
    return True


def fam_ooo(sizes):
    """[w S]*p, B, w, B, k writes, N (the newer ends first), j writes, E, then 0-3 writes"""
    for p in (0, 1):
        for k in (0, 1):
            for j in (0, 1, 2):
                for post in (0, 1, 2, 3):
                    for ws in itertools.product(sizes, repeat=p + 1 + k + j + post):
                        out = []
                        for w in ws[:p]:
                            out += [w, 'S']
                        i = p
                        out += ['B', ws[i], 'B']
                        i += 1
                        out += list(ws[i:i + k]); i += k
                        out.append('N')
                        out += list(ws[i:i + j]); i += j
                        out.append('E')
                        out += list(ws[i:])
                        yield tuple(out)


name = '%s %s %s %s' % (cname, rule, fam, ' '.join('%s=%s' % kv for kv in sorted(o.items())))
R = R2(H, mod.recover, o, name, cap=cap)
if fam == 'one':
    R.run(tuple(eval(o_) for o_ in []) or eval(sys.stdin.read()))
elif fam == 'ooo':
    sizes = [1, 3]
    for s_ in fam_ooo(sizes):
        R.run(s_)
elif fam == 'any':
    alpha = ([1, 2, 3, 5] if cname == 'jrp' else [1, 2, 3, 5]) + ['S', 'B', 'E', 'N']
    for n in range(1, maxlen + 1):
        for s_ in itertools.product(alpha, repeat=n):
            if wf(s_) and 'N' in s_:
                R.run(s_)
R.T.report()
