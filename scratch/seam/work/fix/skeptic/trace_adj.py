#!/usr/bin/env python3
"""Run one ADJ op sequence through the fix model's Handle and print each
call's case and the ring record asked for; no crash views are enumerated.
usage: python3 -I trace_adj.py <model dir> "OPS" [k=v ...]"""
import sys, ast
sys.path.insert(0, sys.argv[1])
import cand, adj

ops = ast.literal_eval(sys.argv[2])
o = {}
for a in sys.argv[3:]:
    k, v = a.split('=', 1)
    o[k] = int(v)
asked = []
orig = adj.ADJ.ring_alloc


def ring_alloc(s, k, P):
    r = orig(s, k, P)
    asked.append((k, sorted(l[1] for l in P if l[0] == 'r'), r and [l[1] for l in r]))
    return r


adj.ADJ.ring_alloc = ring_alloc
W = cand.World()
h = adj.ADJ(W, o, cand.Orc())
for op in ops:
    before = dict(W.events)
    asked.clear()
    try:
        if isinstance(op, int):
            h.write(op)
        elif op == 'S':
            h.sync()
        elif op == 'B':
            h.sync_begin()
        elif op == 'E':
            h.sync_end_oldest()
    except cand.Blocked:
        print(op, 'BLOCKED: ring asked', asked)
        break
    ev = {k: v - before.get(k, 0) for k, v in W.events.items() if v - before.get(k, 0)}
    print(op, ev, 'Q=%s' % h.Q, 'ring:', asked,
          'A_pc', h.A_pc and h.A_pc['locs'][:1], 'A_sync', h.A_sync and h.A_sync['locs'][:1])
