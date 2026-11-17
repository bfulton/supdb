#!/usr/bin/env python3
"""The manifest's two slots as sealed records behind the door (owed item
"slots" of decisions.md decision 2).

A slot is a file holding ONE sealed record at offset 0, rewritten whole in
place by each publish that chooses it.  The record:

    head (clear: magic, slot idx, seq, len, nonce) | ciphertext | tag

modelled as units (img, k), k < n, n = the record's length in units; a
device sector holds SU units and is written atomically (old or new as a
whole), so a rewrite tears at every sector boundary, and a process kill or a
write error inside the write call leaves a prefix of its units (a byte
boundary at the model's grain) in the page cache.  A slot reads as

    EMPTY  every unit is zero (made and never written, or never durable),
    VALID  units 0..n-1 are the record's own and its AAD checks,
    TORN   anything else (bytes present that do not authenticate).

Units past the record's length -- the tail of a longer older image -- are
not read.  The key is the store's slot key (one store in the model: an
image sealed for another store is not modelled here, see adversary.py);
the nonce is drawn by the layer at every seal: (prefix drawn per open,
count), count advanced at every seal attempt.  AAD = (domain, store,
slot idx, seq, len[, pred]).

The engine's protocol (src/db.rs: SlotChoice 12343-12376, Maint::write_slot
40839-40870, publish 40914-40960, publish_unsynced 40785-40812,
sync_owed_slot 40815-40830, the pending slot 39640-39710, Db::open
28740-28775):

  * a publish takes seq+1 and writes the slot that does NOT hold the newest
    manifest known durable (SlotChoice.next()), giving up every sync entry
    owed or in flight for that slot BEFORE its write begins;
  * "known durable" = an fdatasync covering the slot's last write returned
    while its entry was still held;
  * the open reads both slots, takes the newest VALID by seq, fdatasyncs it,
    and only then counts it durable; refuses when no slot is valid and one
    is torn.

Device (per file, per sector): a set of values the device may hold
(`dev`), the writes since (`pend`, in order, with their time) and what the
page cache holds (`cache`).  Power loss: each sector holds any value in dev
or any pending value.  Successful fdatasync begun at tb: a sector with
writes at or before tb holds the last of them for good.  FAILED fdatasync
begun at tb: those writes are dropped from pend and added to dev (the device
may hold any of them, or what it held), and the cache keeps the newest --
Linux marks the pages clean after failed writeback and never writes them
again.  A process kill keeps the page cache, and a reopen in a new process
reads it.

ops
  W  publish, its own fdatasync (a merge's, a promotion's landing)
  U  publish, its fdatasync owed (a drain: publish_unsynced)
  O  the owed fdatasync (sync_owed_slot), if one is owed
  B  publish, its fdatasync begun on another thread (the pending slot's helper)
  E  the oldest fdatasync in flight completes;  F  the newest completes
  w  publish whose write fails part way (every prefix of its units)
  V  publish whose own fdatasync fails (never retried)
  s  the owed or oldest in-flight fdatasync fails (never retried)
  K  a process kill inside a publish's write (every prefix), reopen
  X  process kill, reopen          P  power loss, reopen (every view)

properties (each reported by name)
  WRONG      the open takes a record no publish wrote whole (never published)
  LOSS_BRANCH the open takes a record a publish wrote whole that an earlier
             open passed over (a state the store has since contradicted: not
             in the history the store follows)
  LOSS       the open takes a state older than the newest acknowledged durable
             (acknowledged = the engine counted it durable: its sync returned,
             or the open's sync of it returned)
  NO_SLOT    a state reachable with no valid slot (refused or empty) after a
             slot was durable -- the store is created with slot 0 durable
  CLAIM      the engine counts a slot durable whose device bytes are not
             exactly the record it believes is there
  REFUSED    the chain check refused a legal crash state (chain=cross only)
  NONCE      a (key, nonce) sealed twice

options (mutants marked M)
  parity=1        M  the slot chosen by the sequence's parity
  land_issue=1    M  "durable" recorded where the sync is issued (B, U)
  keep_owed=1     M  the owed / in-flight entry not given up before a write
                     into its slot
  open_sync=taken|both|none
                     taken: the open fdatasyncs the slot it takes (the
                     engine's rule).  both (FIX candidate, E4 F6's open_sync):
                     it fdatasyncs both slot files.  none (M): it counts the
                     slot it takes durable without syncing it.
  noidx=1         M  the AAD without the slot's index (crash model: no
                     effect expected; adversary.py is where it shows)
  nocheck=1       M  the reader takes a record whose head is there, tag and
                     body unchecked
  torn_empty=1    M  a slot with bytes that do not authenticate reads as empty
  count_reuse=1   M  the nonce count given back after a failed write
  prefix_fixed=1  M  the nonce prefix per store, not per open
  chain=none|cross|last
                     none: no chain.  cross: the record carries the tag of
                     the record the engine knows durable in the OTHER slot,
                     and the open refuses a valid older other slot that is
                     not it.  last (M): the tag of the previous publish,
                     wherever it went.
  open_read=cache|device
                     cache: the open reads through the page cache (today).
                     device (FIX candidate): the open fdatasyncs both slot
                     files and drops their clean pages before it reads, so
                     what it reads is what the device holds.
  lens=3,2,4         record lengths in units, by seq (cycled)
"""
import itertools, sys, time, collections

NONE = None


class Fail(Exception):
    pass


OPT = dict(SU=2, NSEC=3, lens='3,2,4', parity=0, land_issue=0, keep_owed=0,            noidx=0, nocheck=0, open_sync='taken', torn_empty=0, count_reuse=0, prefix_fixed=0, chain='none',
           open_read='cache', maxP=1, maxX=2, take_old=0, keep_inflight=0, land_unheld=0, ignore_fail=0, w_whole=0, nolandcheck=0)


class Dev:
    def __init__(s, nf, nsec, su):
        s.nf, s.nsec, s.su = nf, nsec, su
        s.t = 0
        z = (None,) * su
        s.dev = {(f, j): frozenset([z]) for f in range(nf) for j in range(nsec)}
        s.pend = {}
        s.cache = {(f, j): z for f in range(nf) for j in range(nsec)}
        s.infl = {}
        s.ntok = 0

    def clone(s):
        d = Dev.__new__(Dev)
        d.nf, d.nsec, d.su, d.t, d.ntok = s.nf, s.nsec, s.su, s.t, s.ntok
        d.dev = dict(s.dev)
        d.pend = {k: list(v) for k, v in s.pend.items()}
        d.cache = dict(s.cache)
        d.infl = dict(s.infl)
        return d

    def write_units(s, f, units):
        """units: {pos: value}; one call; returns nothing"""
        s.t += 1
        secs = {}
        for pos, val in units.items():
            j = pos // s.su
            if j not in secs:
                secs[j] = list(s.cache[(f, j)])
            secs[j][pos % s.su] = val
        for j in sorted(secs):
            v = tuple(secs[j])
            s.cache[(f, j)] = v
            s.pend.setdefault((f, j), []).append((s.t, v))

    def sync_begin(s, f):
        s.t += 1
        s.ntok += 1
        s.infl[s.ntok] = (f, s.t)
        return s.ntok

    def sync_end(s, tok, ok=True):
        f, tb = s.infl.pop(tok)
        for k in list(s.pend):
            if k[0] != f:
                continue
            lst = s.pend[k]
            bef = [v for t, v in lst if t <= tb]
            if not bef:
                continue
            if ok:
                s.dev[k] = frozenset([bef[-1]])
            else:
                s.dev[k] = s.dev[k] | frozenset(bef)
            rest = [(t, v) for t, v in lst if t > tb]
            if rest:
                s.pend[k] = rest
            else:
                del s.pend[k]

    def sync(s, f, ok=True):
        s.sync_end(s.sync_begin(f), ok)

    def options(s, k):
        out = list(s.dev[k])
        for _, v in s.pend.get(k, []):
            if v not in out:
                out.append(v)
        return out

    def views(s):
        keys = sorted(s.dev)
        ch = [s.options(k) for k in keys]
        for pick in itertools.product(*ch):
            yield dict(zip(keys, pick))

    def settled(s, f, n):
        """the device holds exactly the cache for file f's first n units"""
        for j in range((n + s.su - 1) // s.su):
            k = (f, j)
            if k in s.pend or s.dev[k] != frozenset([s.cache[k]]):
                return False
        return True

    @staticmethod
    def from_view(old, v):
        d = Dev(old.nf, old.nsec, old.su)
        d.t = old.t
        d.dev = {k: frozenset([x]) for k, x in v.items()}
        d.cache = dict(v)
        return d


class World:
    def __init__(s):
        s.imgs = {}
        s.nonce = {}
        s.bad = []
        s.cnt = [0]
        s.pub = set()

    def clone(s):
        w = World.__new__(World)
        w.imgs, w.cnt, w.pub = s.imgs, s.cnt, s.pub
        w.nonce, w.bad = dict(s.nonce), list(s.bad)
        return w

    def uniq(s):
        s.cnt[0] += 1
        return s.cnt[0]

    def seal(s, key, nonce, content, **meta):
        k = (key, nonce)
        old = s.nonce.get(k)
        if old is not None:
            s.bad.append(('NONCE', 'reuse' if old != content else 'repeat', k))
        else:
            s.nonce[k] = content
        i = s.uniq()
        meta.update(key=key, nonce=nonce, content=content)
        s.imgs[i] = meta
        return i


def read_slot(W, v, f, o, nsec, su):
    units = []
    for j in range(nsec):
        units += list(v[(f, j)])
    if all(x is None for x in units):
        return ('EMPTY',)
    bad = ('EMPTY',) if o['torn_empty'] else ('TORN',)
    u0 = units[0]
    if not (isinstance(u0, tuple) and u0[1] == 0):
        return bad
    m = W.imgs[u0[0]]
    n = m['n']
    whole = n <= len(units) and all(units[k] == (u0[0], k) for k in range(n))
    if not whole:
        if o['nocheck'] and n <= len(units):
            return ('VALID', u0[0], m['seq'], tuple(units[:n]), m.get('pred'))
        return bad
    if m['key'] != 'Km' or m['store'] != 'S':
        return bad
    if not o['noidx'] and m['slot'] != f:
        return bad
    return ('VALID', u0[0], m['seq'], tuple(units[:n]), m.get('pred'))


def choose(W, v, o, nsec, su):
    """the open's rule: (verdict, taken record or None, the reads)"""
    r = [read_slot(W, v, f, o, nsec, su) for f in (0, 1)]
    valid = [(x[2], f, x) for f, x in enumerate(r) if x[0] == 'VALID']
    if not valid:
        if any(x[0] == 'TORN' for x in r):
            return 'REFUSE', None, r
        return 'NO_MANIFEST', None, r
    valid.sort(key=lambda t: (t[0], t[1]) if o.get('take_old') else (-t[0], t[1]))
    seq, f, x = valid[0]
    if o['chain'] in ('cross', 'last'):
        other = r[1 - f]
        if other[0] == 'VALID' and other[2] < seq and x[4] != other[1]:
            return 'REFUSE_CHAIN', None, r
    return 'OK', (f, x), r


class Tally:
    def __init__(s, name):
        s.name = name
        s.states = collections.Counter()
        s.bad = collections.Counter()
        s.first = {}
        s.runs = 0
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
        print('==', s.name, 'runs', s.runs, 'states', dict(s.states), 'violations', nb,
              'secs %.1f' % (time.time() - s.t0), file=out)
        for k in sorted(s.bad):
            print('   ', k, s.bad[k], file=out)
        for k in sorted(s.first):
            verdict, info = s.first[k]
            print('  first', k, verdict, file=out)
            for f in ('ops', 'trace', 'reads'):
                if info.get(f) is not None:
                    print('     %s' % f, info[f], file=out)
        out.flush()
        return nb


class Eng:
    """The segment work's slot state (Maint.slot, slot_sync_due, the helper)."""

    def __init__(s, W, D, o, seq, durable, dimg, life):
        s.W, s.D, s.o = W, D, o
        s.seq = seq
        s.durable = durable            # SlotChoice.durable
        s.dimg = list(dimg)            # the image the engine knows durable in each slot
        s.entries = []                 # sync entries: dict(slot, img, tok or None, held, kind)
        s.prefix = ('pfx', 'S') if o['prefix_fixed'] else ('pfx', life)
        s.count = 0
        s.lens = [int(x) for x in str(o['lens']).split(',')]
        s.last_img = dimg[durable] if durable is not None and durable in (0, 1) else None

    def next(s):
        if s.o['parity']:
            return s.seq % 2           # MUTANT: the publish being made is seq; slot = its parity
        return 1 - s.durable

    def seal(s, to):
        o = s.o
        n = s.lens[s.seq % len(s.lens)]
        if o['chain'] == 'cross':
            pred = s.dimg[1 - to]
        elif o['chain'] == 'last':
            pred = s.last_img          # MUTANT
        else:
            pred = None
        nonce = (s.prefix, s.count)
        content = (s.seq, to, s.W.uniq())
        img = s.W.seal('Km', nonce, content, store='S', slot=to, seq=s.seq, n=n, pred=pred)
        s.count += 1
        return img, n

    def give_up(s, to):
        if s.o['keep_owed']:
            return                     # MUTANT
        # an owed sync given up is dropped (write_slot's take()); one in
        # flight on another thread still completes on the device, unheld
        s.entries = [e for e in s.entries if not (e['slot'] == to and e['tok'] is None)]
        for e in s.entries:
            if e['slot'] == to and not s.o['keep_inflight']:
                e['held'] = False

    def failed(s, f):
        if s.o['ignore_fail']:
            return
        """a writeback of f failed: every sync of f owed or in flight through
        a descriptor opened before it reports the failure (errseq)"""
        for e in s.entries:
            if e['slot'] == f:
                e['failed'] = True

    def begin_publish(s):
        s.seq += 1
        to = s.next()
        s.give_up(to)
        img, n = s.seal(to)
        return to, img, n

    def write(s, to, img, n, prefix=None):
        k = n if prefix is None else prefix
        s.D.write_units(to, {i: (img, i) for i in range(k)})

    def land(s, to, img, orc, tally, ops, trace):
        s.durable = to
        s.dimg[to] = img
        orc.ack(img)
        if not s.o['nolandcheck'] and (not s.D.settled(to, s.W.imgs[img]['n']) or not s.cache_is(to, img)):
            tally.add('claim', ('CLAIM', to), {'ops': ops, 'trace': list(trace)})

    def cache_is(s, f, img):
        units = []
        for j in range(s.D.nsec):
            units += list(s.D.cache[(f, j)])
        n = s.W.imgs[img]['n']
        return all(units[k] == (img, k) for k in range(n))


class Orc:
    """hist: the published images in order (the engine's logical history in
    this life and the lives it came from); acked: index of the newest the
    engine has counted durable."""

    def __init__(s, first):
        s.hist = [first]
        s.ackpos = 0

    def copy(s):
        o = Orc.__new__(Orc)
        o.hist, o.ackpos = list(s.hist), s.ackpos
        return o

    def pub(s, img):
        s.hist.append(img)

    def ack(s, img):
        if img in s.hist:
            s.ackpos = max(s.ackpos, s.hist.index(img))

    def judge(s, img, W):
        if img not in s.hist:
            return ('LOSS_BRANCH', img) if img in W.pub else ('WRONG', img)
        p = s.hist.index(img)
        if p < s.ackpos:
            return ('LOSS', p, 'acked', s.ackpos)
        return None

    def cut(s, img):
        p = s.hist.index(img)
        s.hist = s.hist[:p + 1]
        s.ackpos = p


class Runner:
    def __init__(s, o, name):
        s.o = dict(OPT, **o)
        s.T = Tally(name)

    def run(s, ops):
        o = s.o
        W = World()
        D = Dev(2, o['NSEC'], o['SU'])
        # the store's birth: publish 0 into slot 0, synced, known durable
        E = Eng(W, D, o, 0, 1, [None, None], 0)
        E.seq = -1
        to, img, n = E.begin_publish()
        E.seq = 0
        E.write(to, img, n)
        D.sync(to)
        orc = Orc(img)
        W.pub.add(img)
        E.durable = to
        E.dimg[to] = img
        E.last_img = img
        s.go(W, D, E, orc, ops, 0, [], 1, 0, 0)
        s.T.runs += 1

    # -- the open -----------------------------------------------------------
    def check_view(s, W, v, orc, ops, trace, model):
        o = s.o
        verdict, taken, reads = choose(W, v, o, o['NSEC'], o['SU'])
        info = lambda: {'ops': ops, 'trace': list(trace), 'reads': reads}
        if verdict == 'REFUSE_CHAIN':
            s.T.add(model, ('REFUSED',), info)
            return None
        if verdict != 'OK':
            s.T.add(model, ('NO_SLOT', verdict), info)
            return None
        f, x = taken
        img, content = x[1], x[3]
        m = W.imgs[img]
        if content != tuple((img, k) for k in range(m['n'])):
            s.T.add(model, ('WRONG', 'mixed'), info)
            return None
        vd = orc.judge(img, W)
        s.T.add(model, vd, info)
        if vd:
            return None
        return taken, reads

    def reopen(s, W, D, orc, ops, i, trace, life, nP, nX, model):
        """D is the device after the crash (its cache is what the new process reads)."""
        o = s.o
        branches = [D]
        if o['open_read'] == 'device':
            # FIX: fdatasync both slot files, then drop their clean pages and
            # read what the device holds (branch over what it may hold)
            D = D.clone()
            if o['open_sync'] != 'none':
                for f in (0, 1):
                    D.sync(f)
            # a dirty page is not dropped (FADV_DONTNEED writes it back and
            # keeps it): the read sees the cache there
            keys = [k for k in sorted(D.dev) if k not in D.pend and
                    (len(D.dev[k]) > 1 or D.dev[k] != frozenset([D.cache[k]]))]
            branches = []
            for pick in itertools.product(*[sorted(D.dev[k], key=repr) for k in keys]):
                D2 = D.clone()
                for k, x in zip(keys, pick):
                    D2.dev[k] = frozenset([x])
                    D2.cache[k] = x
                branches.append(D2)
        for D2 in branches:
            res = s.check_view(W, D2.cache, orc, ops, trace, model)
            if res is None:
                continue
            (f, x), reads = res
            W2, D3, orc2 = W.clone(), D2.clone(), orc.copy()
            orc2.cut(x[1])
            other = reads[1 - f]
            dimg = [None, None]
            dimg[f] = x[1]
            if other[0] == 'VALID':
                dimg[1 - f] = other[1]
            E2 = Eng(W2, D3, o, x[2], f, dimg, life + 1)
            E2.last_img = x[1]
            if o['open_sync'] == 'taken':
                D3.sync(f)
            elif o['open_sync'] == 'both':
                D3.sync(0)
                D3.sync(1)
            E2.land(f, x[1], orc2, s.T, ops, trace)
            s.go(W2, D3, E2, orc2, ops, i, list(trace), life + 1, nP, nX)

    # -- the run --------------------------------------------------------------
    def go(s, W, D, E, orc, ops, i, trace, life, nP, nX):
        o = s.o
        while i < len(ops):
            op = ops[i]
            i += 1
            trace.append(op)
            if op in 'WUBV':
                to, img, n = E.begin_publish()
                E.write(to, img, n)
                orc.pub(img)
                W.pub.add(img)
                E.last_img = img
                if op == 'W':
                    D.sync(to)
                    E.land(to, img, orc, s.T, ops, trace)
                elif op == 'V':
                    D.sync(to, ok=False)
                    E.failed(to)
                elif op == 'U':
                    e = dict(slot=to, img=img, tok=None, held=True)
                    E.entries.append(e)
                    if o['land_issue']:
                        E.land(to, img, orc, s.T, ops, trace)      # MUTANT
                else:
                    tok = D.sync_begin(to)
                    E.entries.append(dict(slot=to, img=img, tok=tok, held=True))
                    if o['land_issue']:
                        E.land(to, img, orc, s.T, ops, trace)      # MUTANT
            elif op in 'Os':
                ow = [e for e in E.entries if e['tok'] is None]
                fl = [e for e in E.entries if e['tok'] is not None]
                if op == 'O' and ow:
                    e = ow[-1]
                    E.entries.remove(e)
                    ok = not e.get('failed')
                    D.sync(e['slot'], ok=ok)
                    if not ok:
                        E.failed(e['slot'])
                    elif e['held']:
                        E.land(e['slot'], e['img'], orc, s.T, ops, trace)
                elif op == 's' and (ow or fl):
                    e = ow[-1] if ow else fl[0]
                    E.entries.remove(e)
                    if e['tok'] is None:
                        D.sync(e['slot'], ok=False)
                    else:
                        D.sync_end(e['tok'], ok=False)
                    E.failed(e['slot'])
            elif op in 'EF':
                fl = [e for e in E.entries if e['tok'] is not None]
                if fl:
                    e = fl[0] if op == 'E' else fl[-1]
                    E.entries.remove(e)
                    ok = not e.get('failed')
                    D.sync_end(e['tok'], ok=ok)
                    if not ok:
                        E.failed(e['slot'])
                    elif e['held'] or o['land_unheld']:
                        E.land(e['slot'], e['img'], orc, s.T, ops, trace)
            elif op in 'wK':
                to, img, n = E.begin_publish()
                for k in range(1, n + (1 if (op == 'w' and o['w_whole']) else 0)):
                    W2, D2, E2, orc2 = W.clone(), D.clone(), s.cl(E, W, D), orc.copy()
                    E2.W, E2.D = W2, D2
                    E2.write(to, img, n, prefix=k)
                    tr = trace + ['%s%d' % (op, k)]
                    if op == 'w':
                        if o['count_reuse']:
                            E2.count -= 1                           # MUTANT
                        s.go(W2, D2, E2, orc2, ops, i, tr, life, nP, nX)
                    else:
                        s.reopen(W2, D2, orc2, ops, i, tr, life, nP, nX + 1, 'proc')
                return
            elif op == 'X':
                s.reopen(W, D.clone(), orc, ops, i, trace, life, nP, nX + 1, 'proc')
                return
            elif op == 'P':
                seen = set()
                for v in D.views():
                    key = frozenset(v.items())
                    if key in seen:
                        continue
                    seen.add(key)
                    s.reopen(W, Dev.from_view(D, v), orc, ops, i, trace, life, nP + 1, nX, 'any')
                return
        # end of the sequence: the page cache (a kill now) and every device view
        s.check_view(W, D.cache, orc, ops, trace, 'proc')
        seen = set()
        for v in D.views():
            key = frozenset(v.items())
            if key in seen:
                continue
            seen.add(key)
            s.check_view(W, v, orc, ops, trace, 'any')
        for b in W.bad:
            s.T.add('seal', (b[0], b[1]), {'ops': ops, 'trace': trace})
            break

    @staticmethod
    def cl(E, W, D):
        e = Eng.__new__(Eng)
        e.__dict__.update(E.__dict__)
        e.dimg = list(E.dimg)
        e.entries = [dict(x) for x in E.entries]
        return e


def well_formed(t, o):
    if t.count('P') > o['maxP'] or t.count('X') + t.count('K') > o['maxX']:
        return False
    return True


def main():
    o = {}
    for a in sys.argv[1:]:
        k, v = a.split('=', 1)
        try:
            v = int(v)
        except ValueError:
            pass
        o[k] = v
    maxlen = o.pop('maxlen', 4)
    alpha = o.pop('alpha', 'WUOBEFwVsKXP')
    tlimit = o.pop('tlimit', 540)
    name = 'slots %s maxlen=%d alpha=%s' % (' '.join('%s=%s' % kv for kv in sorted(o.items())), maxlen, alpha)
    R = Runner(o, name)
    t0 = time.time()
    done = True
    for n in range(1, maxlen + 1):
        for t in itertools.product(alpha, repeat=n):
            if not well_formed(t, R.o):
                continue
            R.run(t)
            if time.time() - t0 > tlimit:
                done = False
                break
        if not done:
            print('time limit at length', n, 'last', t)
            break
    nb = R.T.report()
    sys.exit(1 if nb else 0)


if __name__ == '__main__':
    main()
