#!/usr/bin/env python3
"""SYNCOP families for the Log class (E4's pfm.py/logrun.py, copied and
extended): syncs in flight together, completed in any order, or failed.

  python3 syncop.py FAMILY [option=value ...] [maxlen=N] [secs=S]

ops (logrun.py): n write; 'S' the caller's sync (under `wait` it waits for
every sync in the sequencer, then issues its own; without it, it is issued
and completes at once, beside any in flight); 'B' a sync issued by another
thread (under `wait`: queued while one is in flight, issued when it ends);
'E'/'N' the oldest/newest in flight completes; 'F'/'G' the oldest/newest in
flight fails.

families:
  ooo     every sequence over [1, 5, S, B, E, N] up to maxlen with at least two
          syncs outstanding at once (B B, or B ... S), at most `inf` outstanding;
          N only where two or more are outstanding (else it is E)
  fail    as ooo with F and G, at least one failure
  tail    a fixed shape that lets a regressed `synced` reach the ring:
          pre, B, w*, (S | B w* N), w*, E, then `post` writes
options beyond pfm.DEFAULT: inf (max outstanding, default 2)
"""
import sys, time, itertools
import pfm
from logrun import Runner, seqs


def outstanding_ok(s, inf):
    k = 0
    for x in s:
        if x == 'B':
            k += 1
            if k > inf:
                return False
        elif x in ('E', 'F'):
            if k == 0:
                return False
            k -= 1
        elif x in ('N', 'G', 'Y', 'V'):
            if k < 2:            # with one outstanding, newest == oldest
                return False
            k -= 1
    return True


def overlaps(s):
    k = 0
    for x in s:
        if x == 'B':
            if k >= 1:
                return True
            k += 1
        elif x in ('E', 'F', 'N', 'G', 'Y', 'V'):
            k -= 1
        elif x == 'S' and k >= 1:
            return True
    return False


def fam_ooo(o, maxlen, inf):
    alpha = [1, 5, 'S', 'B', 'E', 'N']
    for s in seqs(alpha, maxlen):
        if outstanding_ok(s, inf) and overlaps(s):
            yield s


def fam_fail(o, maxlen, inf):
    alpha = [1, 5, 'S', 'B', 'E', 'N', 'F', 'G']
    for s in seqs(alpha, maxlen):
        if not any(x in ('F', 'G') for x in s):
            continue
        if outstanding_ok(s, inf):
            yield s


def fam_tail(o, maxlen, inf):
    for pre in ((), (1, 'S'), (5, 'S')):
        for d1 in range(0, 3):
            for mid in ('S', 'N'):
                for d2 in range(0, 3):
                    for d3 in range(0, 3):
                        for post in range(0, maxlen + 1):
                            ops = list(pre) + ['B'] + [1] * d1
                            if mid == 'S':
                                ops += ['S']
                            else:
                                ops += ['B'] + [1] * d2 + ['N']
                            ops += [1] * d3 + ['E'] + [1] * post
                            yield tuple(ops)


def fam_steal(o, maxlen, inf):
    alpha = [1, 5, 'S', 'B', 'E', 'N', 'Y', 'V']
    for s in seqs(alpha, maxlen):
        if not any(x in ('Y', 'V') for x in s):
            continue
        if outstanding_ok(s, inf):
            yield s


def fam_hand(o, maxlen, inf):
    for t in o.pop('_hand', '').split(';'):
        if t:
            yield tuple(int(x) if x.isdigit() else x for x in t.split('_'))


FAMS = dict(steal=fam_steal, hand=fam_hand, ooo=fam_ooo, fail=fam_fail, tail=fam_tail)


def parse(argv):
    o = {}
    maxlen, cap, inf, secs = 5, 400, 2, 540
    for a in argv:
        k, v = a.split('=')
        if k == 'maxlen':
            maxlen = int(v); continue
        if k == 'cap':
            cap = int(v); continue
        if k == 'inf':
            inf = int(v); continue
        if k == 'secs':
            secs = float(v); continue
        if v in ('True', 'False'):
            v = v == 'True'
        elif v.isdigit() and k != '_hand':
            v = int(v)
        o[k] = v
    return o, maxlen, cap, inf, secs


if __name__ == '__main__':
    fam = sys.argv[1]
    o, maxlen, cap, inf, secs = parse(sys.argv[2:])
    name = 'syncop %s %s maxlen=%d inf=%d' % (fam, o, maxlen, inf)
    r = Runner(o, name, branch_cap=cap)
    t0 = time.time()
    n = 0
    stopped = False
    for ops in FAMS[fam](r.o, maxlen, inf):
        r.run(tuple(ops))
        n += 1
        if time.time() - t0 > secs:
            stopped = True
            break
    print('sequences', n, 'time %.1fs' % (time.time() - t0), 'STOPPED AT TIME LIMIT' if stopped else 'complete')
    nb = r.T.report()
    sys.exit(1 if nb else 0)
