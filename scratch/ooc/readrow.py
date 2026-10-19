# Read a bounded row: per workload and arm, the rate and the counts per operation beside it.
#   python3 -I readrow.py <row.json> [workload-prefix...]
import json, sys, statistics, collections
r = json.load(open(sys.argv[1])); pref = sys.argv[2:] 
by = collections.defaultdict(dict)
for m in r["measurements"]:
    if pref and not any(m["workload"].startswith(p) for p in pref): continue
    by[(m["workload"], m["arm"])][m["quantity"]] = statistics.median(m["samples"])
print(f"row {r['utc']} {r['sha']} scale {r['scale']} class {r['machine'].get('mem_limit_bytes','-')}")
rates = ("ops_per_s","reads_per_s","entries_per_s")
for (w, arm), q in sorted(by.items()):
    rate = [(k, v) for k, v in q.items() if any(k.startswith(x) for x in rates) and "lag" not in k and "_2t" not in k and "_3t" not in k]
    lag = [(k, v) for k, v in q.items() if "lag" in k and any(k.startswith(x) for x in ("entries_per_s","updates_per_s"))]
    cnt = {k.split("_per_op")[0]: v for k, v in q.items() if "_per_op" in k}
    rs = " ".join(f"{k.split('_per_s')[0]}={v:.3g}" for k, v in rate + lag)
    cs = " ".join(f"{k}={v:.3g}" for k, v in sorted(cnt.items()))
    print(f"  {w:14s} {arm:16s} {rs}")
    if cs: print(f"  {'':14s} {'':16s} {cs}")
