#!/usr/bin/env python3
"""Drives the Log class through op sequences and checks every crash state.

ops:  n          a write call of n bytes
      'S'        a sync (fdatasync) begun and completed
      'B' / 'E'  a sync begun / the oldest in flight completed (another thread)
      'X'        a process crash, then the file reopened and resumed
      'P'        a power loss: every 'any' state is checked, and the run goes on
                 from each (sampled past `branch_cap`) after a reopen and resume
      ('f', n, k) a write call of n bytes whose syscall k ('data','tag','frame')
                 fails, its objects torn in the page cache; the engine resends
                 the bytes with its next write call
      ('T', k)   set_len(len - k), as replay's cut
      ('O', k)   write_all_at one byte at len - k (a patch below what was written)
      'Z'        finish (Log -> Once)
checks: every state recovers without refusal; its content is one the file
held at or after the last acknowledged sync ('any'), or exactly the content
after the last completed write call ('proc'); every page below the recovered
length verifies (UNREADABLE otherwise: a poison); no LATENT tag page; no
(key, nonce) sealed twice.
"""
import sys, itertools
import pfm
from pfm import LogH, Orc, World, Tally, Blocked, Fail, Refused, PB


def verdict_of(W, v, ino, fid, o, legal, hist, ack):
    try:
        st = pfm.recover_log(W, v, ino, fid, o)
    except Refused as e:
        return ('REFUSED', str(e)), None
    c = tuple(st['content']) if not st['empty'] else ()
    if not st['empty'] and st['unreadable']:
        return ('UNREADABLE', st['unreadable'], 'trusted' if st.get('trusted') else 'log'), st
    if c not in legal:
        if c in hist[:ack]:
            return ('LOSS', len(c), 'acked', len(hist[ack])), st
        return ('WRONG', c), st
    if not st['empty'] and st['latent']:
        return ('LATENT', st['latent']), st
    return None, st


def short(v, ino):
    return {k[1]: (x if x == pfm.TORN else x) for k, x in sorted(v.items(), key=repr) if k[0] == ino}


class Runner:
    def __init__(s, o, name, branch_cap=400, ino=1):
        s.o = dict(pfm.DEFAULT, **o)
        s.T = Tally(name)
        s.cap = branch_cap
        s.ino = ino
        s.sampled = 0

    def any_check(s, W, orc, fid, ops, trace):
        legal = orc.legal()
        out = []
        for v, d in W.dev.any_views():
            vd, st = verdict_of(W, v, s.ino, fid, s.o, legal, orc.hist, orc.ack)
            s.T.add('any', vd, {'ops': ops, 'trace': list(trace), 'view': short(v, s.ino),
                                'rec': (st or {}).get('content')})
            out.append((v, d, vd, st))
        return out

    def proc_check(s, W, orc, fid, ops, trace, mid=None):
        v, d = W.dev.pc()
        legal = {orc.hist[-1]} | ({mid} if mid is not None else set())
        vd, st = verdict_of(W, v, s.ino, fid, s.o, legal, orc.hist, len(orc.hist) - 1)
        s.T.add('proc', vd, {'ops': ops, 'trace': list(trace), 'view': short(v, s.ino),
                             'rec': (st or {}).get('content')})
        return vd, st

    def nonce_check(s, W, ops):
        for b in W.bad:
            s.T.add('seal', (b[0], b[1]), {'ops': ops})
            return

    def run(s, ops):
        W = World()
        fid = W.uniq('fid')
        orc = Orc()
        trace = []
        try:
            h = LogH.create(W, s.ino, fid, s.o, orc)
            s.go(W, h, orc, fid, ops, 0, trace)
        except Blocked:
            s.T.blocked += 1
            return
        s.T.runs += 1

    def go(s, W, h, orc, fid, ops, i, trace):
        W.hooks = [lambda: s.any_check(W, orc, fid, ops, trace)]
        while i < len(ops):
            op = ops[i]
            i += 1
            trace.append(op)
            if isinstance(op, int):
                h.write(op, proc=lambda mid: s.proc_check(W, orc, fid, ops, trace, mid))
            elif op == 'S':
                h.sync()
            elif op == 'B':
                h.request_sync()
            elif op == 'E':
                h.complete('oldest')
            elif op == 'N':
                h.complete('newest')           # SYNCOP: the newest in flight completes first
            elif op == 'F':
                h.complete('oldest', fail=True)  # SYNCOP: the oldest in flight fails
            elif op == 'G':
                h.complete('newest', fail=True)  # SYNCOP: the newest in flight fails
            elif op == 'Z':
                h.finish()
            elif isinstance(op, tuple) and op[0] == 'f':
                kinds = ['data', 'tag', 'frame']
                try:
                    # the index among this call's syscalls: compute by kind
                    h.write(op[1], fail=s.fail_index(h, op[1], op[2]))
                except Fail:
                    pass
            elif isinstance(op, tuple) and op[0] == 'T':
                L = max(0, h.n - op[1])
                h.trunc(L)
            elif isinstance(op, tuple) and op[0] == 'O':
                pos = int(op[1][1:]) * PB if isinstance(op[1], str) else h.n - op[1]
                if 0 <= pos < h.n:
                    h.patch(pos)
            elif op in ('X', 'P'):
                if op == 'X':
                    vd, st = s.proc_check(W, orc, fid, ops, trace)
                    if vd and vd[0] not in ('LATENT',):
                        return
                    branches = [(W.dev.pc(), st)]
                else:
                    res = s.any_check(W, orc, fid, ops, trace)
                    ok = [(v, d, st) for v, d, vd, st in res if not vd or vd[0] == 'LATENT']
                    if len(ok) > s.cap:
                        step = len(ok) / s.cap
                        ok = [ok[int(k * step)] for k in range(s.cap)]
                        s.sampled += 1
                    branches = [((v, d), st) for v, d, st in ok]
                s.nonce_check(W, ops)
                for (v, d), st in branches:
                    W2 = W.clone() if op == 'X' else W.clone_after_loss(v, d)
                    orc2 = orc.copy()
                    orc2.cut_to(st['content'] if not st['empty'] else ())
                    try:
                        if st['empty']:
                            h2 = LogH.create(W2, s.ino, fid, s.o, orc2, hdr=st.get('hdr'))
                        else:
                            h2 = LogH.resume(W2, s.ino, fid, s.o, orc2, st)
                        s.go(W2, h2, orc2, fid, ops, i, list(trace))
                    except Blocked:
                        s.T.blocked += 1
                return
            s.proc_check(W, orc, fid, ops, trace)
        s.any_check(W, orc, fid, ops, trace)
        s.nonce_check(W, ops)

    def fail_index(s, h, n, kind):
        # which syscall of this call is `kind`: data pages first, a tag page, the frame
        total = len(h.failed) + n
        buf = len(h.tail) + total
        pages = buf // PB
        idx = 0
        calls = []
        if pages:
            calls.append('data')
            if any((h.home + j) % pfm.G == pfm.G - 1 for j in range(pages)):
                calls.append('tag')
        calls.append('frame')
        return calls.index(kind) if kind in calls else None


def seqs(alpha, maxlen, must=None, start=()):
    for n in range(1, maxlen + 1):
        for s in itertools.product(alpha, repeat=n):
            s = tuple(start) + s
            if must and not must(s):
                continue
            yield s


if __name__ == '__main__':
    pass
