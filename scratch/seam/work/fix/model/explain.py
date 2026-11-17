#!/usr/bin/env python3
"""python3 explain.py adj|jrp OPS [opt=val ...]: run one sequence, print each call's case and
writes, then the first violating power-loss view (if any) and what recovery returned."""
import sys, ast
import cand
from cand import World, Orc, Fail, judge
def main():
    which, ops = sys.argv[1], ast.literal_eval(sys.argv[2])
    o = {}
    for a in sys.argv[3:]:
        k, v = a.split('=', 1)
        try: v = int(v)
        except ValueError: pass
        o[k] = v
    mod = __import__(which)
    H = mod.ADJ if which == 'adj' else mod.JRP
    W = World(); orc = Orc(); h = H(W, o, orc)
    for op in ops:
        before = dict(W.events); t0 = W.dev.t
        if isinstance(op, int):
            h.write(op)
        elif op == 'S': h.sync()
        elif op == 'B': h.sync_begin()
        elif op == 'E': h.sync_end_oldest()
        elif isinstance(op, tuple):
            try: h.write(op[1], fail=op[2])
            except Fail: pass
        ev = {k: v - before.get(k, 0) for k, v in W.events.items() if v - before.get(k, 0)}
        wr = [(loc, x) for loc, lst in W.dev.pend.items() for t, x in lst if t > t0]
        print(op, ev, 'writes:', sorted(set(l for l, _ in wr), key=repr), 'L=%d' % len(h.content))
    legal = orc.legal()
    print('acked', orc.hist[orc.ack], 'legal count', len(legal))
    for v in W.dev.any_views():
        res = mod.recover(W, v, h.o, h)
        vd = judge(res, legal, orc.hist, orc.ack)
        if vd:
            print('VIOLATION', vd)
            print(' view', {k: (W.imgs[x]['kind'] if isinstance(x, int) else x) for k, x in sorted(v.items(), key=repr)})
            print(' recovered', res)
            return
    print('no violation over', W.dev.n_any(), 'views')
main()
