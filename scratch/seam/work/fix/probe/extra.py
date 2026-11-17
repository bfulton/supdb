# extra.py FILE... : the pairs and splits notes.md cites beyond analyze.py
import sys, statistics as st
rows = {}
for fn in sys.argv[1:]:
    for line in open(fn):
        if not line.startswith('label='): continue
        d = dict(kv.split('=', 1) for kv in line.split()); d['sit'] = d['label'].split('r')[0]
        rows[(d['label'], int(d['n']), d['shape'])] = d
sizes = sorted(set(k[1] for k in rows)); sits = sorted(set(d['sit'] for d in rows.values()))
def pr(a, b, n, key='med_us', sit=None):
    r = [float(d[key]) / float(rows[(l, n, b)][key]) for (l, nn, s), d in rows.items()
         if nn == n and s == a and (l, n, b) in rows and (sit is None or d['sit'] == sit)]
    return r
def fmt(r): return f"{st.median(r):.3f} [{min(r):.2f}-{max(r):.2f}] {sum(x > 1 for x in r)}/{len(r)}"
print('== against D (today\'s cipher run commit) and mean-per-commit ratios (the spikes included)')
for n in sizes:
    for a, b in [('R0','D'),('Rw','D'),('A0','D'),('Aw','D'),('O0','D'),('T0','D'),('R0','B'),('O0','B'),('A0','B')]:
        print(f"  n={n:<6} {a:>3}/{b:<2} median {fmt(pr(a,b,n))}   mean {fmt(pr(a,b,n,'mean_us'))}   p90 {fmt(pr(a,b,n,'p90_us'))}")
print('\n== decision rules per sitting, three decimals')
for n in sizes:
    for a, b in [('O0','A0'),('Ow','Aw'),('T0','A0'),('Tw','Aw'),('A0','J'),('Ap0','Jp'),('A0','Ds'),('Ap0','Dsp'),('R0','B'),('R0','A0'),('R0','D'),('S','Cf'),('Sp','Cp')]:
        print(f"  n={n:<6} {a:>3}/{b:<3} " + ' '.join(f"{s}:{st.median(pr(a,b,n,sit=s)):.3f}" for s in sits) + f"  pooled {st.median(pr(a,b,n)):.3f}")
print('\n== write part, sync part and cpu (median of runs, us) and the discard mode split of the sync part')
for n in sizes:
    for s in ['B','D','A0','Aw','O0','T0','R0','Rw','Cf','Cp','Ds','Dsp','S','J']:
        rs = [d for k, d in rows.items() if k[1] == n and k[2] == s]
        inm = [float(d['spart_med_us']) for d in rs if float(d['disc_per']) >= 0.3]
        out = [float(d['spart_med_us']) for d in rs if float(d['disc_per']) < 0.3]
        g = lambda k: st.median([float(d[k]) for d in rs])
        f2 = lambda v: f"{st.median(v):.0f} (n={len(v)})" if v else '-'
        print(f"  n={n:<6} {s:<3} write {g('wpart_med_us'):6.1f} sync {g('spart_med_us'):6.1f} cpu {g('cpu_us_per'):6.1f} mean {g('mean_us'):6.1f} | sync in mode {f2(inm)} out {f2(out)}")
