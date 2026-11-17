# analyze.py FILE... : the pooled and paired tables of the fix's probe.
# A row is one process (one shape, one size, one rep); a pair is two shapes in
# the same rep of the same sitting at the same size. Ratios are of the runs'
# medians per commit, paired per rep, then pooled: median [min-max], the
# count of pairs where the first is slower, and the per-sitting medians.
import sys, statistics as st
from collections import defaultdict

rows = {}
for fn in sys.argv[1:]:
    for line in open(fn):
        if not line.startswith('label='):
            continue
        d = dict(kv.split('=', 1) for kv in line.split())
        d['sit'] = d['label'].split('r')[0]
        rows[(d['label'], int(d['n']), d['shape'])] = d

sizes = sorted(set(k[1] for k in rows))
sits = sorted(set(d['sit'] for d in rows.values()))
SH = ['A', 'Aw', 'A0', 'Ap0', 'B', 'D', 'Ow', 'O0', 'Tw', 'T0', 'Rw', 'R0', 'Cf', 'Cp', 'Ds', 'Dsp', 'J', 'Jp', 'S', 'Sp']
MODE = 0.3  # discards a commit at or above which a run is "in the discard mode"


def f(v, q=float):
    return q(v)


def q3(v, fmt='{:.1f}'):
    v = sorted(v)
    return (fmt + ' [' + fmt + '-' + fmt + ']').format(st.median(v), v[0], v[-1])


print('== absolute, pooled over sittings: median of run medians [min-max]; n = runs')
for n in sizes:
    print(f'\n-- commit {n} B')
    print(f"{'shape':5} {'n':>3} {'med_us':>22} {'p90_us':>22} {'p99':>8} {'wpart':>6} {'spart':>7} {'cpu':>6} {'sect/c':>8} {'ios/c':>6} {'disc/c':>6} {'inmode':>6} {'late':>5} {'blocks/c':>8}")
    for s in SH:
        rs = [d for k, d in rows.items() if k[1] == n and k[2] == s]
        if not rs:
            continue
        g = lambda key: [float(d[key]) for d in rs]
        inmode = sum(1 for d in rs if float(d['disc_per']) >= MODE)
        print(f"{s:5} {len(rs):3} {q3(g('med_us')):>22} {q3(g('p90_us')):>22} {st.median(g('p99_us')):8.0f} {st.median(g('wpart_med_us')):6.1f} {st.median(g('spart_med_us')):7.1f} {st.median(g('cpu_us_per')):6.1f} {st.median(g('sect_per')):8.2f} {st.median(g('ios_per')):6.3f} {st.median(g('disc_per')):6.3f} {inmode:3}/{len(rs):<2} {st.median(g('late_sect')):5.0f} {st.median([float(d['st_blocks']) * 512 / float(d['commits']) for d in rs]):8.0f}")

PAIRS = [
    ('E5 again', [('A', 'B'), ('Aw', 'B'), ('A0', 'B'), ('Aw', 'A0'), ('D', 'B')]),
    ('ADJ one write (O)', [('O0', 'A0'), ('Ow', 'Aw'), ('O0', 'B'), ('Ow', 'B'), ('Ow', 'D'), ('O0', 'T0')]),
    ('ADJ two writes (T)', [('T0', 'A0'), ('Tw', 'Aw'), ('T0', 'B'), ('Tw', 'B'), ('Tw', 'D')]),
    ('JRP (R)', [('R0', 'A0'), ('Rw', 'Aw'), ('R0', 'B'), ('Rw', 'B'), ('Rw', 'D'), ('R0', 'T0'), ('R0', 'O0'), ('Rw', 'Ow')]),
    ('write-through by pwrite', [('Ow', 'O0'), ('Tw', 'T0'), ('Rw', 'R0')]),
    ('WAL records against the plain WAL', [('Ds', 'Cf'), ('Dsp', 'Cp'), ('J', 'Cf'), ('Jp', 'Cp'), ('S', 'Cf'), ('Sp', 'Cp')]),
    ('WAL records against each other', [('J', 'Ds'), ('S', 'Ds'), ('Jp', 'Dsp'), ('Sp', 'Dsp')]),
    ('a paged Log WAL against the record WAL (decision 2)', [('A0', 'Ds'), ('T0', 'Ds'), ('O0', 'Ds'), ('R0', 'Ds'), ('A0', 'J'), ('R0', 'S'), ('Ap0', 'Dsp'), ('Ap0', 'Jp'), ('R0', 'Dsp'), ('O0', 'Dsp'), ('T0', 'Dsp')]),
]


def pair(a, b, n, sit=None, key='med_us', same_mode=None):
    out = []
    for (lab, nn, s), d in rows.items():
        if nn != n or s != a or (sit and d['sit'] != sit):
            continue
        e = rows.get((lab, n, b))
        if not e:
            continue
        if same_mode is not None:
            ma, mb = float(d['disc_per']) >= MODE, float(e['disc_per']) >= MODE
            if (ma == mb) != same_mode:
                continue
        out.append((float(d[key]) / float(e[key]), float(d[key]) - float(e[key])))
    return out


print('\n== paired per rep: ratio of run medians, median [min-max], first slower in k/n; diff in us; per-sitting medians; sectors ratio; pairs whose two runs share the discard mode')
for title, prs in PAIRS:
    print(f'\n-- {title}')
    for n in sizes:
        for a, b in prs:
            p = pair(a, b, n)
            if not p:
                continue
            r = [x[0] for x in p]
            dd = [x[1] for x in p]
            ps = ' '.join(f"{s}:{st.median([x[0] for x in pair(a, b, n, s)]):.2f}" for s in sits if pair(a, b, n, s))
            sr = [x[0] for x in pair(a, b, n, key='sect_per')]
            same = [x[0] for x in pair(a, b, n, same_mode=True)]
            sm = f"{st.median(same):.2f} [{min(same):.2f}-{max(same):.2f}] {sum(x > 1 for x in same)}/{len(same)}" if same else '-'
            print(f"  n={n:<6} {a:>3}/{b:<3} t {st.median(r):.2f} [{min(r):.2f}-{max(r):.2f}] {sum(x > 1 for x in r):2}/{len(r):<2} diff {st.median(dd):+6.0f} us [{min(dd):+.0f},{max(dd):+.0f}]  sittings {ps}  sect {st.median(sr):.2f}  same-mode {sm}")

print('\n== counters of the candidates (median per run)')
for n in sizes:
    for s in ['Ow', 'O0', 'Tw', 'T0']:
        rs = [d for k, d in rows.items() if k[1] == n and k[2] == s]
        if not rs:
            continue
        C = float(rs[0]['commits'])
        m = lambda key: st.median([float(d[key]) for d in rs])
        print(f"  n={n:<6} {s}: cases c0={m('c0'):.0f} c1={m('c1'):.0f} c2={m('c2'):.0f} c3={m('c3'):.0f} c4={m('c4'):.0f}; carried pages/c={m('carried') / C:.2f}; record pages/c ring {m('rec_pages_ring') / C:.2f} data {m('rec_pages_data') / C:.2f}; home runs/c {m('home_runs') / C:.2f}; tag pages/c {m('tag_pages') / C:.3f}; fallbacks noncontig {m('fb_noncontig'):.0f} reach {m('fb_reach'):.0f} carryP {m('fb_carryP'):.0f} qmax {m('fb_qmax'):.0f}")
    for s in ['Rw', 'R0']:
        rs = [d for k, d in rows.items() if k[1] == n and k[2] == s]
        if not rs:
            continue
        C = float(rs[0]['commits'])
        m = lambda key: st.median([float(d[key]) for d in rs])
        print(f"  n={n:<6} {s}: homings/c {m('homings') / C:.3f} (pages each {m('homing_pages') / max(m('homings'), 1):.1f}); RP slots {m('rp_slots'):.0f} (dead {m('rp_slots') * 4096 / (m('homing_pages') * 4096 + 1) * 100:.1f}% of homed); frames/c {m('frames') / C:.3f}; journaled B/c {m('journaled') / C:.0f}; wraps {m('wraps'):.0f}; ring_full {m('ring_full'):.0f}")
    for s in ['A', 'A0']:
        rs = [d for k, d in rows.items() if k[1] == n and k[2] == s]
        if not rs:
            continue
        C = float(rs[0]['commits'])
        m = lambda key: st.median([float(d[key]) for d in rs])
        print(f"  n={n:<6} {s}: tail bytes/c {m('tail_bytes') / C:.0f}; frame bytes/c {m('frame_bytes') / C:.0f}; ckpts {m('ckpts'):.0f}; tag pages/c {m('tag_pages') / C:.3f}")
