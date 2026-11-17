#!/usr/bin/env python3
"""python3 run.py CAND FAMILY [key=value ...]
CAND: adj | jrp     (run temps: no resume, db.rs:27727-27770)
FAMILY: dur | any | fail | buf | infl
options: maxlen=N n=N (commits, dur) cap=N (any-views per sequence; sampled past it), and any
model option (e.g. sh=F, P_none=1, M=0 for ADJ's T)."""
import sys, itertools, time
import cand
from cand import NoResumeRunner, seqs, well_formed


def parse(argv):
    o = {}
    for a in argv:
        k, v = a.split('=', 1)
        try:
            v = int(v)
        except ValueError:
            pass
        o[k] = v
    return o


def fam_dur(n, sizes):
    """the durable policy: every call followed by its sync; the last call
    unsynced, or with its sync in flight"""
    for k in range(1, n + 1):
        for ws in itertools.product(sizes, repeat=k):
            base = []
            for w in ws[:-1]:
                base += [w, 'S']
            yield tuple(base + [ws[-1]])
            yield tuple(base + [ws[-1], 'B'])
            yield tuple(base + [ws[-1], 'S'])


def fam_any(maxlen, alpha):
    for s in seqs(alpha, maxlen):
        if well_formed(s):
            yield s


def fam_infl(n, sizes):
    """the helper's sync in flight beside synced commits: B before a commit,
    E after one or two more"""
    for k in range(1, n + 1):
        for ws in itertools.product(sizes, repeat=k):
            for b in range(k):
                for e in range(b + 1, min(k, b + 3) + 1):
                    out = []
                    for i, w in enumerate(ws):
                        if i == b:
                            out.append('B')
                        if i == e:
                            out.append('E')
                        out += [w, 'S'] if i % 2 == 0 else [w]
                    yield tuple(out)


def fam_infl2(sizes):
    """a sync in flight across unsynced calls: [w S]*p, B, k writes, E, then 0-1 more,
    and the same with a second sync begun inside the first"""
    for p in (0, 1, 2):
        for k in (1, 2, 3):
            for post in (0, 1):
                for ws in itertools.product(sizes, repeat=p + k + post):
                    out = []
                    for w in ws[:p]:
                        out += [w, 'S']
                    out.append('B')
                    out += list(ws[p:p + k])
                    out.append('E')
                    out += list(ws[p + k:])
                    yield tuple(out)
                    if k >= 2:
                        o2 = []
                        for w in ws[:p]:
                            o2 += [w, 'S']
                        o2 += ['B', ws[p], 'B'] + list(ws[p + 1:p + k]) + ['E'] + list(ws[p + k:]) + ['E']
                        yield tuple(o2)


def main():
    cname, fam = sys.argv[1], sys.argv[2]
    o = parse(sys.argv[3:])
    maxlen = o.pop('maxlen', 5)
    n = o.pop('n', 5)
    cap = o.pop('cap', None)
    sizes = o.pop('sizes', None)
    if cname == 'adj':
        import adj as mod
        H = mod.ADJ
        sizes = [int(x) for x in str(sizes).split(',')] if sizes else [1, 2, 3, 5]
        alpha_any = sizes + [4, 'S', 'B', 'E']
        alpha_fail = [1, 3, 'S', ('f', 1, 0), ('f', 3, 0), ('f', 3, -1), ('f', 5, 0)]
        alpha_buf = [4, 1, 3, 'S', 'B', 'E']
    elif cname == 'jrp':
        import jrp as mod
        H = mod.JRP
        sizes = [int(x) for x in str(sizes).split(',')] if sizes else [1, 2, 3, 5]
        alpha_any = sizes + ['S', 'B', 'E']
        alpha_fail = [1, 3, 5, 'S', ('f', 1, 0), ('f', 3, 0), ('f', 5, 0)]
        alpha_buf = [4, 1, 3, 'S', 'B', 'E']
    else:
        raise SystemExit('cand?')
    name = '%s %s %s' % (cname, fam, ' '.join('%s=%s' % kv for kv in sorted(o.items())))
    R = NoResumeRunner(H, mod.recover, o, name + ' maxlen=%d n=%d' % (maxlen, n), cap=cap)
    if fam == 'dur':
        gen = fam_dur(n, sizes)
    elif fam == 'any':
        gen = fam_any(maxlen, alpha_any)
    elif fam == 'fail':
        gen = (s for s in fam_any(maxlen, alpha_fail) if any(isinstance(x, tuple) for x in s))
    elif fam == 'buf':
        gen = fam_any(maxlen, alpha_buf)
    elif fam == 'infl':
        gen = fam_infl(n, sizes)
    elif fam == 'infl2':
        gen = fam_infl2(sizes)
    else:
        raise SystemExit('family?')
    import signal
    def stop(sig, frm):
        print('## TIMEOUT: partial counts follow', flush=True)
        R.T.report()
        sys.exit(3)
    signal.signal(signal.SIGTERM, stop)
    for s in gen:
        R.run(s)
    nb = R.T.report()
    sys.exit(0)


if __name__ == '__main__':
    main()
