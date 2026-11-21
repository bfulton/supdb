# A count mutant: an edit that changes no answer, applied to the finished
# file, scripts/cost.sh BASE land run over it, the file put back by its sum.
import subprocess, hashlib, os, sys
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T17"
R = "/home/user/supdb"
SUMS = dict(reversed(l.split()) for l in open(U + "/keep-r1.sha256"))
def sha(p): return hashlib.sha256(open(p, "rb").read()).hexdigest()
M = [
 ("R1_X8_planned_always_by_piece", "src/ordindex.rs",
  "        if self.lim == 0 {\n            return self.seek_planned_by_piece(plan, key, key_at);",
  "        if true {\n            return self.seek_planned_by_piece(plan, key, key_at);"),
 ("R1_O5_stride_cut_past_hi", "src/ordindex.rs",
  "        let stride = &heads[base.min(hi)..hi];", "        let stride = &heads[base.min(hi)..heads.len()];"),
]
res = open(U + "/mutants-r1.results", "a")
for name, f, old, new in M:
    path = R + "/" + f; key = f.replace("/", "_")
    src = open(path).read()
    assert sha(path) == SUMS[key]
    assert src.count(old) == 1
    open(path, "w").write(src.replace(old, new))
    env = dict(os.environ, COST_DIR=f"{U}/r1/cm-{name}", COST_BASE_TARGET=f"{U}/tb")
    log = open(f"{U}/r1/cost-{name}.log", "w")
    p = subprocess.run(["sh", "scripts/cost.sh", "9bfc4362", "land"], cwd=R, env=env, stdout=log, stderr=subprocess.STDOUT)
    log.close()
    open(path, "w").write(src)
    assert sha(path) == SUMS[key], "not put back"
    last = open(f"{U}/r1/cost-{name}.log").read().strip().splitlines()[-1]
    res.write(f"{name}\tcost.sh land exit {p.returncode}: {last}\n"); res.flush()
