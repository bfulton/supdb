"""Bounded model of a direct run (the run's temp as its own log) beside the WAL,
and of the bindings proposed for it.  Abstract: the run temp and the WAL are
append-only logs whose recovery takes a prefix (the walk stops at the first
unit that does not verify), so a power loss leaves, per file, any prefix
between what was synced and what was written.

History: run commits r0..r(k-1), the close, WAL commits w0..w(m-1).
Background after the close: SF (the seal thread's fsync of the run's file),
MAN (the durable manifest naming R, covered := e_R), RET (temp unlinked),
MERGE (R's piece folded away: id R no longer live).  PREV is the landing of
the seal of the table frozen just before the run opened (covered := s_open);
it lands before MAN (queue order).

Variants:
  none : no binding (plain store today; Pagefile without decision 1(b)).
  A    : the lead's theory literally: the next WAL record after the close
         carries c_synced; a live binding with the temp missing or short
         refuses; nothing makes the binding moot.
  Ae   : A, plus the binding carries e_R and is moot when covered >= e_R;
         but the close consumes no sequence, so e_R == s_open.
  B0   : the binding framed at the close before end_seq is read (e_R > s_b),
         carrying (R, e_R, c_synced); moot when covered >= e_R.
  B    : B0, plus a settle frame written by an open that recovers R's temp.
  C    : B, plus c_committed in the binding; synced <= rec < committed ends
         the log at the binding; the first sync by policy after the close
         syncs the run's file first and writes a second binding (k, k).

Usage: model.py VARIANT [POLICY ...]
"""
import itertools
import sys
from collections import Counter

POL = {'always': 1, 'every2': 2, 'every3': 3, 'never': 10**9}


def interleavings(k, m):
    """Event lists: run commits, PREV somewhere before MAN, close, then the
    WAL commits interleaved with the background."""
    out = set()
    for bgtail in (['RET', 'MERGE'], ['MERGE', 'RET']):
        for prev_at in range(k + 3):  # 0..k: before close among run commits; k+1, k+2: in bg
            pre = [('R', i) for i in range(k)]
            bg = ['SF', 'MAN'] + bgtail
            if prev_at <= k:
                pre = pre[:prev_at] + [('PREV',)] + pre[prev_at:]
            else:
                bg = bg[:prev_at - k - 1] + ['PREV'] + bg[prev_at - k - 1:]
            for wpos in itertools.combinations(range(len(bg) + m), m):
                seq, bi, wi = [], 0, 0
                for i in range(len(bg) + m):
                    if i in wpos:
                        seq.append(('W', wi))
                        wi += 1
                    else:
                        seq.append((bg[bi],))
                        bi += 1
                out.add(tuple(pre + [('CLOSE',)] + seq))
    return sorted(out)


def simulate(events, variant, n, k):
    s = dict(tw=0, ts=0, u=0, wal=[], ws=0, closed=False, cs=0, cc=0,
             sf=False, man=False, ret=False, merge=False, prev=False,
             chained=False, wsynced_ws=[])
    for ev in events:
        kind = ev[0]
        if kind == 'R':
            s['tw'] += 1
            s['u'] += 1
            if s['u'] >= n:
                s['ts'] = s['tw']
                s['u'] = 0
        elif kind == 'PREV':
            s['prev'] = True
        elif kind == 'CLOSE':
            s['closed'] = True
            s['cs'], s['cc'] = s['ts'], s['tw']
            if variant in ('B0', 'B', 'C'):
                s['wal'].append(('BIND', s['cs'], s['cc']))
        elif kind == 'W':
            if variant in ('A', 'Ae') and ev[1] == 0:
                s['wal'].append(('W', ev[1], ('BIND', s['cs'], s['cs'])))
            else:
                s['wal'].append(('W', ev[1]))
            s['u'] += 1
            if s['u'] >= n:
                if variant == 'C' and not s['chained'] and s['cs'] < s['cc'] and not s['man']:
                    if not s['sf']:
                        s['ts'] = s['tw']
                    s['wal'].append(('BIND2', s['cc'], s['cc']))
                    s['chained'] = True
                s['ws'] = len(s['wal'])
                s['u'] = 0
        elif kind == 'SF':
            s['sf'] = True
            s['ts'] = s['tw']
        elif kind == 'MAN':
            s['man'] = True
        elif kind == 'RET':
            s['ret'] = True
        elif kind == 'MERGE':
            s['merge'] = True
    return s


def bindings_of(batch):
    if batch[0] in ('BIND', 'BIND2'):
        return [batch]
    if batch[0] == 'W' and len(batch) == 3:
        return [batch[2]]
    return []


def moot(variant, s, b):
    """Whether a binding batch is below the manifest's covered sequence."""
    if variant == 'A':
        return False
    if variant == 'Ae':
        # e_R == s_open: PREV's landing already covers it.
        return s['man'] or s['prev']
    # B0, B, C: BIND framed before end_seq; BIND2 carries e_R.
    return s['man']


def open1(variant, s, k, t, wl, damage):
    """-> ('refuse', why) or ('ok', rcount, ws_applied, recovered_from_temp,
    live_binding_c)."""
    wal = s['wal'][:wl]
    temp = s['closed'] and not s['ret'] or (not s['closed'] and s['tw'] > 0)
    rec = None
    if temp:
        rec = t
        if damage and damage[0] == 'corrupt':
            rec = min(t, damage[1] - 1)
        if damage and damage[0] == 'delete':
            temp = False
            rec = None
    named = s['man'] and not s['merge']
    live = []
    for i, b in enumerate(wal):
        for bb in bindings_of(b):
            if variant != 'none' and not moot(variant, s, bb):
                live.append((i, bb))
    last = live[-1][1] if live else None
    first_at = live[0][0] if live else None
    cut = len(wal)
    if s['man']:
        rcount = k
        from_temp = False
    else:
        rcount = rec if temp else 0
        from_temp = temp and rec > 0
    if last is not None and not named:
        csyn, ccom = last[1], last[2]
        if not temp:
            if csyn > 0:
                return ('refuse', 'missing')
            # A temp whose directory entry a power loss took reads as empty.
            if variant == 'C' and ccom > 0:
                cut = first_at + 1
        else:
            if rec < csyn:
                return ('refuse', 'short')
            if variant == 'C' and rec < ccom:
                cut = first_at + 1  # the log ends at the first binding for R
    applied = [b[1] for b in wal[:cut] if b[0] == 'W']
    return ('ok', rcount, applied, from_temp, last)


def held(rcount, applied, k):
    if applied != list(range(len(applied))):
        return False
    return len(applied) == 0 or rcount == k


def main():
    variant = sys.argv[1]
    pols = sys.argv[2:] or list(POL)
    for pol in pols:
        n = POL[pol]
        st = Counter()
        for k in (1, 2, 3):
            for m in (1, 2):
                for events in interleavings(k, m):
                    for cut in range(len(events) + 1):
                        evs = events[:cut]
                        s = simulate(evs, variant, n, k)
                        # synced W commits, for the "acknowledged durable" check
                        synced_ws = [b[1] for b in s['wal'][:s['ws']] if b[0] == 'W']
                        for t in range(s['ts'], s['tw'] + 1):
                            for wl in range(s['ws'], len(s['wal']) + 1):
                                temp_there = (s['closed'] and not s['ret']) or (not s['closed'] and s['tw'] > 0)
                                damages = [None]
                                if temp_there and not s['man']:
                                    damages += [('corrupt', d) for d in range(1, s['tw'] + 1)]
                                    damages += [('delete',)]
                                for dmg in damages:
                                    r = open1(variant, s, k if s['closed'] else s['tw'], t, wl, dmg)
                                    kk = k if s['closed'] else s['tw']
                                    tag = 'dmg' if dmg else 'clean'
                                    if r[0] == 'refuse':
                                        st[tag + '_refused_' + r[1]] += 1
                                        continue
                                    _, rc, ap, from_temp, last = r
                                    # a damage that removed nothing is no damage
                                    real = dmg is not None and ((dmg[0] == 'delete' and t > 0) or (dmg[0] == 'corrupt' and dmg[1] <= t))
                                    tag = 'dmg' if real else 'clean'
                                    if held(rc, ap, kk):
                                        st[tag + '_held'] += 1
                                    else:
                                        st[tag + '_NOT_held'] += 1
                                        if tag == 'clean' and len(st) < 10**9:
                                            ex = (k, m, evs, t, wl)
                                            st.setdefault('ex_clean_not_held', ex)
                                        if tag == 'dmg':
                                            st.setdefault('ex_dmg_not_held', (k, m, evs, t, wl, dmg))
                                    lost = [w for w in synced_ws if w not in ap]
                                    if lost and not real:
                                        st[tag + '_lost_synced_W'] += 1
                                    # second open: R recovered from its temp,
                                    # then merged away, then a crash.
                                    if not real and from_temp and s['closed']:
                                        for merge2 in (False, True):
                                            settle = variant in ('B', 'C')
                                            live2 = (last is not None) and not settle
                                            named2 = not merge2
                                            if live2 and last[1] > 0 and not named2:
                                                st['clean_open2_refused_missing'] += 1
                                            else:
                                                st['clean_open2_ok'] += 1
        print(variant, pol, {x: st[x] for x in sorted(st) if not x.startswith('ex_')})
        for x in ('ex_clean_not_held', 'ex_dmg_not_held'):
            if x in st:
                print('   ', x, st[x])


if __name__ == '__main__':
    main()
