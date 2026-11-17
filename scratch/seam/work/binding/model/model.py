#!/usr/bin/env python3
"""Bounded model of a store whose ordered run is its own log beside the WAL
(Pagefile, decision 1), and of bindings of the run into the WAL.

Three kinds of file, as the engine has them (src/db.rs at 3df85d22):
  * a run file per direct run (`direct-R.tmp`), a log of commits, one marker
    a commit (commit_direct 28690-28839); after its finish on the seal
    thread it is the run's segment too (hard link, 28989; sync_all 29004);
  * the WAL, files of batches with sequences (one batch = one sequence
    here), replayed from the manifest's covered sequence with the
    continuity rule and the loose-boundary end (4609, 4651-4673,
    27880-27909);
  * the manifest, slots written whole and synced (atomic here), the newest
    valid one the store (11890-11913); covered = max end over landed seals
    (37770); a run's segment is named at end_seq = wal.seq at its close
    (28919), a memtable seal's at wal.seq at its freeze (29186).

Macro operations (the depth bound counts these):
  RC  run commit; opens a run when the memtable is empty (direct_can_open
      28327-28329); under Always syncs the run's file (28800-28807).
  WC  WAL commit; closes an open run first (leave_direct 28354 ->
      close_direct -> close_run); under Always syncs the WAL files.
  SE  seal: closes an open run (seal() 29100-29105), else freezes a
      non-empty memtable and hands its seal (end = wal.seq).
  SY  buffered only: Db::sync idealised as "sync every file written", in
      two steps (a power loss can fall between); the commits before it are
      promised durable when it completes.
  SF  the oldest unfinished seal's finish and fsync (for a run: every
      committed record durable, the file finished; 29004).
  MAN the oldest finished seal's durable landing: a slot written and synced.
  RT  retire behind the landed slots: the run's temp name (37925-37927) and
      the WAL files the seal took at its hand (closed_wals, 29021).
  RO  rotate the WAL (only right after a WAL commit, 28513); the new file is
      LOOSE when the closed one holds unsynced batches.
  MG  merge every named segment into one under a new id (folds a run's
      piece away: its id is no longer named; covered unchanged).
  CR  process crash and a recovering open (27717-27770: a temp the manifest
      does not name is rebuilt as a piece named at the covered sequence,
      covered unchanged), after which the history goes on.

End states, at every node: a process crash (every write kept) and every
power-loss landing (each file keeps its synced prefix and any prefix of its
unsynced writes beyond it, independently; a run file never synced may be
absent).  Damage, one at a time, on top: a changed byte in a run file, a
WAL file; a file deleted; a file replaced by an older copy of itself; the
manifest replaced by an older copy of itself (which covers the newest slot
changed or deleted).

Run-file damage semantics (--run-sem): `design` is design.md section 4 for a
Log-class file: a byte below the last synced CKPT's synced home poisons the
store at its read (the open reads the run to rebuild it, so the open is
refused); in the last synced span recovery falls back to an older synced
CKPT (every count from the sync before the last up to the damage is
offered); in the unsynced span it is a torn tail; a finished file is a
trusted Once file and damage poisons.  `plain` is recover_direct
(6948-7004): the walk stops at the damage.  WAL damage is today's sealed
records (vault.rs:1824-1857): the walk stops at the damaged record.

Usage: model.py VARIANT POLICY DEPTH [--ops OPS] [--run-sem design|plain]
                [--pl-damage-depth N] [--examples N]
POLICY: always | bufWR | bufRW  (bufWR syncs the WAL first, bufRW the runs)
"""
import argparse
import itertools
import sys
import time
from collections import Counter

INF = float('inf')

BASE = dict(l1=None, cnt='synced', settle=False, end=False, l2=False,
            l3=False, chain=False, close_sync=False, bind_before_sync=False,
            missing_ok=False, closed_ignored=False, by_id=True,
            cr_link=False)


def V(**kw):
    d = dict(BASE)
    d.update(kw)
    return d


FULLD = dict(l1='own', settle='pub', end=True, l2=True, l3=True, cr_link=True)
L1SD = dict(l1='own', settle='void')
VARIANTS = {
    # no binding: plain parity, decision 1(a)
    'NONE': V(),
    # the lead's theory, literally: the next WAL record after the close
    # carries (R, synced); a run that comes back short of it, or missing,
    # is refused.  "Comes back" read charitably: a segment named by id R,
    # or the temp's recovered count.
    'LEAD': V(l1='ride'),
    # the same, "comes back" read as the temp only (no segment exemption)
    'LEADT': V(l1='ride', by_id=False),
    # the binding as its own batch framed before end_seq is read, so a
    # manifest covering the run covers it (replay filter s >= from)
    'L1': V(l1='own'),
    # L1 plus a settle frame an open writes for every run it rebuilds,
    # voiding the run's binding (the lifecycle reader's B)
    'L1S': V(**L1SD),
    # L1 plus, for every run an open rebuilds, a binding moot once the
    # chosen manifest is at least as new as the slot that named the piece
    'L1P': V(l1='own', settle='pub'),
    # L1S carrying the committed count too: s <= r < k ends the log there
    'L1SK': V(l1='own', settle='pub', end=True),
    # every boundary between logs linked: L1 (run -> WAL), L2 (run -> next
    # run, in its first marker), L3 (WAL -> run, in its first marker)
    'FULL': V(**FULLD),
    # FULL with the void settle instead
    'FULLV': V(l1='own', settle='void', end=True, l2=True, l3=True, cr_link=True),
    # FULL without the reopen's WAL floor in its slot
    'FULLN': V(l1='own', settle='pub', end=True, l2=True, l3=True),
    # FULL, and the sync after a close appends a binding of (k, k)
    'FULLC': V(chain=True, **FULLD),
    # the other reading of the theory: the close syncs the run's file, and
    # the binding then carries the committed count
    'LEADK': V(close_sync=True, l1='own', settle='pub'),
    # mutants
    'M1': V(cnt='committed', **FULLD),          # counts unsynced commits
    'M1L': V(cnt='committed', l1='own', settle='pub'),
    'M2': V(close_sync=True, bind_before_sync=True, l1='own', settle='pub'),  # written before the close's sync
    'M3': V(missing_ok=True, **FULLD),          # a missing run file passes
    'M3L': V(missing_ok=True, l1='own', settle='pub'),
    'M4': V(closed_ignored=True, **FULLD),      # a binding in a closed WAL file is ignored
    'M4L': V(closed_ignored=True, l1='own', settle='pub'),
}


# ---------------------------------------------------------------- state

def newwal(fid, kind):
    return dict(id=fid, recs=[], synced=0, kind=kind, closed=False,
                hist=[(0, kind)])


def init():
    return dict(H=[], wseq=0, wf=[newwal(0, 'C')], closedp=[], mem=[],
                run=None, rf={}, seals=[], segs={},
                mh=[(0, 0, frozenset(), 0)],    # the slot written at birth
                nid=1, floor=0, last=None, pend=[], forced=[], lastop=None,
                ops=[], lus=-1, u2g={}, nwal=1)


def clone(s):
    t = dict(s)
    t['H'] = list(s['H'])
    t['wf'] = [dict(f, recs=list(f['recs']), hist=list(f['hist']))
               for f in s['wf']]
    t['closedp'] = list(s['closedp'])
    t['mem'] = list(s['mem'])
    t['rf'] = {k: dict(v, recs=list(v['recs']), spts=list(v['spts']))
               for k, v in s['rf'].items()}
    t['seals'] = [dict(x) for x in s['seals']]
    t['segs'] = dict(s['segs'])
    t['mh'] = list(s['mh'])
    t['pend'] = list(s['pend'])
    t['forced'] = list(s['forced'])
    t['ops'] = list(s['ops'])
    t['u2g'] = dict(s['u2g'])
    return t


def cov_now(s):
    return s['mh'][-1][1]


def live(s):
    return s['wf'][-1]


def wal_append(s, rec):
    f = live(s)
    f['recs'].append(rec)
    f['hist'].append((len(f['recs']), f['kind']))


def sync_wal(s):
    # sync_wal (27055-27081): the loose files, the chain, the live file.
    for f in s['wf']:
        f['synced'] = len(f['recs'])
    f = live(s)
    if f['kind'] != 'C':
        f['kind'] = 'C'
        f['hist'].append((len(f['recs']), 'C'))


def sync_run(s, R):
    f = s['rf'][R]
    n = len(f['recs'])
    if f['synced'] < n:
        f['synced'] = n
        f['spts'].append(n)


# ------------------------------------------------------------ operations

def close_run(s, var):
    R = s['run']
    f = s['rf'][R]
    k = len(f['recs'])
    if var['close_sync'] and not var['bind_before_sync']:
        sync_run(s, R)
    if var['cnt'] == 'committed' or var['bind_before_sync']:
        sc = k
    else:
        sc = f['synced']
    f['s_close'] = sc
    if var['l1'] == 'own':
        wal_append(s, ('B', s['wseq'], R, ('seq', s['wseq'] + 1), sc, k))
        s['wseq'] += 1
    e = s['wseq']
    if var['l1'] == 'ride':
        s['pend'].append((R, sc))
    f['e'] = e
    f['k'] = k
    f['closed'] = True
    s['seals'].append(dict(kind='R', id=R, end=e, g=frozenset(f['recs']),
                           sf=False, man=False, rt=False,
                           wals=tuple(s['closedp'])))
    s['closedp'] = []
    s['run'] = None
    if var['close_sync'] and var['bind_before_sync']:
        s['forced'].insert(0, ('CSY', R))


def l3_payload(s):
    P = s['lus'] + 1
    cov = cov_now(s)
    s3 = P
    # the first batch of any kind below P that is neither synced nor
    # covered: replay's reach stops at the first record lost, whatever its
    # kind, so the synced position is of every record
    for f in s['wf']:
        for i, rec in enumerate(f['recs']):
            if rec[1] < P and rec[1] >= cov and i >= f['synced']:
                s3 = min(s3, rec[1])
    return (P, s3)


def op_RC(s, var, pol):
    if s['run'] is None:
        R = s['nid']
        s['nid'] += 1
        f = dict(recs=[], synced=0, spts=[], fin=False, temp=True, alive=True,
                 e=None, k=None, closed=False, l2=None, l3=None,
                 start=len(s['H']), s_close=None, chained=False, pub=None)
        if var['l2'] and s['last'] is not None and s['last'][0] == 'R':
            A = s['last'][1]
            fa = s['rf'][A]
            sa = fa['synced'] if (fa['alive'] and not fa['fin']) else fa['k']
            if var['cnt'] == 'committed':
                sa = fa['k']
            mc = ('pub', fa['pub']) if fa['pub'] is not None else ('seq', fa['e'])
            f['l2'] = (A, mc, sa, fa['k'])
        if var['l3']:
            f['l3'] = l3_payload(s)
        s['rf'][R] = f
        s['run'] = R
    R = s['run']
    g = len(s['H'])
    s['H'].append(('R', R))
    s['rf'][R]['recs'].append(g)
    if pol == 'always':
        sync_run(s, R)
        s['floor'] = len(s['H'])
    s['last'] = ('R', R)


def wc_write(s, var, pol):
    g = len(s['H'])
    s['H'].append(('W', s['wseq']))
    wal_append(s, ('U', s['wseq'], g, tuple(s['pend'])))
    s['pend'] = []
    s['u2g'][s['wseq']] = g
    s['lus'] = s['wseq']
    s['wseq'] += 1
    s['mem'].append(g)
    if pol == 'always':
        sync_wal(s)
        s['floor'] = len(s['H'])
    s['last'] = ('W',)


def op_WC(s, var, pol):
    if s['run'] is not None:
        close_run(s, var)
        if s['forced']:
            s['forced'].append(('WCW',))
            return
    wc_write(s, var, pol)


def op_SE(s, var, pol):
    if s['run'] is not None:
        close_run(s, var)
        return
    sid = s['nid']
    s['nid'] += 1
    s['seals'].append(dict(kind='W', id=sid, end=s['wseq'],
                           g=frozenset(s['mem']), sf=False, man=False,
                           rt=False, wals=tuple(s['closedp'])))
    s['closedp'] = []
    s['mem'] = []


def sync_runs(s):
    for R, f in s['rf'].items():
        if f['alive'] and not f['fin']:
            sync_run(s, R)


def chain_binds(s):
    cov = cov_now(s)
    for R, f in s['rf'].items():
        if (f['closed'] and f['alive'] and f['temp'] and not f['chained']
                and f['e'] > cov and f['s_close'] < f['k']):
            wal_append(s, ('B', s['wseq'], R, ('seq', f['e']), f['k'], f['k']))
            s['wseq'] += 1
            f['chained'] = True


def op_SY(s, var, pol):
    if var['chain'] or pol == 'bufRW':
        sync_runs(s)
    else:
        sync_wal(s)
    s['forced'].insert(0, ('SY2',))


def op_SY2(s, var, pol):
    if var['chain'] or pol == 'bufRW':
        if var['chain']:
            chain_binds(s)
        sync_wal(s)
    else:
        sync_runs(s)
    s['floor'] = len(s['H'])


def op_SF(s, var, pol):
    for x in s['seals']:
        if not x['sf']:
            x['sf'] = True
            if x['kind'] == 'R':
                sync_run(s, x['id'])
                s['rf'][x['id']]['fin'] = True
            return


def op_MAN(s, var, pol):
    for x in s['seals']:
        if not x['man']:
            assert x['sf']
            x['man'] = True
            pub, cov, names, wf = s['mh'][-1]
            s['segs'][x['id']] = (x['g'], x['end'])
            c2 = max(cov, x['end'])
            s['mh'].append((pub + 1, c2, names | {x['id']}, wf if wf > c2 else 0))
            if x['kind'] == 'R':
                s['rf'][x['id']]['pub'] = pub + 1
            return


def op_RT(s, var, pol):
    gone = set()
    for x in s['seals']:
        if x['man'] and not x['rt']:
            x['rt'] = True
            if x['kind'] == 'R':
                s['rf'][x['id']]['temp'] = False
            gone.update(x['wals'])
    if gone:
        assert all(f['closed'] for f in s['wf'] if f['id'] in gone)
        s['wf'] = [f for f in s['wf'] if f['id'] not in gone]
    s['seals'] = [x for x in s['seals'] if not x['rt']]


def op_RO(s, var, pol):
    f = live(s)
    f['closed'] = True
    s['closedp'].append(f['id'])
    kind = 'L' if f['synced'] < len(f['recs']) else 'C'
    s['wf'].append(newwal(s['nwal'], kind))
    s['nwal'] += 1


def op_MG(s, var, pol):
    pub, cov, names, wf = s['mh'][-1]
    M = s['nid']
    s['nid'] += 1
    g = frozenset().union(*(s['segs'][i][0] for i in names))
    end = max(s['segs'][i][1] for i in names)
    for i in names:
        del s['segs'][i]
        if i in s['rf']:
            s['rf'][i]['alive'] = False
    s['segs'][M] = (g, end)
    s['mh'].append((pub + 1, cov, frozenset([M]), wf))


def enabled(s, op, pol, ops):
    if s['forced']:
        return False
    if op not in ops:
        return False
    if op == 'RC':
        return s['run'] is not None or not s['mem']
    if op == 'WC':
        return True
    if op == 'SE':
        return s['run'] is not None or bool(s['mem'])
    if op == 'SY':
        if pol == 'always':
            return False
        return (any(f['synced'] < len(f['recs']) for f in s['wf']) or
                any(f['alive'] and not f['fin'] and f['synced'] < len(f['recs'])
                    for f in s['rf'].values()))
    if op == 'SF':
        return any(not x['sf'] for x in s['seals'])
    if op == 'MAN':
        for x in s['seals']:
            if not x['man']:
                return x['sf']
        return False
    if op == 'RT':
        return any(x['man'] and not x['rt'] for x in s['seals'])
    if op == 'RO':
        return s['lastop'] == 'WC' and len(live(s)['recs']) > 0
    if op == 'MG':
        names = s['mh'][-1][2]
        return (bool(names) and not any(x['man'] and not x['rt'] for x in s['seals'])
                and s['lastop'] != 'MG')
    if op == 'CR':
        return True
    return False


OPF = dict(RC=op_RC, WC=op_WC, SE=op_SE, SY=op_SY, SF=op_SF, MAN=op_MAN,
           RT=op_RT, RO=op_RO, MG=op_MG)


def run_forced(s, var, pol):
    m = s['forced'].pop(0)
    if m[0] == 'CSY':
        sync_run(s, m[1])
    elif m[0] == 'WCW':
        wc_write(s, var, pol)
    elif m[0] == 'SY2':
        op_SY2(s, var, pol)
    elif m[0] == 'CR2':
        op_CR2(s, var, m[1])
    s['ops'].append(m[0])


# ------------------------------------------------------------- the open

def replay(wal, frm):
    """wal: [(fid, recs, kind, closed)] in id order, the files present.
    -> None for a refusal (a sequence gap), else (next, applied, binds, ended)."""
    nxt = frm
    applied = []
    binds = {}
    ended = False
    for i, (fid, recs, kind, closed) in enumerate(wal):
        loose = kind == 'L' and i > 0
        first = True
        for rec in recs:
            seq = rec[1]
            if seq < frm:
                continue
            if seq != nxt:
                if loose and first:
                    ended = True
                    break
                return None
            first = False
            nxt = seq + 1
            if rec[0] == 'U':
                applied.append(rec[2])
                for (R, sc) in rec[3]:
                    binds.setdefault(R, []).append((seq, None, sc, sc, closed))
            elif rec[0] == 'B':
                binds.setdefault(rec[2], []).append((seq, rec[3], rec[4], rec[5], closed))
            elif rec[0] == 'S':
                binds[rec[2]] = []
        if ended:
            break
    return nxt, applied, binds, ended


def open_store(s, v, var):
    """-> ('REFUSE', why) | ('REFUSE_READ', why) | ('OPEN', state, info)"""
    mpub, cov, names, wfloor = v['man']
    for sid in names:
        if sid not in v['segs']:
            return ('REFUSE', 'named-segment-missing')
    if v['poison'] & names:
        return ('REFUSE_READ', 'segment')
    temps = {}
    for R, r in v['temps'].items():
        if R in names:
            continue
        if r == 'P':
            return ('REFUSE_READ', 'run-file')
        temps[R] = r
    rp = replay(v['wal'], cov)
    if rp is None:
        return ('REFUSE', 'wal-gap')
    nxt, applied, binds, ended = rp
    if wfloor and max(cov, nxt) < wfloor:
        return ('REFUSE', 'slot-wal-floor')
    rf = s['rf']
    cuts = []

    def pred(R, mc, sc, kc, why):
        if mc is not None:
            if mc[0] == 'seq' and cov >= mc[1]:
                return None
            if mc[0] == 'pub' and mpub >= mc[1]:
                return None
        if var['by_id'] and R in names:
            return None
        if R in temps:
            r = temps[R]
            if r < sc:
                return ('REFUSE', why + '-short')
            if var['end'] and r < kc:
                cuts.append(rf[R]['start'] + r)
            return None
        if var['missing_ok']:
            return None
        if sc > 0:
            return ('REFUSE', why + '-missing')
        if var['end'] and kc > 0:
            cuts.append(rf[R]['start'])
        return None

    if var['l1'] is not None:
        for R, lst in binds.items():
            if var['closed_ignored']:
                lst = [b for b in lst if not b[4]]
            if not lst:
                continue
            seq, mc, sc, kc, closed = lst[-1]
            res = pred(R, mc, sc, kc, 'L1')
            if res:
                return res
    for R, r in temps.items():
        if r < 1:
            continue
        f = rf[R]
        if var['l2'] and f['l2'] is not None:
            A, mcA, sA, kA = f['l2']
            res = pred(A, mcA, sA, kA, 'L2')
            if res:
                return res
        if var['l3'] and f['l3'] is not None:
            P, s3 = f['l3']
            if cov >= P or nxt >= P:
                pass
            elif nxt < s3:
                return ('REFUSE', 'L3-short')
            else:
                gs = [s['u2g'][q] for q in s['u2g'] if q >= nxt and q < P]
                cuts.append(min(gs))
    S = set()
    for sid in names:
        S |= v['segs'][sid][0]
    for R, r in temps.items():
        S.update(rf[R]['recs'][:r])
    S.update(applied)
    if cuts:
        c = min(cuts)
        S = {g for g in S if g < c}
    return ('OPEN', S, (nxt, applied))


def judge(s, out):
    if out[0] != 'OPEN':
        return out[0]
    S = out[1]
    j = len(S)
    if j and max(S) != j - 1:
        return 'NEVER'
    if j < s['floor']:
        return 'LOSTDUR'
    if j < len(s['H']):
        return 'ROLLBACK'
    return 'WHOLE'


# ------------------------------------------------------- views of the disk

def crash_view(s):
    temps = {R: len(f['recs']) for R, f in s['rf'].items() if f['temp'] and f['alive']}
    wal = [(f['id'], f['recs'], f['kind'], f['closed']) for f in s['wf']]
    return dict(man=s['mh'][-1], segs=s['segs'], poison=frozenset(), temps=temps, wal=wal)


def pl_views(s):
    tR = [(R, f) for R, f in s['rf'].items() if f['temp'] and f['alive']]
    tch = []
    for R, f in tR:
        c = list(range(f['synced'], len(f['recs']) + 1))
        if f['synced'] == 0:
            c.append('A')
        tch.append(c)
    wch = [list(range(f['synced'], len(f['recs']) + 1)) for f in s['wf']]
    for tc in itertools.product(*tch):
        temps = {}
        for (R, f), r in zip(tR, tc):
            if r != 'A':
                temps[R] = r
        for wc in itertools.product(*wch):
            wal = [(f['id'], f['recs'][:c], f['kind'], f['closed'])
                   for f, c in zip(s['wf'], wc)]
            yield (tc, wc), dict(man=s['mh'][-1], segs=s['segs'], poison=frozenset(),
                                 temps=temps, wal=wal)


def damages(s, v):
    out = []
    for R, r in v['temps'].items():
        for d in range(1, r + 1):
            out.append(('rbyte', R, d))
        out.append(('rdel', R))
        for c in range(0, r):
            out.append(('rold', R, c))
    for (fid, recs, kind, closed) in v['wal']:
        for j in range(len(recs)):
            out.append(('wbyte', fid, j))
        out.append(('wdel', fid))
        for c in range(len(recs)):
            out.append(('wold', fid, c))
    for i in range(len(s['mh']) - 1):
        out.append(('mold', i))
    return out


def apply_damage(s, v, dmg, run_sem):
    """-> list of views (a fallback offers several)."""
    k = dmg[0]
    if k in ('rbyte', 'rdel', 'rold'):
        R = dmg[1]
        f = s['rf'][R]
        res = []
        if k == 'rdel':
            t = dict(v['temps'])
            del t[R]
            return [dict(v, temps=t)]
        if k == 'rold':
            t = dict(v['temps'])
            t[R] = dmg[2]
            return [dict(v, temps=t)]
        d = dmg[2]
        if f['fin']:
            t = dict(v['temps'])
            t[R] = 'P'
            return [dict(v, temps=t, poison=frozenset([R]))]
        if run_sem == 'plain':
            opts = [d - 1]
        else:
            synced = f['synced']
            sp = sorted(set(f['spts']))
            prev = sp[-2] if len(sp) >= 2 else 0
            if d <= prev:
                opts = ['P']
            elif d <= synced:
                opts = list(range(prev, d))
            else:
                opts = [d - 1]
        for r in opts:
            t = dict(v['temps'])
            t[R] = r
            res.append(dict(v, temps=t))
        return res
    if k in ('wbyte', 'wdel', 'wold'):
        fid = dmg[1]
        if k == 'wdel':
            return [dict(v, wal=[w for w in v['wal'] if w[0] != fid])]
        if k == 'wbyte':
            return [dict(v, wal=[(w[0], w[1][:dmg[2]], w[2], w[3]) if w[0] == fid else w
                                 for w in v['wal']])]
        c = dmg[2]
        f = [x for x in s['wf'] if x['id'] == fid][0]
        kinds = sorted({kd for (n, kd) in f['hist'] if n == c}) or [f['kind']]
        return [dict(v, wal=[(w[0], w[1][:c], kd, w[3]) if w[0] == fid else w
                             for w in v['wal']]) for kd in kinds]
    if k == 'mold':
        return [dict(v, man=s['mh'][dmg[1]])]
    raise ValueError(dmg)


# -------------------------------------------------------- the recovering open

def op_CR(s, var, pol, rec):
    v = crash_view(s)
    out = open_store(s, v, var)
    j = judge(s, out)
    if j != 'WHOLE':
        rec('P1', 'crash-at-CR', j, s, None, None, out)
        return False
    nxt, applied = out[2]
    pub, cov, names, wfl = s['mh'][-1]
    names = set(names)
    rebuilt = []
    if var['cr_link']:
        # the replayed WAL synced before a piece is named over it
        sync_wal(s)
    for R, f in sorted(s['rf'].items()):
        if not (f['temp'] and f['alive']):
            continue
        f['temp'] = False
        if R in names:
            continue
        r = len(f['recs'])
        f['alive'] = False      # the temp is removed; the piece is a new file
        if r == 0:
            continue
        s['segs'][R] = (frozenset(f['recs'][:r]), cov)
        names.add(R)
        pub += 1
        if var['cr_link'] and f['l3'] is not None and f['l3'][0] > cov:
            wfl = max(wfl, f['l3'][0])
        s['mh'].append((pub, cov, frozenset(names), wfl))
        f['pub'] = pub
        f['k'] = r
        rebuilt.append((R, r, pub))
    if s['run'] is not None:
        s['run'] = None
    s['seals'] = []
    for f in s['wf'][:-1]:
        f['closed'] = True
    s['closedp'] = [f['id'] for f in s['wf'][:-1]]
    s['mem'] = list(applied)
    s['pend'] = []
    assert nxt == s['wseq'], (nxt, s['wseq'])
    if var['settle'] and rebuilt:
        # after open_append: a step of its own, so that a power loss can
        # fall between the slots above and this sync
        s['forced'].append(('CR2', tuple(rebuilt)))
    return True


def op_CR2(s, var, rebuilt):
    for R, r, p in rebuilt:
        if var['settle'] == 'void':
            wal_append(s, ('S', s['wseq'], R))
        else:
            wal_append(s, ('B', s['wseq'], R, ('pub', p), r, r))
        s['wseq'] += 1
    sync_wal(s)


# ---------------------------------------------------------------- driver

def boundary(s, S):
    """The boundary a never-held state breaks: the log of its first missing
    commit -> the log of a later commit it holds."""
    j = 0
    while j in S:
        j += 1
    x = max(S)
    a, b = s['H'][j], s['H'][x]
    ka = 'run' if a[0] == 'R' else 'wal'
    kb = 'run' if b[0] == 'R' else 'wal'
    if ka == kb == 'run':
        return 'run->same-run' if a[1] == b[1] else 'run->run'
    return ka + '->' + kb


class Stats:
    def __init__(self, nex):
        self.c = Counter()
        self.shape = Counter()
        self.shape_ex = {}
        self.ex = {}
        self.nex = nex

    def rec(self, prop, mode, verdict, s, land, dmg, out):
        key = (prop, mode, verdict)
        self.c[key] += 1
        if verdict == 'NEVER':
            sk = (prop, mode, boundary(s, out[1]), dmg[0] if dmg else '-')
            self.shape[sk] += 1
            if sk not in self.shape_ex:
                self.shape_ex[sk] = (' '.join(s['ops']), tuple(s['H']), land, dmg, sorted(out[1]))
        bad = (verdict in ('NEVER', 'LOSTDUR', 'REFUSE', 'REFUSE_READ', 'ROLLBACK')
               if prop == 'P1' else verdict == 'NEVER')
        if prop == 'P1' and mode == 'pl' and verdict == 'ROLLBACK':
            bad = False
        if prop == 'P2' and verdict == 'LOSTDUR':
            key2 = ('P2info', mode, 'LOSTDUR')
            if len(self.ex.get(key2, [])) < 1:
                self.ex.setdefault(key2, []).append((' '.join(s['ops']), land, dmg, out[:2] if out else None))
        if bad:
            lst = self.ex.setdefault(key, [])
            if len(lst) < self.nex:
                o = out
                if o and o[0] == 'OPEN':
                    o = ('OPEN', sorted(o[1]))
                elif o:
                    o = o[:2]
                lst.append((' '.join(s['ops']), tuple(s['H']), land, dmg, o))


def evaluate(s, var, pol, args, st, depth):
    # P1: process crash
    v = crash_view(s)
    out = open_store(s, v, var)
    st.rec('P1', 'crash', judge(s, out), s, None, None, out)
    # P1: power loss
    for land, vv in pl_views(s):
        out = open_store(s, vv, var)
        st.rec('P1', 'pl', judge(s, out), s, land, None, out)
    # P2: damage after a process crash
    for dmg in damages(s, v):
        for vd in apply_damage(s, v, dmg, args.run_sem):
            out = open_store(s, vd, var)
            st.rec('P2', 'crash', judge(s, out), s, None, dmg, out)
    # P2: damage after a power loss
    if depth <= args.pl_damage_depth:
        for land, vv in pl_views(s):
            for dmg in damages(s, vv):
                for vd in apply_damage(s, vv, dmg, args.run_sem):
                    out = open_store(s, vd, var)
                    st.rec('P2', 'pl', judge(s, out), s, land, dmg, out)


def dfs(s, depth, var, pol, args, st, ops, nodes):
    nodes[0] += 1
    evaluate(s, var, pol, args, st, depth)
    if s['forced']:
        t = clone(s)
        run_forced(t, var, pol)
        dfs(t, depth, var, pol, args, st, ops, nodes)
        return
    if depth >= args.depth:
        return
    for op in ops:
        if not enabled(s, op, pol, ops):
            continue
        t = clone(s)
        t['ops'].append(op)
        if op == 'CR':
            ok = op_CR(t, var, pol, st.rec)
            t['lastop'] = 'CR'
            if not ok:
                continue
        else:
            OPF[op](t, var, pol)
            t['lastop'] = op
        dfs(t, depth + 1, var, pol, args, st, ops, nodes)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('variant')
    ap.add_argument('policy', choices=['always', 'bufWR', 'bufRW'])
    ap.add_argument('depth', type=int)
    ap.add_argument('--ops', default='RC,WC,SE,SY,SF,MAN,RT,RO')
    ap.add_argument('--run-sem', default='design', choices=['design', 'plain'])
    ap.add_argument('--pl-damage-depth', type=int, default=-1)
    ap.add_argument('--examples', type=int, default=2)
    ap.add_argument('--shape-examples', action='store_true')
    ap.add_argument('--first', default=None, help='only sequences starting with these ops (comma list)')
    args = ap.parse_args()
    var = VARIANTS[args.variant]
    ops = args.ops.split(',')
    st = Stats(args.examples)
    nodes = [0]
    t0 = time.process_time()
    s = init()
    if args.first:
        for op in args.first.split(','):
            assert enabled(s, op, args.policy, ops) or s['forced'], (op, s['ops'])
            while s['forced']:
                run_forced(s, var, args.policy)
            s['ops'].append(op)
            if op == 'CR':
                assert op_CR(s, var, args.policy, st.rec)
            else:
                OPF[op](s, var, args.policy)
            s['lastop'] = op
        dfs(s, len(args.first.split(',')), var, args.policy, args, st, ops, nodes)
    else:
        dfs(s, 0, var, args.policy, args, st, ops, nodes)
    cpu = time.process_time() - t0
    print('variant=%s policy=%s depth=%d ops=%s run_sem=%s pl_damage_depth=%d first=%s nodes=%d cpu=%.1fs'
          % (args.variant, args.policy, args.depth, args.ops, args.run_sem,
             args.pl_damage_depth, args.first, nodes[0], cpu))
    for key in sorted(st.c):
        print('  %-4s %-6s %-12s %d' % (key + (st.c[key],)))
    for key in sorted(st.shape):
        print('  NEVER-SHAPE %-4s %-6s %-14s %-6s %d' % (key + (st.shape[key],)))
    if args.shape_examples:
        for key in sorted(st.shape_ex):
            print('  SHAPE-EX %s: ops=[%s] H=%s land=%s dmg=%s opened=%s' % ((key,) + st.shape_ex[key]))
    for key in sorted(st.ex):
        for e in st.ex[key]:
            print('  EX %s %s %s: ops=[%s]' % (key[0], key[1], key[2], e[0]))
            print('       H=%s land=%s dmg=%s out=%s' % e[1:] if len(e) == 5 else
                  '       land=%s dmg=%s out=%s' % e[1:])


if __name__ == '__main__':
    main()
