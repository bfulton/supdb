#!/usr/bin/env python3
"""Recycle and the session table's compaction: Log files under a directory.

  python3 dirmodels.py recycle [opt=value ...]
  python3 dirmodels.py compact [opt=value ...]

A directory entry is a sealed spelling (fid, logical name, name_seq, flags,
prev fid); the directory maps spellings to inodes.  Directory operations land
in any subset, in order, until sync_dir (dir_mode=prefix: ext4's journal).

recycle (sec. 4 Recycle): the spare, a retired WAL whose inode still holds
its last life (header A, or A and B after a resume in that life, its frames
and pages), is renamed to the new WAL's spelling with flags.recycled and the
spare's file_id as prev, under a new file_id whose header (one restart
session) is written unsynced.  The engine then commits into it; its first
sync by policy is the directory barrier and the file's fdatasync
(db.rs:27035-27046, sync_wal).  An open reads each entry: headers that
authenticate under the entry's file_id; else, for an entry flagged recycled,
a header under prev reads as an empty log; else a header under another
file_id is refused (design) or read as empty (fix: `mismatch=empty`).
checks: the open never refuses; the WAL's content is legal; the WAL never
shows the last life's bytes.

compact (sec. 4 Table capacity): a resumed Log file whose table is full is
written, recovered content and all, under one restart session to
`<name>.pfcopy` under a new file_id and synced; then rename(2) to a new
spelling of the logical name with a higher name_seq, sync_dir, unlink the
old spelling; the engine then appends to the copy.  An open keeps, of two
authenticating entries for one logical name, the higher name_seq, and
removes `.pfcopy` names.
"""
import sys, itertools, time
import pfm
from pfm import LogH, Orc, World, Tally, Blocked, TORN, PB


def spell(fid, name, ns, recycled=False, prev=None):
    return ('x', fid, name, ns, recycled, prev)


def entries(d):
    return [(sp, ino) for sp, ino in d.items() if isinstance(sp, tuple) and sp[0] == 'x']


def open_entry(W, v, sp, ino, o):
    """Recover one entry's Log file; returns (state, refused)."""
    _, fid, name, ns, rec, prev = sp
    hs = pfm.headers(W, v, ino, fid)
    if hs:
        return pfm.recover_log(W, v, ino, fid, o), None
    if rec and prev is not None and pfm.headers(W, v, ino, prev):
        return {'empty': True, 'why': 'recycled, header lags'}, None
    # a header under any other file_id?  (the clear part names it)
    others = []
    for slot in (0, 1):
        img = v.get((ino, ('h', slot)))
        if img not in (None, TORN) and W.imgs[img]['kind'] == 'hdr':
            others.append(W.imgs[img]['fid'])
    if others and o['mismatch'] == 'refuse':
        return None, ('REFUSED', name, 'header of fid', others, 'entry fid', fid)
    return {'empty': True, 'why': 'no header'}, None


# ---------------------------------------------------------------- recycle
def recycle_run(o, ops, T, old_life):
    W = World()
    I = 20
    F0 = W.uniq('fid')
    # the last life: a WAL with content, then retired and renamed to a spare
    orc0 = Orc()
    h0 = LogH.create(W, I, F0, o, orc0)
    h0.write(5)
    h0.sync()
    if old_life == 'resumed':
        st = pfm.recover_log(W, W.dev.pc()[0], I, F0, o)
        h0 = LogH.resume(W, I, F0, o, orc0, st)
        h0.write(3)
        h0.sync()
    spare = spell(F0, 'spare-1', 1)
    W.dev.dop(('create', spare, I))
    W.dev.dsync()
    # recycle
    F1 = W.uniq('fid')
    wal = spell(F1, 'wal-2', 2, not o['no_prev'], F0 if not (o['same_fid'] or o['no_prev']) else None)
    if o['same_fid']:
        F1 = F0
        wal = spell(F0, 'wal-2', 2, True, None)
    orc = Orc()
    newest = max(pfm.headers(W, W.dev.pc()[0], I, F0))[1]
    if o['order'] == 'rename_first':
        W.dev.dop(('rename', spare, wal))
    slot = None
    if o['rslot'] == 'other':
        slot = newest                       # create() writes hdr_slot ^ 1
    o2 = dict(o, resume_sync=False)         # a recycle issues no sync of its own
    if o['same_fid']:
        h = LogH.create(W, I, F1, o2, orc, hdr=(max(pfm.headers(W, W.dev.pc()[0], I, F0))[0], newest))
    else:
        h = LogH.create(W, I, F1, o2, orc, hdr=None if slot is None else (0, slot))
    if o['order'] != 'rename_first':
        W.dev.dop(('rename', spare, wal))
    trace = []
    legal_wal = lambda: orc.legal()

    def check(model, v, d, legal):
        es = entries(d)
        for sp, ino in es:
            st, ref = open_entry(W, v, sp, ino, o)
            if ref:
                T.add(model, ref, {'ops': ops, 'trace': list(trace)})
                return
            if sp[2] == 'wal-2':
                c = tuple(st['content']) if not st['empty'] else ()
                if not st['empty'] and st['unreadable']:
                    T.add(model, ('UNREADABLE',), {'ops': ops, 'trace': list(trace)})
                    return
                if c not in legal:
                    T.add(model, ('WRONG' if c not in orc.hist else 'LOSS', c),
                          {'ops': ops, 'trace': list(trace)})
                    return
        if not any(sp[2] == 'wal-2' for sp, _ in es) and () not in legal:
            T.add(model, ('LOSS', 'no wal-2 entry'), {'ops': ops, 'trace': list(trace)})
            return
        T.add(model, None, {})

    def anyc():
        for v, d in W.dev.any_views(o['dir_mode']):
            check('any', v, d, orc.legal())
    W.hooks = [anyc]
    for op in ops:
        trace.append(op)
        if isinstance(op, int):
            h.write(op)
        elif op == 'S':
            W.dev.dsync()                   # the directory barrier the new name owes
            h.sync()
        v, d = W.dev.pc()
        check('proc', v, d, {orc.hist[-1]})
    anyc()
    T.runs += 1


# ---------------------------------------------------------------- compaction
def compact_run(o, ops, T):
    W = World()
    A, B = 30, 31
    FA = W.uniq('fid')
    orc = Orc()
    h = LogH.create(W, A, FA, o, orc)
    pre, post = ops
    for op in pre:
        if op == 'S':
            h.sync()
        else:
            h.write(op)
    ea = spell(FA, 'wal-1', 1)
    W.dev.dop(('create', ea, A))
    W.dev.dsync()
    trace = list(pre) + ['X']
    # a process crash; the reopen finds the table full and compacts
    v, d = W.dev.pc()
    st = pfm.recover_log(W, v, A, FA, o)
    content = st['content']
    FB = W.uniq('fid')
    tmp = spell(FB, 'wal-1.pfcopy', 2)
    new = spell(FB, 'wal-1', 3 if o['ns'] == 'higher' else 0)
    W.dev.dop(('create', tmp, B))
    hb = LogH.create(W, B, FB, o, orc)
    hb.write(0, vals=content)
    orc.hist.pop()                          # the copy is no new state of the logical file

    def pick(d):
        cands = [(sp, ino) for sp, ino in entries(d) if sp[2] == 'wal-1']
        if not cands:
            return None
        if o['keep'] == 'higher':
            return max(cands, key=lambda c: c[0][3])
        return min(cands, key=lambda c: c[0][3])

    def check(model, v, d, legal):
        c = pick(d)
        if c is None:
            T.add(model, ('LOSS', 'no entry'), {'ops': ops, 'trace': list(trace)})
            return
        sp, ino = c
        st2, ref = open_entry(W, v, sp, ino, o)
        if ref:
            T.add(model, ref, {'ops': ops, 'trace': list(trace)})
            return
        cc = tuple(st2['content']) if not st2['empty'] else ()
        if not st2['empty'] and st2['unreadable']:
            T.add(model, ('UNREADABLE', sp[3]), {'ops': ops, 'trace': list(trace)})
            return
        if cc not in legal:
            T.add(model, ('LOSS' if cc in orc.hist else 'WRONG', len(cc), 'entry ns', sp[3]),
                  {'ops': ops, 'trace': list(trace)})
            return
        T.add(model, None, {})

    def anyc():
        for v, d in W.dev.any_views(o['dir_mode']):
            check('any', v, d, orc.legal())
    W.hooks = [anyc]
    steps = []
    if o['copy_sync']:
        steps.append('fsync')
    if o['order_c'] == 'unlink_first':
        steps += ['unlink', 'rename']
    else:
        steps += ['rename']
        if o['dsync']:
            steps.append('dsync')
        steps.append('unlink')
    for s_ in steps:
        trace.append(s_)
        if s_ == 'fsync':
            a0 = orc.ack
            hb.sync()
            orc.ack = a0                    # the layer's copy acknowledges nothing new
        elif s_ == 'rename':
            W.dev.dop(('rename', tmp, new))
        elif s_ == 'dsync':
            W.dev.dsync()
        elif s_ == 'unlink':
            W.dev.dop(('unlink', ea))
        v, d = W.dev.pc()
        check('proc', v, d, {orc.hist[-1]})
    for op in post:
        trace.append(op)
        if op == 'S':
            hb.sync()
        else:
            hb.write(op)
        v, d = W.dev.pc()
        check('proc', v, d, {orc.hist[-1]})
    anyc()
    T.runs += 1


OPT = dict(mismatch='refuse', same_fid=False, no_prev=False, order='header_first', rslot='A',
           dir_mode='subset', ns='higher', keep='higher', copy_sync=True, dsync=True,
           order_c='rename_first')

if __name__ == '__main__':
    which = sys.argv[1]
    o = dict(pfm.DEFAULT, resume_sync=True, sh_rehome=True, retry='R2', **OPT)
    for a in sys.argv[2:]:
        k, v = a.split('=')
        o[k] = {'True': True, 'False': False}.get(v, int(v) if v.isdigit() else v)
    T = Tally('%s %s' % (which, {k: v for k, v in o.items()
                                 if k in OPT and OPT[k] != v}))
    if which == 'recycle':
        for life in ('fresh', 'resumed'):
            for n in range(0, 4):
                for ops in itertools.product([1, 5, 'S'], repeat=n):
                    recycle_run(o, ops, T, life)
    else:
        for pre in [(5, 'S'), (1, 'S', 5), (5, 5, 'S', 1), (3, 'S', 5, 'S')]:
            for n in range(0, 4):
                for post in itertools.product([1, 5, 'S'], repeat=n):
                    compact_run(o, (pre, post), T)
    sys.exit(1 if T.report() else 0)
