#!/usr/bin/env python3
"""ADJ (design-adjacent/design.md sec. 3-7): self-sufficient records, placed
beside the data (cases 2/3) or in ring slots (cases 0, 1, 4).

Model scale: PB bytes a page, G pages a group (tag pages chained: a tag
page's sealed content carries the MAC of the one before), M (cases 2/3 for
1 <= m <= M), QMAX = 2M+2, NS ring slots of 2 pages, PIECE pages a writer's
piece (case 1), a record = 1 page + 1 page per pending image carried.
Locations: ('d', p) data page p's home, which a record may also occupy;
('r', i) ring pages; ('t', g) tag page g; ('h', 0) header.  All pages
(written whole, TORN when mixed).

Write rule as sec. 3.2, in this order (the spec gives conditions, not an
order; this is the literal reading):
  m == 0                                   case 0: record -> first free ring run
  case1 and t == 0 at a piece boundary,
      the call whole pieces, Q empty         case 1: homes, record -> ring (no P check: as written)
  m > M                                    case 4
  H contiguous and J misses P              case 2: one write [homes(H)][record at home(T)]
  P meets homes(H), m <= mhat, |H|<=QMAX   case 3: record carrying H at home(T)+mhat+r
                                                   (its placement meeting P -> case 4, counted)
  otherwise                                case 4: homes of H outside P (a write per run),
                                                   record -> ring, blocked pages stay in Q
  then case 5: every group now whole, in chain order, its tag page (one more write).
P = the records of A_pc, A_sync and each sync in flight (registered at issue
with the newest completed call's record; on completion A_sync := it if newer).
synced_home = the home frontier of A_sync = min(A_sync.Q, A_sync.T).
Recovery as sec. 5: every ring page and data page holding a whole record
sealed for its place; newest (seq, attempt) first; the first whose tag chain
verifies and whose non-pending pages in [round_down(sh, G), F) verify.
"""
from cand import Fail, Blocked, TORN, syscalls

DEFAULT = dict(
    PB=2, G=2, M=2, NS=8, PIECE=2, R0=1,
    case1=True,
    case1_checkP=False,      # FIX candidate: case 1 falls to case 4 when its homes meet P
    lit_interval=False,      # VARIANT: case 2 over a non-contiguous H writes the whole
                             # interval, resealing the home pages in its gaps
    # mutants
    P_none=False,            # nothing protected (the one-commit delay and the pin both gone)
    nocarry=False,           # the one-commit delay removed: a flush over the protected record
    P_no_sync=False,         # A_sync not protected: a record may go over the newest durable copy
    P_no_inflight=False,     # a sync in flight pins nothing
    rec_nocheck=False,       # recovery takes the newest record that verifies, unchecked
    m_ctr=False,             # a failed call gives back its seq, attempt and ctr
    sh='frontier',           # 'frontier' | 'F' (A_sync.F, pending ignored) | 'completion'
    noround=False,
)


class ADJ:
    def __init__(s, W, o, orc):
        s.W, s.o, s.orc = W, dict(DEFAULT, **o), orc
        s.key = W.uniq('S')
        s.L, s.content = 0, []
        s.Q = []
        s.home = set()
        s.tags = {}
        s.chain_g, s.chain_mac = -1, None
        s.seq, s.ctr, s.att = -1, 0, {}
        s.A_pc = s.A_sync = None
        s.sh_comp = 0
        s.infl = {}
        s.ms = []
        s.failed = []
        hdr = W.seal(('kf',), W.uniq('rnd'), ('hdr', s.key), kind='hdr')
        W.dev.write([(('h', 0), hdr)])

    @property
    def T(s):
        return s.L // s.o['PB']

    def nctr(s):
        c = s.ctr
        s.ctr += 1
        return c

    def P(s):
        o = s.o
        if o['P_none']:
            return set()
        locs = set()
        recs = [s.A_pc]
        if not o['P_no_sync']:
            recs.append(s.A_sync)
        if not o['P_no_inflight']:
            recs += [r for r, _ in s.infl.values()]
        for r in recs:
            if r is not None:
                locs.update(r['locs'])
        return locs

    def frontier(s, r):
        if r is None:
            return 0
        return min(list(r['Q']) + [r['T']])

    def synced_home(s):
        if s.o['sh'] == 'F':
            return s.A_sync['T'] if s.A_sync else 0
        if s.o['sh'] == 'completion':
            return s.sh_comp
        return s.frontier(s.A_sync)

    def ring_alloc(s, k, P):
        NS = s.o['NS']
        need = (k + 1) // 2
        for j in range(NS - need + 1):
            locs = [('r', 2 * j + i) for i in range(k)]
            if not (set(locs) & P):
                return locs
        return None

    def write(s, nb, fail=None, mid=None):
        o, W = s.o, s.W
        PB, G, M = o['PB'], o['G'], o['M']
        new = [W.uniq('b') for _ in range(nb)]
        bts = s.failed + new
        save = (s.L, list(s.content), list(s.Q), set(s.home), dict(s.tags), s.chain_g, s.chain_mac)
        snap = (s.seq, s.ctr, dict(s.att))
        T0 = s.T
        L2 = s.L + len(bts)
        content2 = s.content + bts
        T2 = L2 // PB
        C = list(range(T0, T2))
        m = len(C)
        H = sorted(set(s.Q) | set(C))
        P = s.P()
        mhat = max(s.ms[-4:], default=0)
        s.seq += 1
        att = s.att.get(s.seq, 0)
        R0 = o['R0']

        def page(p):
            return tuple(content2[p * PB:(p + 1) * PB])

        def homes(ps):
            objs = []
            for p in ps:
                img = W.seal(s.key, ('d', p, s.nctr()), page(p), kind='data', p=p)
                objs.append((('d', p), img))
                s.tags[p] = img
                s.home.add(p)
            return objs

        def tagpages():
            objs = []
            g = s.chain_g + 1
            while all(g * G + i in s.home for i in range(G)) and (g + 1) * G <= T2:
                tg = tuple(s.tags[g * G + i] for i in range(G))
                img = W.seal(s.key, ('t', g, s.nctr()), (tg, s.chain_mac), kind='tag', g=g,
                             tg=tg, prev=s.chain_mac)
                objs.append((('t', g), img))
                s.chain_g, s.chain_mac = g, img
                g += 1
            return objs

        def record(pending, locs):
            body = dict(seq=s.seq, att=att, L=L2, sh=s.synced_home(), F=T2,
                        chain_g=s.chain_g, chain_mac=s.chain_mac,
                        open=tuple(sorted((p, s.tags[p]) for p in s.home if p // G > s.chain_g)),
                        pending=tuple((p, page(p)) for p in pending),
                        tail=tuple(content2[T2 * PB:L2]), locs=tuple(locs))
            img = W.seal(s.key, ('rec', s.seq, att), repr(sorted(body.items())), kind='rec',
                         **body)
            return img, [(l, img) for l in locs]

        def touches_P(objs):
            if set(l for l, _ in objs) & P:
                W.events['P_WRITTEN'] += 1

        case = None
        if m == 0:
            case = 0
        elif (o['case1'] and L2 % PB == 0 and s.L % (o['PIECE'] * PB) == 0
              and len(bts) % (o['PIECE'] * PB) == 0 and not s.Q):
            case = 1
            if o['case1_checkP'] and set(('d', p) for p in H) & P:
                case = 4
        elif m > M:
            case = 4
        else:
            contiguous = H == list(range(H[0], T2))
            Jh = set(('d', p) for p in H)
            Jr = set(('d', T2 + i) for i in range(R0))
            if o['nocarry'] and contiguous and not (Jr & P):
                case = 2          # MUTANT: written over whatever protected record sits on H
            elif contiguous and not ((Jh | Jr) & P):
                case = 2
            elif not contiguous and o['lit_interval'] and not ((set(('d', p) for p in range(H[0], T2)) | Jr) & P):
                case = '2i'
            elif (Jh & P) and m <= mhat and len(H) <= 2 * M + 2:
                case = 3
            else:
                case = 4
                if not contiguous:
                    W.events['case2_noncontig'] += 1
        calls = []
        newQ = []
        if case == 0:
            newQ = list(s.Q)
            locs = s.ring_alloc(1 + len(newQ), P)
            if locs is None:
                W.events['ring_blocked'] += 1
                raise Blocked()
            img, robjs = record(newQ, locs)
            calls = [robjs]
        elif case == 1:
            hobjs = homes(H)
            touches_P(hobjs)
            tobjs = tagpages()
            locs = s.ring_alloc(1, P)
            if locs is None:
                W.events['ring_blocked'] += 1
                raise Blocked()
            img, robjs = record([], locs)
            calls = [hobjs] + ([tobjs] if tobjs else []) + [robjs]
        elif case in (2, '2i'):
            ps = H if case == 2 else list(range(H[0], T2))
            if case == '2i':
                W.events['gap_rehome'] += sum(1 for p in ps if p in s.home)
            hobjs = homes(ps)
            touches_P(hobjs)
            tobjs = tagpages()
            img, robjs = record([], [('d', T2 + i) for i in range(R0)])
            calls = [hobjs + robjs] + ([tobjs] if tobjs else [])
        elif case == 3:
            start = T2 + mhat + R0
            locs = [('d', start + i) for i in range(R0 + len(H))]
            if set(locs) & P:
                W.events['carry_meets_P'] += 1
                case = 4
            else:
                newQ = list(H)
                img, robjs = record(newQ, locs)
                calls = [robjs]
        if case == 4:
            go = [p for p in H if ('d', p) not in P]
            newQ = [p for p in H if ('d', p) in P]
            if newQ:
                W.events['case4_blocked'] += 1
            runs = []
            for p in go:
                if runs and runs[-1][-1] == p - 1:
                    runs[-1].append(p)
                else:
                    runs.append([p])
            hcalls = [homes(r) for r in runs]
            tobjs = tagpages()
            locs = s.ring_alloc(1 + len(newQ), P)
            if locs is None:
                W.events['ring_blocked'] += 1
                raise Blocked()
            img, robjs = record(newQ, locs)
            calls = hcalls + ([tobjs] if tobjs else []) + [robjs]
        W.events['case%s' % case] += 1
        rec = dict(seq=s.seq, att=att, img=img, locs=tuple(l for l, _ in robjs), T=T2, Q=tuple(newQ))
        if fail is not None:
            k = fail if fail >= 0 else len(calls) + fail
            k = max(0, min(k, len(calls) - 1))
            for objs in calls[:k]:
                W.dev.write(objs)
            W.dev.write([(l, TORN) for l, _ in calls[k]])
            (s.L, s.content, s.Q, s.home, s.tags, s.chain_g, s.chain_mac) = save
            s.failed = bts
            s.att[s.seq] = att + 1
            if o['m_ctr']:
                s.seq, s.ctr, s.att = snap
            else:
                s.seq -= 1
            raise Fail()
        syscalls(W, calls, mid, content2)
        s.failed = []
        s.L, s.content, s.Q = L2, content2, newQ
        s.A_pc = rec
        s.ms.append(m)
        s.orc.done(s.content)

    # ---- syncs
    def sync_begin(s):
        tok = s.W.dev.sync_begin()
        s.infl[tok] = (s.A_pc, s.orc.at())
        return tok

    def sync_end(s, tok):
        s.W.dev.sync_end(tok)
        r, idx = s.infl.pop(tok)
        if r is not None and (s.A_sync is None or (r['seq'], r['att']) > (s.A_sync['seq'], s.A_sync['att'])):
            s.A_sync = r
        s.sh_comp = s.frontier(s.A_pc)          # what 'completion' would take
        s.orc.acked(idx)

    def sync(s):
        s.sync_end(s.sync_begin())

    def sync_end_oldest(s):
        if s.infl:
            s.sync_end(min(s.infl))


def recover(W, v, o, h=None):
    o = dict(DEFAULT, **o)
    PB, G = o['PB'], o['G']
    hdr = v.get(('h', 0))
    if hdr in (None, TORN) or W.imgs[hdr]['kind'] != 'hdr':
        return (), []
    key = W.imgs[hdr]['content'][1]
    cands = {}
    for loc, x in v.items():
        if loc[0] not in ('d', 'r') or x in (None, TORN):
            continue
        m = W.imgs[x]
        if m['kind'] != 'rec' or m['key'] != key or m['locs'][0] != loc:
            continue
        if all(v.get(l) == x for l in m['locs']):
            cands[x] = m
    for R in sorted(cands.values(), key=lambda m: (m['seq'], m['att']), reverse=True):
        # the chain the record names
        chain = {}
        g, mac, ok = R['chain_g'], R['chain_mac'], True
        while g >= 0:
            if v.get(('t', g)) != mac:
                ok = False
                break
            chain[g] = mac
            mac = W.imgs[mac]['prev']
            g -= 1
        opent = dict(R['open'])
        pend = dict(R['pending'])

        def expected(p):
            if p in opent:
                return opent[p]
            g = p // G
            if g in chain:
                return W.imgs[chain[g]]['tg'][p % G]
            return None

        if not o['rec_nocheck']:
            if not ok:
                continue
            lo = R['sh'] if o['noround'] else (R['sh'] // G) * G
            if not all(p in pend or (expected(p) is not None and v.get(('d', p)) == expected(p))
                       for p in range(lo, R['F'])):
                continue
        content, bad = [], []
        for p in range(R['F']):
            if p in pend:
                content += list(pend[p])
                continue
            e = expected(p)
            if e is not None and v.get(('d', p)) == e:
                content += list(W.imgs[e]['content'])
            else:
                bad.append(p)
                content += [None] * PB
        content += list(R['tail'])
        return tuple(content[:R['L']]), bad
    return (), []
