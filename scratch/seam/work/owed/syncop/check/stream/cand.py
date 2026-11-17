#!/usr/bin/env python3
"""Common ground for the fix candidates (ADJ, JRP, Stream/J): device, world,
oracle, tally and the no-resume driver.  Written beside the E4 model (pfm.py),
which it follows, with two changes to the device:

  * a location is a PAGE (several sectors written whole by a call; a mix of
    two different images authenticates as nothing, so it is collapsed to
    TORN -- added only where no option already reads as nothing, and never
    for a rewrite with the identical image, which E4's Dev did add) or a
    SECTOR (a key whose first element is in ATOMIC: written whole or not at
    all; its value is a tuple of units, so byte-packed records that share a
    sector are torn at every sector boundary, as the brief asks);
  * a process crash may also cut a syscall: the page cache then holds a
    prefix of its objects (pages or sectors, in file order).

Power loss: every location dirty since the last barrier that covers it holds
its durable value or any value written to it since ('any' subset and order);
fdatasync begun at tb and completed: a location dirty at tb holds its tb value
or a later one.  States only grow between barriers, so checking at the end of
every op sequence, with every prefix enumerated, checks every instant.
"""
import itertools, sys, time, collections

TORN = 'TORN'
ATOMIC = {'rs', 'ss'}          # ring sectors (JRP), stream sectors (Stream/J)


class Blocked(Exception):
    pass


class Fail(Exception):
    pass


class Dev:
    def __init__(s):
        s.t = 0
        s.dur, s.pend = {}, {}
        s.infl, s.ntok = {}, 0
        s.limbo = {}      # SYNCOP: values whose writeback failed: no later sync makes them durable

    def clone(s):
        d = Dev.__new__(Dev)
        d.t, d.ntok = s.t, s.ntok
        d.dur = dict(s.dur)
        d.pend = {k: list(v) for k, v in s.pend.items()}
        d.infl = dict(s.infl)
        d.limbo = {k: list(v) for k, v in s.limbo.items()}
        return d

    def sync_fail(s, tok):
        """SYNCOP: the fdatasync begun at tb fails: a location dirty at tb holds
        its durable value or any value written up to tb (or later), and no later
        sync writes those values again unless they are written again (Linux marks
        the pages clean after a failed writeback)."""
        tb = s.infl.pop(tok)
        for k in list(s.pend):
            lst = s.pend[k]
            bef = [v for t, v in lst if t <= tb]
            if bef:
                s.limbo.setdefault(k, []).extend(bef)
                rest = [(t, v) for t, v in lst if t > tb]
                if rest:
                    s.pend[k] = rest
                else:
                    del s.pend[k]

    def write(s, objs):
        s.t += 1
        for loc, v in objs:
            s.pend.setdefault(loc, []).append((s.t, v))

    def sync_begin(s):
        s.t += 1
        s.ntok += 1
        s.infl[s.ntok] = s.t
        return s.ntok

    def sync_end(s, tok):
        tb = s.infl.pop(tok)
        for k in list(s.pend):
            lst = s.pend[k]
            bef = [v for t, v in lst if t <= tb]
            if bef:
                s.dur[k] = bef[-1]
                s.limbo.pop(k, None)
                rest = [(t, v) for t, v in lst if t > tb]
                if rest:
                    s.pend[k] = rest
                else:
                    del s.pend[k]

    def cur(s, loc):
        if loc in s.pend:
            return s.pend[loc][-1][1]
        if loc in s.limbo:
            return s.limbo[loc][-1]
        return s.dur.get(loc)

    def pc(s, extra=()):
        v = dict(s.dur)
        for k, lst in s.limbo.items():
            v[k] = lst[-1]
        for k, lst in s.pend.items():
            v[k] = lst[-1][1]
        for k, x in extra:
            v[k] = x
        return v

    def options(s, k):
        d0 = s.dur.get(k)
        vals = []
        for x in [d0] + list(s.limbo.get(k, ())) + [v for _, v in s.pend.get(k, ())]:
            if x not in vals:
                vals.append(x)
        if k[0] not in ATOMIC and len(vals) >= 2 and None not in vals:
            vals.append(TORN)
        return vals

    def n_any(s):
        n = 1
        for k in set(s.pend) | set(s.limbo):
            n *= len(s.options(k))
        return n

    def any_views(s, cap=None, rng=None):
        keys = list(dict.fromkeys(list(s.pend) + list(s.limbo)))
        ch = [s.options(k) for k in keys]
        tot = 1
        for c in ch:
            tot *= len(c)
        if cap is not None and tot > cap:
            # deterministic stride sample over the product (reported as sampled)
            step = tot / cap
            for j in range(cap):
                idx = int(j * step)
                v = dict(s.dur)
                for k, c in zip(keys, ch):
                    x = c[idx % len(c)]
                    idx //= len(c)
                    if x is None:
                        v.pop(k, None)
                    else:
                        v[k] = x
                yield v
            return
        for pick in itertools.product(*ch):
            v = dict(s.dur)
            for k, x in zip(keys, pick):
                if x is None:
                    v.pop(k, None)
                else:
                    v[k] = x
            yield v

    @staticmethod
    def from_view(t, v):
        n = Dev()
        n.t = t
        n.dur = {k: x for k, x in v.items() if x is not None}
        return n


class World:
    def __init__(s):
        s.dev = Dev()
        s.imgs = {}
        s.nonce = {}
        s.bad = []
        s.cnt = [0]
        s.events = collections.Counter()

    def clone_after_loss(s, v):
        w = World.__new__(World)
        w.dev = Dev.from_view(s.dev.t, v)
        w.imgs, w.cnt = s.imgs, s.cnt
        w.nonce, w.bad = dict(s.nonce), list(s.bad)
        w.events = collections.Counter(s.events)
        return w

    def clone(s):
        w = World.__new__(World)
        w.dev = s.dev.clone()
        w.imgs, w.cnt = s.imgs, s.cnt
        w.nonce, w.bad = dict(s.nonce), list(s.bad)
        w.events = collections.Counter(s.events)
        return w

    def uniq(s, tag):
        s.cnt[0] += 1
        return (tag, s.cnt[0])

    def seal(s, key, nonce, content, **meta):
        k = (key, nonce)
        old = s.nonce.get(k)
        if old is not None:
            s.bad.append(('NONCE_REUSE' if old != content else 'NONCE_REPEAT', k))
        else:
            s.nonce[k] = content
        s.cnt[0] += 1
        i = s.cnt[0]
        meta.update(key=key, nonce=nonce, content=content)
        s.imgs[i] = meta
        return i


class Orc:
    def __init__(s):
        s.hist = [()]
        s.ack = 0

    def copy(s):
        o = Orc()
        o.hist, o.ack = list(s.hist), s.ack
        return o

    def done(s, content):
        s.hist.append(tuple(content))

    def at(s):
        return len(s.hist) - 1

    def acked(s, i):
        s.ack = max(s.ack, i)

    def legal(s):
        return set(s.hist[s.ack:])

    def cut_to(s, content):
        c = tuple(content)
        for i in range(len(s.hist) - 1, s.ack - 1, -1):
            if s.hist[i] == c:
                s.hist = s.hist[:i + 1]
                return
        raise AssertionError('cut_to an illegal state')


def judge(res, legal, hist, ack):
    """res = (content tuple, unreadable list)"""
    c, bad = res
    if bad:
        return ('UNREADABLE', tuple(bad))
    if c not in legal:
        if c in hist[:ack]:
            return ('LOSS', len(c), 'acked', len(hist[ack]))
        a = hist[ack]
        if c[:len(a)] == a and any(len(x) > len(c) and x[:len(c)] == c for x in legal):
            return ('PARTIAL', len(c))     # a byte prefix of a later call, past the acked state
        return ('WRONG', c)
    return None


class Tally:
    def __init__(s, name):
        s.name = name
        s.states = collections.Counter()
        s.bad = collections.Counter()
        s.first = {}
        s.runs = 0
        s.blocked = 0
        s.sampled = 0
        s.events = collections.Counter()
        s.t0 = time.time()

    def add(s, model, verdict, info):
        s.states[model] += 1
        if verdict:
            key = (model, verdict[0])
            s.bad[key] += 1
            if key not in s.first:
                s.first[key] = (verdict, info() if callable(info) else info)

    def report(s, out=sys.stdout):
        nb = sum(s.bad.values())
        print('==', s.name, 'runs', s.runs, 'blocked', s.blocked, 'sampled', s.sampled,
              'states', dict(s.states), 'violations', nb,
              'secs %.1f' % (time.time() - s.t0), file=out)
        if s.events:
            print('   events', dict(s.events), file=out)
        for k in sorted(s.bad):
            print('   ', k, s.bad[k], file=out)
        for k in sorted(s.first):
            verdict, info = s.first[k]
            print('  first', k, verdict, file=out)
            for f in ('ops', 'trace', 'view', 'rec', 'legal'):
                if info.get(f) is not None:
                    print('     %s' % f, info[f], file=out)
        out.flush()
        return nb


def short(v):
    return {k: x for k, x in sorted(v.items(), key=repr)}


def seqs(alpha, maxlen, minlen=1):
    for n in range(minlen, maxlen + 1):
        for s in itertools.product(alpha, repeat=n):
            yield s


def well_formed(s, maxinfl=2):
    """'E' only with a sync in flight; at most maxinfl in flight"""
    k = 0
    for x in s:
        if x == 'B':
            k += 1
            if k > maxinfl:
                return False
        elif x == 'E':
            if k == 0:
                return False
            k -= 1
    return True


class NoResumeRunner:
    """For a file that is never written again after a crash (the run temp,
    db.rs:27727-27770: read by recover_direct, rebuilt, removed).  Every op
    sequence is run once and judged at its end: the page cache after the
    last completed call ('proc'), a crash inside the last call's syscalls
    ('proc', mid), and every power-loss view ('any')."""

    def __init__(s, Hcls, recover, o, name, cap=None):
        s.Hcls, s.recover, s.o = Hcls, recover, o
        s.T = Tally(name)
        s.cap = cap

    def run(s, ops):
        W = World()
        orc = Orc()
        trace = []
        h = s.Hcls(W, s.o, orc)
        try:
            for i, op in enumerate(ops):
                last = i == len(ops) - 1
                trace.append(op)
                if isinstance(op, int):
                    h.write(op, mid=(lambda view, new: s.mid(W, h, orc, ops, view, new)) if last else None)
                elif op == 'S':
                    h.sync()
                elif op == 'B':
                    h.sync_begin()
                elif op == 'E':
                    h.sync_end_oldest()
                elif isinstance(op, tuple) and op[0] == 'f':
                    try:
                        h.write(op[1], fail=op[2])
                    except Fail:
                        pass
                else:
                    raise ValueError(op)
        except Blocked:
            s.T.blocked += 1
            s.T.events.update(W.events)
            return
        s.T.runs += 1
        s.T.events.update(W.events)
        # proc: the page cache
        v = W.dev.pc()
        res = s.recover(W, v, s.o, h)
        s.T.add('proc', judge(res, {orc.hist[-1]}, orc.hist, len(orc.hist) - 1),
                lambda: {'ops': ops, 'view': short(v), 'rec': res[0]})
        # any
        n = W.dev.n_any()
        cap = s.cap
        if cap is not None and n > cap:
            s.T.sampled += 1
        legal = orc.legal()
        for v in W.dev.any_views(cap=cap):
            res = s.recover(W, v, s.o, h)
            s.T.add('any', judge(res, legal, orc.hist, orc.ack),
                    lambda: {'ops': ops, 'view': short(v), 'rec': res[0], 'legal': sorted(legal, key=len)})
        for b in W.bad:
            s.T.add('seal', (b[0], b[1]), {'ops': ops})
            break

    def mid(s, W, h, orc, ops, view, new):
        """a process crash inside the last write call: `view` is the page
        cache at that instant; legal: the content before the call, or after it"""
        res = s.recover(W, view, s.o, h)
        s.T.add('proc', judge(res, {orc.hist[-1], tuple(new)}, orc.hist, len(orc.hist) - 1),
                lambda: {'ops': ops, 'view': short(view), 'rec': res[0]})


def syscalls(W, calls, mid, new_content):
    """Perform a write call's syscalls in order.  calls: list of object lists.
    mid(view, new) is called at every instant a process crash could cut the
    call: before each syscall and after each object of it (page-cache prefix)."""
    for objs in calls:
        if mid:
            for j in range(len(objs)):
                mid(W.dev.pc(objs[:j]), new_content)
        W.dev.write(objs)
    if mid:
        mid(W.dev.pc(), new_content)
