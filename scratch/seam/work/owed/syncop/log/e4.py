#!/usr/bin/env python3
"""E4 families.  python3 e4.py FAMILY [option=value ...] [maxlen=N]
Each family enumerates op sequences over its alphabet and checks every crash
state of every sequence (see logrun.py)."""
import sys, time
import pfm
from logrun import Runner, seqs


def fam_base(o, maxlen):
    # the existing model's ground, with syncs in flight
    alpha = [1, 3, 5, 'S', 'B', 'E']
    for s in seqs(alpha, maxlen):
        yield s


def fam_inflight(o, maxlen):
    # a sync in flight across writes long enough to lap the ring (inflight.py's
    # scenario, and its neighbours)
    for pre in ([1, 1, 1], [5], [3, 'S', 3]):
        for mid in ([1, 1], [1], [5]):
            for during in range(1, maxlen + 1):
                yield tuple(pre) + ('S',) + tuple(mid) + ('B',) + (1,) * during + ('E',)


def fam_resume(o, maxlen):
    # session bases: restart and continue, a resume after a process crash and
    # after a power loss, repeated
    alpha = [1, 5, 'S', 'X', 'P']
    for s in seqs(alpha, maxlen):
        if s.count('P') > 1:
            continue
        if not any(x in ('X', 'P') for x in s):
            continue
        yield s


def fam_header(o, maxlen):
    # the header lagging: resumes that write a header with nothing synced between
    alpha = [1, 5, 'S', 'X']
    for s in seqs(alpha, maxlen):
        if s.count('X') < 2:
            continue
        yield s


def fam_retry(o, maxlen):
    alpha = [1, 3, 'S', ('f', 1, 'frame'), ('f', 3, 'data'), ('f', 3, 'tag'), ('f', 1, 'data')]
    for s in seqs(alpha, maxlen):
        if not any(isinstance(x, tuple) for x in s):
            continue
        yield s


def fam_trunc(o, maxlen):
    alpha = [1, 5, 'S']
    cuts = [('T', 1), ('T', 3), ('T', 5)]
    pres = [p for p in seqs(alpha, 3) if 'S' in p and any(isinstance(x, int) for x in p)]
    posts = list(seqs([1, 5, 'S'], maxlen))
    for post in posts:
        for pre in pres:
            for x in ('X', 'P'):
                for c in cuts:
                    yield pre + (x, c, 'S') + post


def fam_finish(o, maxlen):
    # a run: commits (synced under the durable policy, or not), then the close:
    # the records' presync in flight beside the finish's trailer and its head
    # patches, the presync joined, the layer's finish, the final sync_all
    for pol in ('durable', 'buffered'):
        for body in seqs([1, 3, 5], o.get('body', 3)):
            pre = []
            for w in body:
                pre.append(w)
                if pol == 'durable':
                    pre.append('S')
            for trailer in ((), (5,), (1,)):
                for patches in ((), (('O', 'P0'),), (('O', 'P0'), ('O', 'P1'))):
                    for presync in (True, False):
                        ops = list(pre)
                        if presync:
                            ops.append('B')
                        ops += list(trailer)
                        ops += list(patches)
                        if presync:
                            ops.append('E')
                        ops += ['Z', 'S']
                        yield tuple(ops)


def fam_mix(o, maxlen):
    # failed writes, process crashes and power losses together
    alpha = [1, 5, 'S', ('f', 1, 'frame'), ('f', 3, 'data'), 'X', 'P', 'B', 'E']
    for s in seqs(alpha, maxlen):
        if s.count('P') > 1:
            continue
        yield s


FAMS = dict(mix=fam_mix, base=fam_base, inflight=fam_inflight, resume=fam_resume, header=fam_header,
            retry=fam_retry, trunc=fam_trunc, finish=fam_finish)


def parse(argv):
    o = {}
    maxlen = 3
    cap = 400
    for a in argv:
        k, v = a.split('=')
        if k == 'maxlen':
            maxlen = int(v)
            continue
        if k == 'cap':
            cap = int(v)
            continue
        if v in ('True', 'False'):
            v = v == 'True'
        elif v.isdigit():
            v = int(v)
        o[k] = v
    return o, maxlen, cap


if __name__ == '__main__':
    fam = sys.argv[1]
    for a in list(sys.argv[2:]):
        if a.startswith('G='):
            pfm.G = int(a[2:])
            import logrun as _l
            _l.pfm.G = pfm.G
            sys.argv.remove(a)
    o, maxlen, cap = parse(sys.argv[2:])
    name = '%s %s maxlen=%d' % (fam, o, maxlen)
    r = Runner(o, name, branch_cap=cap)
    limit = float(o.pop('secs', 540)) if 'secs' in o else 540
    t0 = time.time()
    for ops in FAMS[fam](r.o, maxlen):
        # patches named by page: resolve against the run at run time
        r.run(tuple(ops))
        if time.time() - t0 > limit:
            print('time limit reached')
            break
    if r.sampled:
        print('power-loss branches sampled at', cap, 'in', r.sampled, 'points')
    nb = r.T.report()
    sys.exit(1 if nb else 0)
