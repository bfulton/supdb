"""A spare recycled twice: the first recycle's header lands and its rename
does not (power loss before any sync, so nothing in it was acked), the open
keeps the spare (db.rs:27858-27865), and the next rotation recycles it again.
Uses dirmodels' spellings and open_entry; the spare's entry is never opened."""
import sys, itertools
sys.path.insert(0, '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e4-model')
import pfm, dirmodels
from pfm import LogH, Orc, World, Tally, TORN
from dirmodels import spell, entries, open_entry

def run(o, rslot, prev_from, T, ops2):
    W = World(); I = 20
    F0 = W.uniq('fid'); orc0 = Orc()
    h0 = LogH.create(W, I, F0, o, orc0); h0.write(5); h0.sync()
    spare = spell(F0, 'spare-1', 1)
    W.dev.dop(('create', spare, I)); W.dev.dsync()
    # recycle 1
    F1 = W.uniq('fid'); o2 = dict(o, resume_sync=False)
    newest = max(pfm.headers(W, W.dev.pc()[0], I, F0))[1]
    h1 = LogH.create(W, I, F1, o2, Orc(), hdr=None if rslot == 'A' else (0, newest))
    W.dev.dop(('rename', spare, spell(F1, 'wal-2', 2, True, F0)))
    # power loss: every state where the rename did not land and F1's header did
    n = 0
    for v, d in W.dev.any_views():
        if not any(sp[2] == 'spare-1' for sp, _ in entries(d)):
            continue
        hs1 = pfm.headers(W, v, I, F1)
        if not hs1:
            continue
        n += 1
        W2 = W.clone_after_loss(v, d)
        # the next rotation recycles spare-1 again
        F2 = W2.uniq('fid')
        # which slot holds "the spare's newest header": any header that authenticates under its own fid
        hdrs = []
        for slot in (0, 1):
            img = v.get((I, ('h', slot)))
            if img not in (None, TORN) and W2.imgs[img]['kind'] == 'hdr':
                hdrs.append((W2.imgs[img]['hdr_seq'], slot, W2.imgs[img]['fid']))
        top = max(hdrs)
        prev = F0 if prev_from == 'spelling' else top[2]
        orc = Orc()
        h2 = LogH.create(W2, I, F2, o2, orc, hdr=None if rslot == 'A' else (0, top[1]))
        wal = spell(F2, 'wal-3', 3, True, prev)
        W2.dev.dop(('rename', spare, wal))
        for op in ops2:
            if op == 'S':
                W2.dev.dsync(); h2.sync()
            else:
                h2.write(op)
        for v2, d2 in W2.dev.any_views():
            for sp, ino in entries(d2):
                if sp[2] != 'wal-3':
                    continue
                st, ref = open_entry(W2, v2, sp, ino, o)
                if ref:
                    T.add('any', ref, {'ops': (rslot, prev_from, ops2)}); break
                c = tuple(st['content']) if not st['empty'] else ()
                T.add('any', None if c in orc.legal() else ('LOSS/WRONG', c), {'ops': (rslot, prev_from, ops2)})
    return n

for mismatch in ('refuse', 'empty'):
    for rslot in ('A', 'other'):
        for prev_from in ('spelling', 'inode'):
            o = dict(pfm.DEFAULT, resume_sync=True, sh_rehome=True, retry='R2', **dict(dirmodels.OPT, mismatch=mismatch))
            T = Tally('mismatch=%s rslot=%s prev_from=%s' % (mismatch, rslot, prev_from))
            n = 0
            for k in range(0, 3):
                for ops2 in itertools.product([1, 'S'], repeat=k):
                    n += run(o, rslot, prev_from, T, ops2)
            print('first-recycle loss states continued:', n)
            T.report()
