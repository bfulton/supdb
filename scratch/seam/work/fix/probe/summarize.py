import sys, statistics as st
from collections import defaultdict
rows = []
for line in open(sys.argv[1]):
    if not line.startswith('label='): continue
    d = {}
    for kv in line.split():
        k, v = kv.split('=', 1)
        try: d[k] = float(v)
        except ValueError: d[k] = v
    rows.append(d)
g = defaultdict(list)
for d in rows: g[(d['n'], d['shape'])].append(d)
order = ['A', 'Aw', 'A0', 'Ap', 'B', 'Cf', 'Cp', 'D']
def ms(vals):
    m = st.median(vals); return f"{m:.1f} [{min(vals):.1f}-{max(vals):.1f}]"
def m2(vals, f='{:.2f}'):
    m = st.median(vals); return (f+' [' + f + '-' + f + ']').format(m, min(vals), max(vals))
cols = ['shape','reps','med_us','p90_us','p99_us','wpart_med','spart_med','cpu_us','sect/c','ios/c','flush/c','late_sect','wbytes/c','handed/c','sys/c (pwv,pw,w,wv,fds,sfr,fadv)']
for n in sorted(set(k[0] for k in g)):
    print(f"\n== commit {int(n)} B ==")
    print(' | '.join(cols))
    for s in order:
        rs = g.get((n, s))
        if not rs: continue
        C = rs[0]['commits']
        sysc = '(' + ','.join(f"{st.median([r[k] for r in rs])/C:.2f}" for k in ['sys_pwritev','sys_pwrite','sys_write','sys_writev','sys_fdatasync','sys_sfr','sys_fadvise']) + ')'
        tot_sys = st.median([sum(r[k] for k in ['sys_pwritev','sys_pwrite','sys_write','sys_writev','sys_fdatasync','sys_sfr','sys_fadvise'])/C for r in rs])
        print(' | '.join([s, str(len(rs)), ms([r['med_us'] for r in rs]), ms([r['p90_us'] for r in rs]), ms([r['p99_us'] for r in rs]),
            ms([r['wpart_med_us'] for r in rs]), ms([r['spart_med_us'] for r in rs]), ms([r['cpu_us_per'] for r in rs]),
            m2([r['sect_per'] for r in rs]), m2([r['ios_per'] for r in rs]), m2([r['flush_per'] for r in rs]),
            m2([r['late_sect'] for r in rs], '{:.0f}'), m2([r['wbytes_per'] for r in rs], '{:.0f}'), f"{st.median([r['handed_per'] for r in rs]):.0f}",
            f"{tot_sys:.2f} {sysc}"]))
    # A-specific
    for s in ['A', 'Aw', 'A0', 'Ap']:
        rs = g.get((n, s))
        if not rs: continue
        r = rs[0]
        print(f"  {s}: frames={r['frames']:.0f} frame_bytes/c={r['frame_bytes']/r['commits']:.0f} ckpts={r['ckpts']:.0f} ckpt_bytes={r['ckpt_bytes']:.0f} wraps={r['wraps']:.0f} pages_home={r['pages_home']:.0f} tag_pages={r['tag_pages']:.0f} tail_bytes/c={r['tail_bytes']/r['commits']:.0f} seals/c={r['seals']/r['commits']:.1f} st_blocks*512/c={r['st_blocks']*512/r['commits']:.0f}")
    for s in ['Cp', 'Ap']:
        rs = g.get((n, s))
        if rs and 'prefill_sect' in rs[0]:
            print(f"  {s} prefill: bytes={rs[0]['prefill_bytes']:.0f} sect={st.median([r['prefill_sect'] for r in rs]):.0f} fsync={rs[0]['prefill_fsync']:.0f} (outside the window)")
