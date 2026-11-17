import sys, statistics as st
from collections import defaultdict
rows = {}
for fn in sys.argv[1:]:
    for line in open(fn):
        if not line.startswith('label='): continue
        d = dict(kv.split('=',1) for kv in line.split())
        rows[(fn, d['label'], int(d['n']), d['shape'])] = d
def q(vals):
    vals = sorted(vals); n = len(vals)
    return st.median(vals), vals[0], vals[-1], n
sizes = sorted(set(k[2] for k in rows))
shapes = ['A','Aw','A0','Ap','B','Cf','Cp','D']
print("absolute, pooled over sittings (median of run medians [min-max], n runs)")
for n in sizes:
    print(f"n={n}")
    for s in shapes:
        rs = [v for k,v in rows.items() if k[2]==n and k[3]==s]
        if not rs: continue
        m = q([float(r['med_us']) for r in rs]); p = q([float(r['p90_us']) for r in rs]); sc = q([float(r['sect_per']) for r in rs]); io = q([float(r['ios_per']) for r in rs])
        w = q([float(r['wpart_med_us']) for r in rs]); sp = q([float(r['spart_med_us']) for r in rs]); cpu = q([float(r['cpu_us_per']) for r in rs]); wb = q([float(r['wbytes_per']) for r in rs])
        print(f"  {s:3} n={m[3]:2} med {m[0]:6.1f} [{m[1]:.0f}-{m[2]:.0f}]  p90 {p[0]:6.1f} [{p[1]:.0f}-{p[2]:.0f}]  write-part {w[0]:5.1f}  sync-part {sp[0]:6.1f}  cpu {cpu[0]:5.1f}  sect/c {sc[0]:6.2f} [{sc[1]:.2f}-{sc[2]:.2f}]  ios/c {io[0]:.2f}  wbytes/c {wb[0]:.0f}")
pairs = [('A','B'),('Aw','B'),('A0','B'),('Ap','Cp'),('A','D'),('D','B'),('Cp','Cf')]
print("\npaired per rep, pooled over sittings: ratio of run medians (median [min-max], A slower in k of n), and difference in us")
for n in sizes:
    print(f"n={n}")
    for a,b in pairs:
        rat=[]; dif=[]; sr=[]
        for k,v in rows.items():
            if k[2]==n and k[3]==a:
                kb=(k[0],k[1],n,b)
                if kb in rows:
                    rat.append(float(v['med_us'])/float(rows[kb]['med_us'])); dif.append(float(v['med_us'])-float(rows[kb]['med_us']))
                    sr.append(float(v['sect_per'])/float(rows[kb]['sect_per']))
        if not rat: continue
        r=q(rat); d=q(dif); s=q(sr)
        print(f"  {a}/{b}: t {r[0]:.2f} [{r[1]:.2f}-{r[2]:.2f}] slower in {sum(x>1 for x in rat)}/{r[3]}; diff {d[0]:+.0f} us [{d[1]:+.0f} to {d[2]:+.0f}]; sectors {s[0]:.2f} [{s[1]:.2f}-{s[2]:.2f}]")
