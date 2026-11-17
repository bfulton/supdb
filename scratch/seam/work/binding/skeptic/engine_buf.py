#!/usr/bin/env python3
"""The run's close as the engine does it, with the writer's buffering.

What the earlier models take as given and the engine does not do:

  * A run commit that is not due is not written: the records and the marker
    go to the AlignedWriter's buffer (`mark`, db.rs:6757-6761, through
    `Write::write`, 6293-6318) or stay in the held stream (`mark_stream` ->
    `stream_flush(false)`, 6893, whole 1 MiB pieces only, 5707-5753). Only a
    due sync (`SegmentWriter::sync`, 6917-6937) or a whole piece puts them on
    the file. Modelled: RC leaves `w` alone unless due; PF (a piece filled)
    moves `w` to any count up to the committed one.
  * The close hands the writer, unflushed, to the seal thread
    (`close_run` 29047-29052, `hand_seal` 31015). The thread writes the tail
    only once the trailer is composed (`finish_unsynced_with`, the stream
    trailer at 7584-7619, its bytes written at 7624) and syncs it at
    `seg.sync_all` (29004). Modelled: TW then TS on the seal thread.
  * A WAL commit is written at its commit (28495-28496) and synced only by a
    due `sync_wal`, which syncs WAL files alone (27055-27081). The counter
    `unsynced` is shared between run and WAL commits (28799, 28498).
  * A run sync that fails leaves `committed` advanced (28797-28798) and a
    retried commit returns Ok at 28691-28693 without a sync. Modelled by
    FAIL (Always only, optional).

Variants of the binding (one WAL batch at the close, before any later WAL
commit):
  NONE       no binding (design decision 1(a), the plain store today)
  LEAD       carries s = the run's count at its last completed sync; refuse
             when the run comes back short of s
  C_CLONE    lifecycle reader's variant C: (s, k) at the close, END at the
             binding when s <= r < k; the first due sync_wal after the close
             fdatasyncs a clone of the run's file and appends (k, k) before
             the WAL's own fdatasync
  C_NOCHAIN  (s, k) and END only, no sync order: the model reader's FULL as
             it stands for the run -> WAL boundary
  C_TAIL     C_CLONE, with the writer writing the held tail to the file at
             the close (a write, no sync) before the binding
  CLOSESYNC  today reader's LOOSE order: at a close with s < k the WAL is
             synced, then the run by `d.w.sync()` (flush + fdatasync), then
             the binding (k, k)
  *_RETRY    the same with the failed-sync retry fixed (a retry re-syncs)

Judged: at every node a process crash (each file keeps what was written)
and every power loss (each file any count between synced and written),
with and without one changed byte in the run's file (recovers to the commit
before it). NEVER: not a prefix of the history. LOSTDUR: a prefix shorter
than what a completed sync had promised (a due sync promises every commit
before it, the loss bound of `SyncPolicy::EveryN`, db.rs:166-171). FALSEREF:
a refusal with no damage.
"""
import sys, itertools, time

INF = 10**9


def run(variant, n, K, M, allow_fail):
    retry_fix = variant.endswith('_RETRY')
    base = variant.replace('_RETRY', '')
    res = {}
    wit = {}
    seen = set()

    def note(kind, mode, path):
        res[(kind, mode)] = res.get((kind, mode), 0) + 1
        wit.setdefault((kind, mode), path)

    # state: (k, w, sy, s, closed, T, wal, wsync, unsynced, promised, H, loose, pend_retry)
    # wal: tuple of records: ('B', s, k) | ('W', j)
    init = (0, 0, 0, 0, False, 'none', (), 0, 0, 0, (), False, False)

    def open_store(r, walp, damaged_run):
        # returns ('REF',) or ('OK', r_used, list of W ids applied)
        bs = [x for x in walp if x[0] == 'B']
        applied = []
        if base == 'NONE' or not bs:
            return ('OK', r, [x[1] for x in walp if x[0] == 'W'])
        smax = max(b[1] for b in bs)
        kk = bs[0][2]
        if r < smax:
            return ('REF',)
        if base == 'LEAD':
            return ('OK', r, [x[1] for x in walp if x[0] == 'W'])
        if r < kk:
            # END at the first binding
            i = walp.index(bs[0])
            return ('OK', r, [x[1] for x in walp[:i] if x[0] == 'W'])
        return ('OK', r, [x[1] for x in walp if x[0] == 'W'])

    def judge(st, mode, path):
        (k, w, sy, s, closed, T, wal, wsync, uns, prom, H, loose, pr) = st
        # crash views
        if mode == 'proc':
            rs = [w]
            wals = [len(wal)]
        else:
            rs = list(range(sy, w + 1))
            wals = list(range(wsync, len(wal) + 1))
        for r0 in rs:
            for dmg in [None] + list(range(1, r0 + 1)):
                r = r0 if dmg is None else dmg - 1
                for q in wals:
                    out = open_store(r, wal[:q], dmg is not None)
                    if out[0] == 'REF':
                        if dmg is None:
                            note('FALSEREF', mode, path + (('view', r0, q),))
                        continue
                    _, rr, ws = out
                    opened = [('R', i) for i in range(1, rr + 1)] + [('W', j) for j in ws]
                    if tuple(opened) != H[:len(opened)]:
                        note('NEVER' if dmg is None else 'NEVER_DMG', mode,
                             path + (('view', r0, q, dmg),))
                    elif len(opened) < prom and dmg is None:
                        note('LOSTDUR', mode, path + (('view', r0, q),))

    def sync_wal(st, path):
        (k, w, sy, s, closed, T, wal, wsync, uns, prom, H, loose, pr) = st
        if base in ('C_CLONE', 'C_TAIL'):
            if loose:
                sy = max(sy, w)          # fdatasync of a clone: what is on the file
                wal = wal + (('B', k, k),)
                loose = False
        wsync = len(wal)
        uns = 0
        prom = len(H)
        return (k, w, sy, s, closed, T, wal, wsync, uns, prom, H, loose, pr)

    def succ(st):
        (k, w, sy, s, closed, T, wal, wsync, uns, prom, H, loose, pr) = st
        out = []
        if not closed and k < K and not pr:
            k2 = k + 1
            H2 = H + (('R', k2),)
            u2 = uns + 1
            due = u2 >= n
            if due:
                out.append(('RC', (k2, k2, k2, k2, closed, T, wal, wsync, 0, len(H2), H2, loose, False)))
                if allow_fail:
                    # the sync fails: written to the page cache, not synced,
                    # answered Err; the retry (engine) answers Ok unsynced
                    out.append(('RCfail', (k2, k2, sy, s, closed, T, wal, wsync, u2, prom, H2, loose, retry_fix)))
            else:
                out.append(('RC', (k2, w, sy, s, closed, T, wal, wsync, u2, prom, H2, loose, False)))
        if pr:
            # the retried commit, fixed: it re-syncs
            out.append(('RETRY', (k, k, k, k, closed, T, wal, wsync, 0, len(H), H, loose, False)))
        if not closed and w < k:
            for w2 in range(w + 1, k + 1):
                out.append(('PF%d' % w2, (k, w2, sy, s, closed, T, wal, wsync, uns, prom, H, loose, pr)))
        if not closed and k >= 1 and not pr:
            w2, sy2, s2, wal2, wsync2, prom2, uns2 = w, sy, s, wal, wsync, prom, uns
            loose2 = False
            if base == 'C_TAIL':
                w2 = k
            if base == 'CLOSESYNC' and s < k:
                wsync2 = len(wal2)
                w2 = sy2 = s2 = k
                uns2 = 0
                prom2 = len(H)
            if base == 'NONE':
                pass
            elif base == 'LEAD':
                wal2 = wal2 + (('B', s2, s2),)
            else:
                wal2 = wal2 + (('B', s2, k),)
                loose2 = s2 < k
            out.append(('CLOSE', (k, w2, sy2, s2, True, 'handed', wal2, wsync2, uns2, prom2, H, loose2, False)))
        if closed and T == 'handed':
            out.append(('TW', (k, k, sy, s, closed, 'written', wal, wsync, uns, prom, H, loose, pr)))
        if closed and T == 'written':
            out.append(('TS', (k, k, k, s, closed, 'synced', wal, wsync, uns, prom, H, loose, pr)))
        nw = sum(1 for x in H if x[0] == 'W')
        if closed and nw < M:
            j = nw + 1
            H2 = H + (('W', j),)
            wal2 = wal + (('W', j),)
            u2 = uns + 1
            st2 = (k, w, sy, s, closed, T, wal2, wsync, u2, prom, H2, loose, pr)
            if u2 >= n:
                st2 = sync_wal(st2, None)
            out.append(('WC', st2))
        return out

    stack = [(init, ())]
    nodes = 0
    while stack:
        st, path = stack.pop()
        if st in seen:
            continue
        seen.add(st)
        nodes += 1
        judge(st, 'proc', path)
        judge(st, 'power', path)
        for name, s2 in succ(st):
            stack.append((s2, path + (name,)))
    return nodes, res, wit


def main():
    variants = sys.argv[1].split(',')
    pols = sys.argv[2].split(',')
    K = int(sys.argv[3]); M = int(sys.argv[4])
    fail = len(sys.argv) > 5 and sys.argv[5] == 'fail'
    t0 = time.process_time()
    for v in variants:
        for p in pols:
            n = {'always': 1, 'every2': 2, 'every3': 3, 'never': INF}[p]
            if fail and n != 1:
                continue
            nodes, res, wit = run(v, n, K, M, fail)
            keys = sorted(res)
            summ = ', '.join('%s/%s=%d' % (a, b, res[(a, b)]) for a, b in keys) or 'clean'
            print('%-14s %-7s K=%d M=%d fail=%s nodes=%d: %s' % (v, p, K, M, fail, nodes, summ))
            for key in keys:
                print('    e.g. %s/%s: %s' % (key[0], key[1], ' '.join(str(x) for x in wit[key])))
    print('cpu %.2fs' % (time.process_time() - t0))


if __name__ == '__main__':
    main()
