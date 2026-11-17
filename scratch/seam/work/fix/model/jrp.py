#!/usr/bin/env python3
"""JRP (design-free/design.md sec. 5): the ordered run as a journal-first Log
file, one request per write call.

Model scale: PB bytes a page; a homing when the call is >= TD bytes or the
bytes not home reach TH, or the ring cannot place the frame past its pins.
Ring: RU units of byte-packed frames, SU units a sector (sectors atomic, so a
frame is torn at every sector boundary and a sector shared by a pinned frame
and a new one holds old or new).  A frame of b bytes takes 1 + b units (its
head and tag as one unit) and never straddles the ring's end.  Data area:
slots ('d', i), each a page written once; an RP takes 1 slot, 2 when it homes
more than RP2 pages.  Header: ('h', 0), written once at create, unsynced.

Rules modelled as written (sec. 5.3-5.6): homing = one pwritev of [pages][RP]
at slot H + dead; RP body carries len, home, first, the pages' tags (here: the
(page, slot, image) list), dead, prev, tail; frames TAIL{from, bytes}; RPs and
frames share seq; nonces: page ('d', p, ctr), frame ('f', seq, att), RP
('rp', seq, att); a failed call's syscall lands TORN in the page cache and the
engine resends its bytes in front of the next call's (E4's retry model), which
reseals the same seq at attempt+1 and new ctrs (E4 F3).
Sync: fdatasync through the sequencer; issued_seq = the newest record of a
completed call; at completion synced_seq := max(synced_seq, issued_seq).
Pins: P1 (synced base RP, synced_seq], P2 the same per sync in flight,
P3 (newest RP, head].
Recovery: header; RPs scanned in the data area; the newest RP R such that R,
every RP in its prev chain and every page each homed verify; then frames with
seq > R.seq, contiguous, highest attempt per seq, each TAIL beginning at the
current length.
"""
from cand import Fail, Blocked, TORN, syscalls

DEFAULT = dict(
    PB=2, TD=4, TH=6, RU=16, SU=2, RP2=2,
    pin_p1=True, pin_p2=True, pin_p3=True,
    # mutants
    m1_home_at_rp=False,     # the homing starts at the newest RP's slot (data over the RP)
    m2_tail_home=False,      # the partial tail page written home, not carried in the RP
    m3_p1_drop=False,        # P1 dropped once a newer (unsynced) RP exists
    m5_rp_nopages=False,     # an RP trusted without its own pages
    m6_rp_nopred=False,      # an RP trusted without its predecessors (and their pages)
    m7_gap=False,            # frames applied across a seq gap
    m8_ctr=False,            # a failed call gives back its seq, attempt and ctr
    cont=False,              # FIX: the RPs of one call but its last are CONT, never an end state
)


class JRP:
    def __init__(s, W, o, orc):
        s.W, s.o, s.orc = W, dict(DEFAULT, **o), orc
        o = s.o
        s.key = W.uniq('S')
        s.L, s.content = 0, []
        s.H, s.dead = 0, 0
        s.slot_of = {}           # page -> slot it was (last) written home at
        s.tags = {}              # page -> image
        s.rps = []               # (seq, slot) of RPs of completed calls
        s.prev = None
        s.next_slot = 0          # the first fresh data slot
        s.seq, s.ctr, s.att = -1, 0, {}
        s.frames = {}            # seq -> (pos, n) of frames of completed calls
        s.head = 0
        s.synced_seq = None
        s.infl = {}
        s.failed = []
        hdr = W.seal(('kf',), W.uniq('rnd'), ('hdr', s.key), kind='hdr')
        s.hdr = hdr
        W.dev.write([(('h', 0), hdr)])

    # ---- pins
    def base_rp(s, upto):
        b = -1
        for q, _ in s.rps:
            if q <= upto:
                b = max(b, q)
        return b

    def pinned_units(s):
        spans = []
        newest = max((q for q, _ in s.rps), default=-1)
        if s.synced_seq is not None and s.o['pin_p1']:
            b = s.base_rp(s.synced_seq)
            if not (s.o['m3_p1_drop'] and newest > b):
                spans.append((b, s.synced_seq))
        if s.o['pin_p2']:
            for iss, _ in s.infl.values():
                if iss is not None:
                    spans.append((s.base_rp(iss), iss))
        if s.o['pin_p3']:
            spans.append((newest, 10 ** 9))
        u = set()
        for q, (pos, n) in s.frames.items():
            if any(a < q <= b for a, b in spans):
                u.update(range(pos, pos + n))
        return u

    def alloc(s, n):
        RU = s.o['RU']
        pin = s.pinned_units()
        for k in range(RU):
            pos = (s.head + k) % RU
            if pos + n > RU:
                continue
            if any(x in pin for x in range(pos, pos + n)):
                continue
            return pos
        return None

    # ---- sectors of the ring
    def frame_objs(s, pos, units):
        """the sector writes that put `units` (list of unit values) at pos"""
        SU = s.o['SU']
        out = {}
        for i, uval in enumerate(units):
            u = pos + i
            sec = u // SU
            if sec not in out:
                cur = s.W.dev.cur(('rs', sec))
                out[sec] = list(cur) if cur is not None else [None] * SU
            out[sec][u % SU] = uval
        return [(('rs', sec), tuple(v)) for sec, v in sorted(out.items())]

    # ---- writes
    def nctr(s):
        c = s.ctr
        s.ctr += 1
        return c

    def write(s, nb, fail=None, mid=None):
        o, W = s.o, s.W
        PB = o['PB']
        new = [W.uniq('b') for _ in range(nb)]
        bts = s.failed + new
        snap = (s.seq, s.ctr, dict(s.att))
        s.seq += 1
        att = s.att.get(s.seq, 0)
        L2 = s.L + len(bts)
        content2 = s.content + bts
        J2 = L2 - s.H * PB
        n_fr = 1 + len(bts)
        pos = None
        home = len(bts) >= o['TD'] or J2 >= o['TH']
        if not home:
            pos = s.alloc(n_fr)
            if pos is None:
                home = True
                W.events['ring_full_home'] += 1
        if home:
            calls, upd = s.homing(L2, content2, att)
            W.events['homing'] += 1
        else:
            body = ('TAIL', s.L, tuple(bts))
            img = W.seal(s.key, ('f', s.seq, att), body, kind='frame', seq=s.seq, att=att,
                         frm=s.L, b=tuple(bts), n=n_fr)
            units = [(img, i) for i in range(n_fr)]
            calls = [s.frame_objs(pos, units)]
            upd = ('frame', pos, n_fr)
            W.events['frame'] += 1
        if fail is not None:
            # the call's (only) syscall fails: its objects torn in the page cache
            if home:
                W.dev.write([(loc, TORN) for loc, _ in calls[0]])
            else:
                W.dev.write(s.frame_objs(pos, [('G', W.uniq('g'))] * n_fr))
                s.head = pos + n_fr
            s.failed = bts
            k = getattr(s, 'nrp_call', 1) if home else 1
            for q in range(s.seq, s.seq + k):
                s.att[q] = s.att.get(q, 0) + 1
            if o['m8_ctr']:
                s.seq, s.ctr, s.att = snap
            else:
                s.seq -= 1          # the retry reseals these seqs at attempt+1
            raise Fail()
        syscalls(W, calls, mid, content2)
        # the call completed
        s.failed = []
        s.L, s.content = L2, content2
        if upd[0] == 'frame':
            s.frames[s.seq] = (upd[1], upd[2])
            s.head = upd[1] + upd[2]
        else:
            _, newH, dead, rpl, slots, nxt = upd
            s.H, s.dead, s.next_slot = newH, dead, nxt
            s.rps += rpl
            s.prev = rpl[-1][1]
            s.seq = rpl[-1][0]
            s.slot_of.update(slots)
        s.orc.done(s.content)

    def homing(s, L2, content2, att):
        """[pages][RP], or with RPMAX several [pages][RP] in the one pwritev
        (sec. 5.1: an RP covers at most 1024 pages)"""
        o, W = s.o, s.W
        PB = o['PB']
        newH = L2 // PB
        first = s.H
        slots = {}
        nxt = s.next_slot
        if o['m1_home_at_rp'] and s.prev is not None:
            nxt = s.prev                      # MUTANT: over the newest RP
        plist = list(range(first, newH))
        tail = tuple(content2[newH * PB:L2])
        partial = None
        if o['m2_tail_home'] and tail:
            partial = newH                    # MUTANT: the partial page goes home
        allp = plist + ([partial] if partial is not None else [])
        k = o.get('RPMAX') or max(1, len(allp))
        chunks = [allp[i:i + k] for i in range(0, len(allp), k)] or [[]]
        objs, rpl = [], []
        prev, dead, seq = s.prev, s.dead, s.seq
        for ci, ch in enumerate(chunks):
            pages = []
            for p in ch:
                if p in s.slot_of and o['m2_tail_home']:
                    sl = s.slot_of[p]         # rewritten in place
                else:
                    sl = nxt
                    nxt += 1
                data = tuple(content2[p * PB:min((p + 1) * PB, L2)])
                img = W.seal(s.key, ('d', p, s.nctr()), data, kind='data', p=p)
                pages.append((p, sl, img))
                slots[p] = sl
            lastc = ci == len(chunks) - 1
            npg = len(ch)
            n_rp = 1 if npg <= o['RP2'] else 2
            slot = nxt
            nxt += n_rp
            dead += n_rp
            if ci > 0:
                seq += 1
            a = s.att.get(seq, 0)
            if lastc:
                ln, hm, tl = L2, newH, (() if partial is not None else tail)
            else:
                hm = ch[-1] + 1
                ln, tl = hm * PB, ()
            body = dict(seq=seq, att=a, len=ln, home=hm, first=ch[0] if ch else first,
                        pages=tuple(pages), dead=dead, prev=prev, tail=tl, n=n_rp,
                        cont=bool(o['cont'] and not lastc))
            img = W.seal(s.key, ('rp', seq, a), repr(sorted(body.items())), kind='rp',
                         slot=slot, **body)
            objs += [(('d', sl), im) for _, sl, im in pages] + [(('d', slot + j), img) for j in range(n_rp)]
            rpl.append((seq, slot))
            prev = slot
        objs.sort(key=lambda x: x[0][1])
        s.nrp_call = len(chunks)
        upd = ('home', newH, dead, rpl, slots, nxt)
        return [objs], upd

    # ---- syncs
    def sync_begin(s):
        tok = s.W.dev.sync_begin()
        s.infl[tok] = (s.seq if s.seq >= 0 else None, s.orc.at())
        return tok

    def sync_end(s, tok):
        s.W.dev.sync_end(tok)
        iss, idx = s.infl.pop(tok)
        if iss is not None:
            s.synced_seq = iss if s.synced_seq is None else max(s.synced_seq, iss)
        s.orc.acked(idx)

    def sync(s):
        s.sync_end(s.sync_begin())

    def sync_end_oldest(s):
        if s.infl:
            s.sync_end(min(s.infl))


def recover(W, v, o, h=None):
    """returns (content tuple, unreadable list)"""
    o = dict(DEFAULT, **o)
    hdr = v.get(('h', 0))
    if hdr in (None, TORN) or W.imgs[hdr]['kind'] != 'hdr':
        return (), []
    key = W.imgs[hdr]['content'][1]
    # RPs in the data area
    rps = {}
    for loc, x in v.items():
        if loc[0] != 'd' or x in (None, TORN):
            continue
        m = W.imgs[x]
        if m['kind'] != 'rp' or m['key'] != key or m['slot'] != loc[1]:
            continue
        if all(v.get(('d', m['slot'] + j)) == x for j in range(m['n'])):
            q = (m['seq'], m['att'])
            rps[m['slot']] = m
    by_slot = rps

    def pages_ok(m):
        return all(v.get(('d', sl)) == im for _, sl, im in m['pages'])

    def chain_ok(m):
        if not (o['m5_rp_nopages'] or pages_ok(m)):
            return False
        if o['m6_rp_nopred']:
            return True
        p = m['prev']
        seen = 0
        while p is not None:
            pm = by_slot.get(p)
            if pm is None or pm['seq'] >= m['seq']:
                return False
            if not (o['m5_rp_nopages'] or pages_ok(pm)):
                return False
            p = pm['prev']
            seen += 1
            assert seen < 10000
        return True

    R = None
    for m in sorted(by_slot.values(), key=lambda m: (m['seq'], m['att']), reverse=True):
        if m.get('cont'):
            continue
        if chain_ok(m):
            R = m
            break
    # the content from R
    pagec = {}
    if R is not None:
        chain = []
        m = R
        while m is not None:
            chain.append(m)
            m = by_slot.get(m['prev']) if m['prev'] is not None else None
        for m in reversed(chain):
            for p, sl, im in m['pages']:
                pagec[p] = (sl, im)
        L = R['len']
        content = []
        bad = []
        PB = o['PB']
        np_ = R['home'] + (1 if (R['tail'] == () and R['home'] in pagec) else 0)
        for p in range(np_):
            if p not in pagec:              # a page no verified RP names (m6's broken chain)
                bad.append(p)
                content += [None] * PB
                continue
            sl, im = pagec[p]
            x = v.get(('d', sl))
            if x == im:
                content += list(W.imgs[im]['content'])
            else:
                bad.append(p)
                content += [None] * PB
        content += list(R['tail'])
        content = content[:L]
        q0 = R['seq']
    else:
        content, bad, L, q0 = [], [], 0, -1
    # frames in the ring: a unit holding (img, 0) followed by the rest of it
    RU, SU = o['RU'], o['SU']
    units = []
    for sec in range(RU // SU):
        x = v.get(('rs', sec))
        units += list(x) if x is not None else [None] * SU
    fr = {}
    for u in range(RU):
        x = units[u]
        if not (isinstance(x, tuple) and len(x) == 2 and x[1] == 0 and isinstance(x[0], int)):
            continue
        m = W.imgs.get(x[0])
        if m is None or m['kind'] != 'frame' or m['key'] != key:
            continue
        if u + m['n'] > RU or any(units[u + i] != (x[0], i) for i in range(m['n'])):
            continue
        q = m['seq']
        if q not in fr or m['att'] > fr[q]['att']:
            fr[q] = m
    if o['m7_gap']:
        qs = sorted(q for q in fr if q > q0)
    else:
        qs = []
        q = q0 + 1
        while q in fr:
            qs.append(q)
            q += 1
    for q in qs:
        m = fr[q]
        if m['frm'] != len(content):
            break
        content += list(m['b'])
    return tuple(content), bad
