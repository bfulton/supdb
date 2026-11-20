#!/usr/bin/env python3
"""G1-3 round 3: mutants of the lines this round wrote in the scripts and the
source test. Each edits the tree's file, runs its killer, and puts the file
back from keep4/, checked by its sum, before the next. Verdicts to
mutants4.results as they come.  mutants4.py NAME... (or none: all)"""
import hashlib, os, shutil, subprocess, sys
REPO = "/home/user/supdb"
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3"
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3"
RES = f"{U}/mutants4.results"
RED_A = ["sh", "scripts/cost-red.sh"]
SRC = ["cargo", "test", "--profile", "checked", "--test", "source", "a_new_entry_point_on_no_line_fails"]
M = {
 "P1_bounds_ignored": ("scripts/cost-diff.py", "        if bs:\n            own_bound[f] = max(bs) * ops\n", "        if bs:\n            pass\n", RED_A),
 "P2_min_bound": ("scripts/cost-diff.py", "own_bound[f] = max(bs) * ops", "own_bound[f] = min(bs) * ops", RED_A),
 "P3_scope_ignored": ("scripts/cost-diff.py", 'if (mode == "whole" if probes is None else name in probes.split(",")):', 'if (mode == "whole" if probes is None else True):', RED_A),
 "P4_unscoped_in_toggle": ("scripts/cost-diff.py", 'if (mode == "whole" if probes is None else name in probes.split(",")):', 'if (True if probes is None else name in probes.split(",")):', RED_A),
 "P5_no_bound_taken": ("scripts/cost-diff.py", '            print(f"cost: the `~` line `{l}` names no bound an operation")\n            sys.exit(2)\n', '            continue\n', RED_A),
 "P6_scoped_not_in_toggle": ("scripts/cost-diff.py", 'if (mode == "whole" if probes is None else name in probes.split(",")):', 'if (mode == "whole" if probes is None else mode == "whole" and name in probes.split(",")):', RED_A),
 "Q1_alias_ignored": ("tests/source.rs", "                aliases.get(&name).cloned().unwrap_or(name)\n", "                name\n", SRC),
 "Q2_blanket_ignored": ("tests/source.rs", "            let name = if generics.contains(&name) {", "            let name = if false {", SRC),
 "Q2b_blanket_ignored_v2": ("tests/source.rs", "            let name = if generics.contains(&name) {", "            let name = if false {", SRC),
 "Q3_alias_chain_one_step": ("tests/source.rs", "        for _ in 0..8 {", "        for _ in 0..1 {", SRC),
 "Q4_alias_of_any_type": ("tests/source.rs", "        if ENTRY_TYPES.contains(&t.as_str()) && alias != &t {", "        if alias != &t {", SRC),
 "Q5_blanket_inherent": ("tests/source.rs", "            let name = if tr.is_some() && generics.contains(&name) {", "            let name = if generics.contains(&name) {", SRC),
}
def keep(f): return f"{U}/keep4/{f.replace('/', '__')}"
def restore(f):
    shutil.copy(keep(f), f"{REPO}/{f}")
    want = hashlib.sha256(open(keep(f), "rb").read()).hexdigest()
    got = hashlib.sha256(open(f"{REPO}/{f}", "rb").read()).hexdigest()
    assert want == got, f
def note(l):
    open(RES, "a").write(l + "\n"); print(l, flush=True)
for n in sys.argv[1:] or list(M):
    f, old, new, cmd = M[n]
    s = open(f"{REPO}/{f}").read()
    if s.count(old) != 1:
        note(f"{n}\tNOT APPLIED ({s.count(old)})"); continue
    open(f"{REPO}/{f}", "w").write(s.replace(old, new))
    env = dict(os.environ, TMPDIR=TMP, COST_RED_STEPS="A")
    try:
        r = subprocess.run(cmd, cwd=REPO, env=env, capture_output=True, text=True, timeout=3600)
        out = r.stdout + r.stderr
        open(f"{U}/mutant4-{n}.log", "w").write(out)
        killed = r.returncode != 0
        why = [l for l in out.splitlines() if l.startswith("cost-red:") or "panicked" in l or "test result" in l][:3]
        note(f"{n}\t{'KILLED' if killed else 'SURVIVED'}\texit {r.returncode}\t{why}")
    finally:
        restore(f)
