#!/usr/bin/env python3
"""The long-name spill under a power cut: the E4 directory model extended.

  nice -n 10 timeout 600 python3 spill.py [n=4] [opt=value ...]

Device: pfm.Dev, unchanged (copied from work/e4-model).  Every object is
written whole by one syscall and at a crash holds its durable image, any
image written since the barrier that covers it, or TORN; directory
operations land in any in-order subset until sync_dir ('subset', POSIX) or
a prefix ('prefix', ext4's ordered journal).  Power-loss states are
enumerated just before every barrier completes (a file's fdatasync and the
directory's fsync) and at the end of every run: between two barriers the
set only grows, so every cut point is covered.  A process crash keeps the
page cache (pfm.Dev.pc).

The design (design.md beside this file):
  * an entry is the sealed spelling ('x', fid, nonce, kind, payload, ns):
    kind 'in' carries the logical name inline; kind 'spill' carries
    (slot, rtag) -- the name record's slot in the file's name pages and the
    record's tag -- and the entry's name_seq ns.  The flag is the kind.
  * a name record is a sealed page of the file: (fid, slot, ns, name),
    sealed under K_f, its tag the image id.  A spilled entry resolves iff
    the record in its slot verifies under its fid and its tag is the
    entry's rtag (bind=tag).
  * the record is written before the directory op (order=record_first);
    vault::sync_dir first fdatasyncs every file with a name record written
    since its last sync began (dsrule=True), then fsyncs the directory.
  * slot allocation: the lowest slot no entry of the file in the layer's
    page-cache directory references (alloc=free).
  * the open: an unresolved spilled entry is withheld from the name table;
    a manifest name without a resolving entry refuses the open (the
    engine's missing-file refusal) with nothing removed; otherwise the
    unresolved entries are swept with the names no manifest holds, and of
    two entries for one name the higher ns is kept.  ns restarts at the
    largest listed ns plus one.

The engine's ops (each the shape of a real path, db.rs cited in notes.md):
  C  merge output: create inline temp, write, rename to a long name, fsync
     the file, sync_dir, publish the manifest with the name
  T  a long temp renamed to a long name (a spilled `.tmp`), as C
  D  a new long-named file created under its name, written, fsync,
     sync_dir, publish
  P  promotion: link the newest manifest name to a new long name (no file
     fsync: db.rs:40724-40765), sync_dir, publish the swap, unlink the old
  R  rename-over: a rebuilt file under an inline temp, fsynced, renamed
     over the newest manifest name (the layer's verb: record, rename,
     sync_dir, unlink the old spelling); the manifest is unchanged
  U  retire: publish without the name, unlink it
  c t d p r u  the op cut at its first barrier: the process dies there, and
     the next op is X or L (the run continues from the cut's states)
  X  process crash and reopen
  L  power cut and reopen: branches over every crash state at that point

Checks, in every power-loss state and after every step (page cache):
  MISNAMED  an entry resolves to a name other than the one it was made for
            (an entry taken for a file the store named under another name)
  AMBIG     two entries resolve to one name and the open cannot order them
            (equal ns), or the one it keeps (highest ns) is not the newest
  UNNAMED   a name the manifest on the device holds has no resolving entry
            (the open would refuse)

Options (mutants marked M):
  flag=honour|ignored(M1)  ignored: a spilled entry's inline field is read
                           as its name
  order=record_first|entry_first
  dsrule=True|False(M2)    False: sync_dir syncs the directory alone, so a
                           record can lag a durable entry with no rule
  bind=tag|seq(M3b)|none(M3)  none: the record's name is taken whatever
                           record is in the slot; seq: bound by (ns, slot)
                           alone, not by the record's tag
  alloc=free|fixed(M4)     fixed: always slot 0
  NS=4                     name slots a file
  dir_mode=subset|prefix
  ns_open=listed|zero(M5)  zero: the open restarts ns at 1
"""
import sys, os, itertools, collections, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pfm import Dev, Tally, TORN

MAN = ('MAN', ('m',))


class Full(Exception):
    pass


class Cut(Exception):
    """the process dies at the op's first barrier (a lowercase op)"""


class Stop(Exception):
    """the open refused: the run ends here (the refusal is counted)"""


class World:
    def __init__(s):
        s.dev = Dev()
        s.imgs = {}
        s.cnt = [0]
        s.hooks = []
        s.truth = {}          # spelling -> (logical name, creation index)

    def uniq(s):
        s.cnt[0] += 1
        return s.cnt[0]

    def after_loss(s, v, d):
        w = World.__new__(World)
        w.dev = Dev.after_loss(s.dev.t, v, d)
        w.imgs, w.cnt, w.truth = s.imgs, s.cnt, s.truth
        w.hooks = []
        return w

    def hook(s):
        for h in s.hooks:
            h()


def long_name(n):
    return isinstance(n, str) and n.startswith('L')


# ---------------------------------------------------------------- resolution
def resolve(w, v, d, o):
    out = []
    for sp, ino in d.items():
        if not (isinstance(sp, tuple) and sp[0] == 'x'):
            continue
        _, fid, nonce, kind, pay, ns = sp
        if kind == 'in':
            out.append((sp, ino, pay, ns))
            continue
        if o['flag'] == 'ignored':
            out.append((sp, ino, ('stub', pay[0]), ns))
            continue
        slot, rtag = pay
        img = v.get((ino, ('rec', slot)))
        name = None
        if img not in (None, TORN):
            m = w.imgs[img]
            if m['fid'] == fid:       # sealed under K_f(fid): another fid fails
                if o['bind'] == 'tag':
                    ok = img == rtag
                elif o['bind'] == 'seq':
                    ok = m['ns'] == ns and m['slot'] == slot
                else:
                    ok = True
                if ok:
                    name = m['name']
        out.append((sp, ino, name, ns))
    return out


def man_of(w, v):
    m = v.get(MAN)
    if m is TORN:                    # the engine's slots give the one before
        m = w.dev.dur.get(MAN)
    return m or ()


def check(w, v, d, o, T, model, info):
    res = resolve(w, v, d, o)
    by = collections.defaultdict(list)
    for sp, ino, name, ns in res:
        if name is None:
            continue
        tn, ci = w.truth[sp]
        if name != tn:
            T.add(model, ('MISNAMED', name, 'made for', tn), info)
            return
        by[name].append((ns, ci, sp))
    for name, g in by.items():
        if len(g) > 1:
            if len({ns for ns, _, _ in g}) < len(g):
                T.add(model, ('AMBIG', name, 'equal ns'), info)
                return
            if max(g)[1] != max(ci for _, ci, _ in g):
                T.add(model, ('AMBIG', name, 'kept an older entry'), info)
                return
    for n in man_of(w, v):
        if n not in by:
            T.add(model, ('UNNAMED', n), info)
            return
    T.add(model, None, {})


# ---------------------------------------------------------------- the layer
class Layer:
    def __init__(s, w, o, T, trace):
        s.w, s.o, s.T, s.trace = w, o, T, trace
        s.ns = 1
        s.table = {}        # logical name -> spelling
        s.spells = {}       # spelling -> ino (the page-cache directory, ours)
        s.fid = {}          # ino -> fid
        s.rec_dirty = set()
        s.cut = False

    def proc(s, what):
        s.trace.append(what)
        v, d = s.w.dev.pc()
        check(s.w, v, d, s.o, s.T, 'proc', {'ops': s.T.cur, 'trace': list(s.trace)})

    def alloc(s, fid):
        if s.o['alloc'] == 'fixed':
            return 0
        used = {sp[4][0] for sp in s.spells if sp[1] == fid and sp[3] == 'spill'}
        for k in range(s.o['NS']):
            if k not in used:
                return k
        raise Full()

    def spelling(s, ino, name):
        fid = s.fid[ino]
        ns = s.ns
        s.ns += 1
        nonce = s.w.uniq()
        recs = []
        if long_name(name):
            slot = s.alloc(fid)
            img = s.w.uniq()
            s.w.imgs[img] = dict(fid=fid, slot=slot, ns=ns, name=name)
            sp = ('x', fid, nonce, 'spill', (slot, img), ns)
            recs = [(('rec', slot), img)]
        else:
            sp = ('x', fid, nonce, 'in', name, ns)
        s.w.truth[sp] = (name, s.w.uniq())
        return sp, recs

    def _name_op(s, ino, name, dop):
        sp, recs = s.spelling(ino, name)

        def rec():
            if recs:
                s.w.dev.write(ino, recs)
                s.rec_dirty.add(ino)
                s.proc(('rec', name))

        def ent():
            s.w.dev.dop(dop(sp))
            s.spells[sp] = ino
            s.proc((dop(sp)[0], name))
        if s.o['order'] == 'record_first':
            rec(); ent()
        else:
            ent(); rec()
        return sp

    def create(s, ino, fid, name):
        s.fid[ino] = fid
        s.table[name] = s._name_op(ino, name, lambda sp: ('create', sp, ino))

    def rename(s, old, new):
        osp = s.table.pop(old)
        ino = s.spells[osp]
        sp = s._name_op(ino, new, lambda sp: ('rename', osp, sp))
        del s.spells[osp]
        s.table[new] = sp

    def link(s, old, new):
        osp = s.table[old]
        ino = s.spells[osp]
        s.table[new] = s._name_op(ino, new, lambda sp: ('link', osp, sp))

    def rename_over(s, tmp, name):
        """rename(2) to a new spelling of an existing name, sync_dir, unlink
        the old spelling (design.md sec. 4, rename-over)."""
        osp = s.table[name]
        tsp = s.table.pop(tmp)
        ino = s.spells[tsp]
        sp = s._name_op(ino, name, lambda sp: ('rename', tsp, sp))
        del s.spells[tsp]
        s.table[name] = sp
        s.sync_dir()
        s.unlink_sp(osp, name)

    def unlink(s, name):
        s.unlink_sp(s.table.pop(name), name)

    def unlink_sp(s, sp, what):
        s.w.dev.dop(('unlink', sp))
        s.spells.pop(sp, None)
        s.proc(('unlink', what))

    def write_data(s, ino):
        s.w.dev.write(ino, [(('d',), s.w.uniq())])
        s.proc(('data', ino))

    def barrier(s):
        if s.cut:
            raise Cut()

    def fsync(s, ino):
        s.barrier()
        tok = s.w.dev.sync_begin(ino)
        s.rec_dirty.discard(ino)
        s.w.hook()
        s.w.dev.sync_end(tok)
        s.proc(('fsync', ino))

    def sync_dir(s):
        s.barrier()
        if s.o['dsrule']:
            for ino in sorted(s.rec_dirty, key=repr):
                s.fsync(ino)
        s.w.hook()
        s.w.dev.dsync()
        s.proc(('sync_dir',))


# ---------------------------------------------------------------- the engine
class Engine:
    def __init__(s, lay):
        s.lay = lay
        s.M = []            # the manifest's names, in order
        s.loc = {}          # name -> ino
        s.k = 0

    def fresh(s, kind):
        s.lay.w.cnt[0] += 1
        return '%s%d' % (kind, s.lay.w.cnt[0])

    def publish(s, M):
        s.lay.barrier()
        s.M = list(M)
        w = s.lay.w
        w.dev.write('MAN', [(('m',), tuple(s.M))])
        s.lay.proc(('man write', tuple(s.M)))
        tok = w.dev.sync_begin('MAN')
        w.hook()
        w.dev.sync_end(tok)
        s.lay.proc(('man sync',))

    def new_file(s):
        ino = s.fresh('i')
        return ino, s.fresh('f')

    def op(s, c):
        L = s.lay
        if c in 'CTD':
            ino, fid = s.new_file()
            name = s.fresh('L')
            if c == 'D':
                L.create(ino, fid, name)
            else:
                tmp = s.fresh('L' if c == 'T' else 't')
                L.create(ino, fid, tmp)
            L.write_data(ino)
            if c != 'D':
                L.rename(tmp, name)
            L.fsync(ino)
            L.sync_dir()
            s.loc[name] = ino
            s.publish(s.M + [name])
        elif c in 'PRU':
            if not s.M:
                return False
            old = s.M[-1]
            if c == 'P':
                new = s.fresh('L')
                L.link(old, new)
                L.sync_dir()
                s.loc[new] = s.loc[old]
                s.publish([n for n in s.M if n != old] + [new])
                L.unlink(old)
                del s.loc[old]
            elif c == 'R':
                ino, fid = s.new_file()
                tmp = s.fresh('t')
                L.create(ino, fid, tmp)
                L.write_data(ino)
                L.fsync(ino)
                L.rename_over(tmp, old)
                s.loc[old] = ino
            else:
                s.publish([n for n in s.M if n != old])
                L.unlink(old)
                del s.loc[old]
        return True


def open_store(w, o, T, trace, v, d):
    """The open over page cache v, d (a reopen) of world w."""
    lay = Layer(w, o, T, trace)
    eng = Engine(lay)
    res = resolve(w, v, d, o)
    man = man_of(w, v)
    named = {n for _, _, n, _ in res if n is not None}
    if any(n not in named for n in man):
        T.refused += 1
        raise Stop()                 # refused, nothing removed (UNNAMED counted)
    tok = w.dev.sync_begin('MAN')    # the open syncs the slot it takes
    w.hook()
    w.dev.sync_end(tok)
    for sp, ino, n, ns in res:
        lay.spells[sp] = ino
        lay.fid[ino] = sp[1]
    lay.ns = 1 if o['ns_open'] == 'zero' else max([ns for _, _, _, ns in res] + [0]) + 1
    keep = {}
    for sp, ino, n, ns in res:
        if n is not None and n in man and (n not in keep or ns > keep[n][3]):
            keep[n] = (sp, ino, n, ns)
    for sp, ino, n, ns in res:
        if keep.get(n, (None,))[0] != sp:
            lay.unlink_sp(sp, ('sweep', n))
    for n, (sp, ino, _, _) in keep.items():
        lay.table[n] = sp
        eng.loc[n] = ino
    eng.M = list(man)
    return lay, eng


def setup(o, T, trace):
    """A store whose manifest names one file under the inline name n0."""
    w = World()
    lay = Layer(w, o, T, trace)
    eng = Engine(lay)
    lay.create('i0', 'f0', 'n0')
    lay.write_data('i0')
    lay.fsync('i0')
    lay.sync_dir()
    eng.loc['n0'] = 'i0'
    eng.publish(['n0'])
    return w, lay, eng


def views(w, o):
    for v, d in w.dev.any_views(o['dir_mode']):
        if v.get(MAN) is TORN:
            v = dict(v)
            v[MAN] = w.dev.dur.get(MAN)
        yield v, d


def run(o, ops, T):
    T.cur = ops
    trace = []
    w, lay, eng = setup(o, T, trace)
    trace.append('--')
    go(o, ops, T, w, lay, eng, trace, 0)
    T.runs += 1


def go(o, ops, T, w, lay, eng, trace, depth):
    def anyc():
        for v, d in views(w, o):
            check(w, v, d, o, T, 'any', {'ops': T.cur, 'trace': list(trace)})
    w.hooks = [anyc]
    try:
        for i, c in enumerate(ops):
            if c == 'X':
                trace.append('X')
                v, d = w.dev.pc()
                lay, eng = open_store(w, o, T, trace, v, d)
                w.hooks = [anyc]
            elif c == 'L':
                anyc()
                rest = ops[i + 1:]
                seen = set()
                for v, d in list(views(w, o)):
                    key = (tuple(sorted(v.items(), key=repr)), tuple(sorted(d.items(), key=repr)))
                    if key in seen:
                        continue
                    seen.add(key)
                    w2 = w.after_loss(v, d)
                    tr2 = list(trace) + ['L']
                    try:
                        l2, e2 = open_store(w2, o, T, tr2, v, d)
                    except Stop:
                        continue
                    go(o, rest, T, w2, l2, e2, tr2, depth + 1)
                return
            else:
                trace.append(c)
                lay.cut = c.islower()
                try:
                    if not eng.op(c.upper()):
                        T.skipped += 1
                except Cut:
                    pass
                lay.cut = False
        anyc()
    except Stop:
        pass
    except Full:
        T.full += 1


OPT = dict(flag='honour', order='record_first', dsrule=True, bind='tag', alloc='free',
           NS=4, dir_mode='subset', ns_open='listed')

if __name__ == '__main__':
    o = dict(OPT)
    n = 4
    alpha = 'CTDPRUXL'
    for a in sys.argv[1:]:
        k, v = a.split('=')
        if k == 'n':
            n = int(v)
        elif k == 'alpha':
            alpha = v
        else:
            o[k] = {'True': True, 'False': False}.get(v, int(v) if v.isdigit() else v)
    T = Tally('spill n=%d alpha=%s %s' % (n, alpha, {k: v for k, v in o.items() if OPT[k] != v}))
    T.refused = T.skipped = T.full = 0
    for m in range(0, n + 1):
        for ops in itertools.product(alpha, repeat=m):
            if ops and ops[-1] in 'XL':
                continue                 # a trailing crash adds no state the end check misses
            if any(c.islower() and (i + 1 == len(ops) or ops[i + 1] not in 'XL')
                   for i, c in enumerate(ops)):
                continue                 # a cut op is a process that died: a crash follows
            run(o, ''.join(ops), T)
    print('   refused opens', T.refused, 'skipped ops', T.skipped, 'slot table full', T.full)
    sys.exit(1 if T.report() else 0)
