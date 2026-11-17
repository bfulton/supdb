#!/usr/bin/env python3
# Every 5 s: for each store directory under ROOT/supdb-ab-*/*, apparent and
# allocated bytes by file kind, on CLOCK_MONOTONIC. Every 30 s, for a store with
# no x- files (a clear store), runs scripts/resident.py over it (mincore, faults
# nothing in). Reads directory entries and stat only.  usage: storesize.py ROOT OUT
import glob, os, re, subprocess, sys, time
from collections import defaultdict
ROOT, OUT = sys.argv[1], sys.argv[2]
RES = "/home/user/supdb/scripts/resident.py"
def kind(name):
    if name.startswith("x-"):
        return "x" + (os.path.splitext(name)[1] or ".seg")
    if name.endswith(".zpg") or "zpg" in name:
        return "form"
    m = re.match(r"([a-z]+)", name)
    k = m.group(1) if m else name
    if name.endswith(".ord") or "ord" in name.split(".")[-1]:
        return k + ".ord"
    return k
last_res = {}
with open(OUT, "a", buffering=1) as out:
    while True:
        now = time.monotonic()
        for d in glob.glob(os.path.join(ROOT, "supdb-ab-*", "*")):
            if not os.path.isdir(d):
                continue
            tot = defaultdict(lambda: [0, 0, 0])
            anyx = False
            try:
                names = os.listdir(d)
            except OSError:
                continue
            for f in names:
                try:
                    st = os.stat(os.path.join(d, f))
                except OSError:
                    continue
                if not os.path.isfile(os.path.join(d, f)):
                    continue
                k = kind(f); anyx |= f.startswith("x-")
                t = tot[k]; t[0] += 1; t[1] += st.st_size; t[2] += st.st_blocks * 512
            segb = sum(v[1] for k, v in tot.items() if k in ("seg", "par", "pcs"))
            ordb = sum(v[1] for k, v in tot.items() if k.endswith(".ord"))
            line = " ".join(f"{k}:{v[0]}f:{v[1]}:{v[2]}" for k, v in sorted(tot.items()))
            out.write(f"{now:.3f} size {os.path.basename(d)} segbytes {segb} ordbytes {ordb} | {line}\n")
            if not anyx and now - last_res.get(d, 0) >= 30:
                last_res[d] = now
                try:
                    r = subprocess.run(["python3", "-I", RES, d, os.path.basename(d)], capture_output=True, text=True, timeout=60)
                    for l in r.stdout.splitlines():
                        out.write(f"{time.monotonic():.3f} {l}\n")
                except Exception as e:
                    out.write(f"{now:.3f} resident-error {e}\n")
        time.sleep(5)
