#!/usr/bin/env python3
"""The Replace class under the engine's two-slot manifest (db.rs:11598-11600
slot by seq parity; 11695-11721 write_all_at then sync_data; 38851-38883 the
drain's publish_unsynced, its slot's fdatasync owed and superseded by the
next synced publish; 11890-11913 the newest valid slot wins).

  python3 replace.py MODE [opt=value ...] maxlen=N
  MODE  plain : a slot is one object, valid whole or torn (its CRC)
        pg    : a slot is a Pagefile Replace file: header A/B with a session
                table, NP data pages rewritten in place at fresh ctrs, a REC
                ring holding every page's tag, written in the same call
ops   'W' publish and sync it        'U' publish, its sync owed (a drain)
      'O' the owed sync              'B' publish, its sync begun (another thread)
      'E' that sync completed        'X' process crash, reopen
      'P' power loss, reopen (every state checked, the run continued from each)
checks: the newest valid slot is a publish that was written and is at least
the newest whose own sync completed; a store with an acknowledged publish
never opens with no valid slot.
"""
import sys, itertools, time
import pfm
from pfm import World, TORN, Tally

NP, RR = 2, 2
OPT = dict(lazy=True, resume_sync=False, own='table', cap=48, drop='never', idx='stable',
           sync_owed_first=False, owed_same_slot=False, settle_other=False, open_sync=False)
# drop: never      the table only grows (cap large)
#       after_sync once the layer's own sync (resume_sync) has made the recovered
#                  REC durable, sessions older than its session are dropped
#       oldest     MUTANT: a full table drops its oldest entry, synced or not
# idx:  stable     a frame names its session by an id the session keeps
#       pos        a frame names its session by its position in the table


class RepH:
    """A Pagefile Replace Handle for one slot file (one per open of the store)."""

    def __init__(s, W, ino, fid, o, st=None):
        s.W, s.ino, s.fid, s.o = W, ino, fid, o
        s.kf = ('kf', fid)
        s.cur = None
        if st and not st.get('empty'):
            s.sessions = list(st['table'])
            s.hdr_seq, s.hdr_slot = st['hdr_seq'], st['hdr_slot']
            s.seq = st['seq']
            s.ring_last = st['ring_slot']
            s.rec_key = st.get('rec_key')
        elif st and st.get('hdr'):
            s.sessions = list(st['hdr'][2])
            s.hdr_seq, s.hdr_slot = st['hdr'][0], st['hdr'][1]
            s.seq = -1
            s.ring_last = -1
        else:
            s.sessions, s.hdr_seq, s.hdr_slot, s.seq, s.ring_last = [], 0, None, -1, -1
        s.hdr_safe = s.hdr_slot is None
        s.synced_sess = set()        # sessions whose whole rewrite a completed sync covers
        s.infl = {}
        if not s.o['lazy']:
            s.ensure()

    def ensure(s):
        if s.cur is not None:
            return
        if s.o['resume_sync'] and not s.hdr_safe:
            tok = s.W.dev.sync_begin(s.ino)
            s.W.sync_end(tok)
            s.hdr_safe = True
        table = list(s.sessions)
        if s.o['drop'] == 'after_sync' and s.hdr_safe and getattr(s, 'rec_key', None):
            keys = [t[0] for t in table]
            if s.rec_key in keys:
                table = table[keys.index(s.rec_key):]
        if s.o['drop'] == 'oldest' and len(table) >= s.o['cap']:
            table = table[len(table) - s.o['cap'] + 1:]
        nid = (max([t[4] for t in s.sessions]) + 1) if s.sessions else 0
        new = (s.W.uniq('s'), 'continue', 0, s.seq + 1, nid)
        s.sessions = table + [new]
        s.cur = len(s.sessions) - 1
        s.ctr = 0
        slot = 0 if s.hdr_slot is None else s.hdr_slot ^ 1
        s.hdr_seq += 1
        img = s.W.seal(s.kf, s.W.uniq('rnd'), (s.fid, s.hdr_seq, tuple(s.sessions)),
                       kind='hdr', fid=s.fid, hdr_seq=s.hdr_seq, table=tuple(s.sessions))
        s.W.dev.write(s.ino, [(('h', slot), img)])
        s.hdr_slot = slot
        s.hdr_safe = False
        s.hdr_t = s.W.dev.t

    def write(s, k):
        s.ensure()
        key = s.sessions[s.cur][0]
        imgs = []
        for p in range(NP):
            c = s.ctr
            s.ctr += 1
            imgs.append(s.W.seal(key, ('d', p, c), (k, p), kind='data', p=p))
        s.seq += 1
        body = {'k': k, 'tags': tuple(imgs)}
        sidx = s.cur if s.o['idx'] == 'pos' else s.sessions[s.cur][4]
        rimg = s.W.seal(key, ('f', s.seq, 0), pfm.fz(body), kind='frame', sidx=sidx,
                        seq=s.seq, body=body)
        slot = (s.ring_last + 1) % RR
        s.ring_last = slot
        # one call: the pages in place, the REC beside them (sec. 4 Write, Replace)
        s.W.dev.write(s.ino, [(('d', p), imgs[p]) for p in range(NP)])
        s.W.dev.write(s.ino, [(('r', slot), rimg)])

    def sync_begin(s):
        tok = s.W.dev.sync_begin(s.ino)
        s.infl[tok] = (getattr(s, 'hdr_t', None), s.cur)
        return tok

    def sync_end(s, tok):
        s.W.sync_end(tok)
        ht, cur = s.infl.pop(tok)
        if ht is not None and ht == getattr(s, 'hdr_t', None):
            s.hdr_safe = True
        if cur is not None:
            s.synced_sess.add(s.sessions[cur][0])


def recover_rep(W, v, ino, fid, o):
    hs = pfm.headers(W, v, ino, fid)
    if not hs:
        return {'empty': True, 'k': None}
    hdr_seq, hslot, hm = max(hs, key=lambda x: x[0])
    table = list(hm['table'])
    best = None
    for slot in range(RR):
        img = v.get((ino, ('r', slot)))
        if img in (None, TORN):
            continue
        m = W.imgs[img]
        if m['kind'] != 'frame':
            continue
        if o['idx'] == 'pos':
            ent = table[m['sidx']] if m['sidx'] < len(table) else None
        else:
            ent = {t[4]: t for t in table}.get(m['sidx'])
        if ent is None or ent[0] != m['key']:
            continue
        if best is None or (m['seq'], m['sidx']) > (best[0]['seq'], best[0]['sidx']):
            best = (m, slot)
    st = {'empty': False, 'table': table, 'hdr_seq': hdr_seq, 'hdr_slot': hslot,
          'seq': best[0]['seq'] if best else -1, 'ring_slot': best[1] if best else -1,
          'rec_key': best[0]['key'] if best else None}
    if best is None:
        st['k'] = None
        st['empty'] = True
        st['hdr'] = (hdr_seq, hslot, table)
        return st
    m = best[0]
    okey = table[-1][0] if o['own'] == 'table' else m['key']
    ok = True
    for p, t in enumerate(m['body']['tags']):
        if v.get((ino, ('d', p))) != t or W.imgs[t]['key'] != okey:
            ok = False
    st['k'] = m['body']['k'] if ok else None      # failed pages read as zeros: the CRC fails
    return st


class Store:
    """The engine's slot protocol over two files."""

    def __init__(s, W, mode, o, fids, st=None):
        s.W, s.mode, s.o, s.fids = W, mode, o, fids
        s.inos = (10, 11)
        s.h = [None, None]
        s.owed = None
        s.infl = []
        if st is None:
            s.seq = 0
        else:
            s.seq = st['seq']
        if mode == 'pg':
            for i in (0, 1):
                s.h[i] = RepH(W, s.inos[i], fids[i], o, st['files'][i] if st else None)
        if st is not None and o['open_sync']:
            # FIX: an open after an unclean stop makes the slot it found newest
            # durable before the first publish overwrites the other one
            for i in (0, 1):
                tok = W.dev.sync_begin(s.inos[i])
                W.dev.sync_end(tok)

    def publish(s, k):
        i = k % 2
        if s.o['owed_same_slot'] and s.owed is not None:
            i = s.owed                       # FIX (alt): a publish supersedes an owed slot in place
        if s.mode == 'plain':
            s.W.dev.write(s.inos[i], [(('slot',), ('pub', k))])
        else:
            s.h[i].write(k)
        return i

    def sync_file(s, i):
        if s.mode == 'plain':
            tok = s.W.dev.sync_begin(s.inos[i])
            s.W.sync_end(tok)
        else:
            s.h[i].sync_end(s.h[i].sync_begin())

    def settle_other(s, orc):
        """FIX: before a slot is overwritten, the other slot's newest write is
        durable: an in-flight sync is waited for, an owed one issued."""
        while s.infl:
            i, tok, k = s.infl.pop(0)
            if s.mode == 'plain':
                s.W.sync_end(tok)
            else:
                s.h[i].sync_end(tok)
            orc['acked'] = max(orc['acked'], k)
        if s.owed is not None:
            s.sync_file(s.owed)
            orc['acked'] = max(orc['acked'], s.owed_k)
            s.owed = None

    def op(s, op, orc):
        if s.o['settle_other'] and op in 'WUB':
            s.settle_other(orc)
        if op == 'W':
            if s.o['sync_owed_first'] and s.owed is not None:   # FIX: sync the owed slot first
                s.sync_file(s.owed)
                orc['acked'] = max(orc['acked'], s.owed_k)
                s.owed = None
            s.seq += 1
            i = s.publish(s.seq)
            orc['written'].add(s.seq)
            s.sync_file(i)
            orc['acked'] = max(orc['acked'], s.seq)
            s.owed = None                    # superseded: what waited behind it retires now
        elif op == 'U':
            if s.o['sync_owed_first'] and s.owed is not None:
                s.sync_file(s.owed)
                orc['acked'] = max(orc['acked'], s.owed_k)
            s.seq += 1
            i = s.publish(s.seq)
            orc['written'].add(s.seq)
            s.owed, s.owed_k = i, s.seq
        elif op == 'O':
            if s.owed is not None:
                s.sync_file(s.owed)
                orc['acked'] = max(orc['acked'], s.owed_k)
                s.owed = None
        elif op == 'B':
            s.seq += 1
            i = s.publish(s.seq)
            orc['written'].add(s.seq)
            if s.mode == 'plain':
                tok = s.W.dev.sync_begin(s.inos[i])
            else:
                tok = s.h[i].sync_begin()
            s.infl.append((i, tok, s.seq))
        elif op == 'E':
            if s.infl:
                i, tok, k = s.infl.pop(0)
                if s.mode == 'plain':
                    s.W.sync_end(tok)
                else:
                    s.h[i].sync_end(tok)
                orc['acked'] = max(orc['acked'], k)


def read_store(W, v, mode, o, fids):
    inos = (10, 11)
    ks, files = [], []
    for i in (0, 1):
        if mode == 'plain':
            x = v.get((inos[i], ('slot',)))
            k = x[1] if isinstance(x, tuple) and x[0] == 'pub' else None
            files.append(None)
        else:
            st = recover_rep(W, v, inos[i], fids[i], o)
            k = st['k']
            files.append(st)
        ks.append(k)
    valid = [k for k in ks if k is not None]
    return (max(valid) if valid else None), files


def run(mode, o, ops, T, cap=200):
    W = World()
    fids = (W.uniq('fid'), W.uniq('fid'))
    orc = {'written': {0}, 'acked': 0}
    # the store's birth: slot 0 holds publish 0, synced (manifest_make_slots + the first publish)
    S = Store(W, mode, o, fids)
    S.publish(0)
    S.sync_file(0)
    go(W, S, mode, o, fids, orc, ops, 0, T, [], cap)
    T.runs += 1


def check_view(W, v, mode, o, fids, orc):
    k, files = read_store(W, v, mode, o, fids)
    if k is None:
        return ('NO_MANIFEST', orc['acked']), k, files
    if k not in orc['written']:
        return ('WRONG', k), k, files
    if k < orc['acked']:
        return ('LOSS', k, 'acked', orc['acked']), k, files
    return None, k, files


def go(W, S, mode, o, fids, orc, ops, i, T, trace, cap):
    def any_check():
        out = []
        for v, d in W.dev.any_views():
            vd, k, files = check_view(W, v, mode, o, fids, orc)
            T.add('any', vd, {'ops': ops, 'trace': list(trace),
                              'view': {kk: vv for kk, vv in sorted(v.items(), key=repr)}})
            out.append((v, d, vd, k, files))
        return out
    W.hooks = [any_check]
    while i < len(ops):
        op = ops[i]
        i += 1
        trace.append(op)
        if op in ('X', 'P'):
            if op == 'X':
                v, d = W.dev.pc()
                vd, k, files = check_view(W, v, mode, o, fids, orc)
                T.add('proc', vd, {'ops': ops, 'trace': list(trace)})
                if vd:
                    return
                br = [(None, None, k, files)]
            else:
                res = [(v, d, k, f) for v, d, vd, k, f in any_check() if not vd]
                if len(res) > cap:
                    step = len(res) / cap
                    res = [res[int(j * step)] for j in range(cap)]
                br = res
            for v, d, k, files in br:
                W2 = W.clone() if op == 'X' else W.clone_after_loss(v, d)
                orc2 = {'written': set(orc['written']), 'acked': orc['acked']}
                S2 = Store(W2, mode, o, fids, {'seq': k, 'files': files})
                go(W2, S2, mode, o, fids, orc2, ops, i, T, list(trace), cap)
            return
        S.op(op, orc)
    any_check()
    for b in W.bad:
        T.add('seal', b, {'ops': ops})
        break


if __name__ == '__main__':
    mode = sys.argv[1]
    o = dict(OPT)
    maxlen = 4
    alpha = 'WUOBEXP'
    for a in sys.argv[2:]:
        k, val = a.split('=')
        if k == 'maxlen':
            maxlen = int(val)
        elif k == 'alpha':
            alpha = val
        else:
            o[k] = {'True': True, 'False': False}.get(val, int(val) if val.isdigit() else val)
    T = Tally('replace %s %s maxlen=%d alpha=%s' % (mode, {k: v for k, v in o.items() if OPT.get(k) != v}, maxlen, alpha))
    t0 = time.time()
    for n in range(1, maxlen + 1):
        for ops in itertools.product(alpha, repeat=n):
            if ops.count('P') > 1:
                continue
            run(mode, o, ops, T)
        if time.time() - t0 > 540:
            print('time limit')
            break
    sys.exit(1 if T.report() else 0)
