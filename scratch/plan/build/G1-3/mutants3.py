#!/usr/bin/env python3
"""G1-3 round 2: mutants of the lines this round wrote. Each edit is made in
the tree, its killer run, the verdict appended to mutants3.results, and the
file put back from keep3/ and checked by its sum."""
import hashlib, os, subprocess, sys
REPO = "/home/user/supdb"
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3"
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3"
RES = f"{U}/mutants3.results"
A = ["sh", "scripts/cost-red.sh"]
SRC = ["cargo", "test", "--profile", "checked", "--test", "source", "a_new_entry_point"]
D = "scripts/cost-diff.py"
M = {
 "N1_toggle_slack_200": (D, "TOGGLE_SLACK = 64 ", "TOGGLE_SLACK = 200 ", A, "A"),
 "N2_whole_slack_002": (D, "WHOLE_SLACK = 0.01 ", "WHOLE_SLACK = 0.02 ", A, "A"),
 "N3_bare_name_dropped": (D, 'f.startswith(own) or ("::" not in f and any(workspace_file(x) for x in files[f]))', "f.startswith(own)", A, "A"),
 "N4_libc_as_source": (D, 'return f.startswith(("./", "/usr/", "???"))', 'return f.startswith(("/usr/", "???"))', A, "A"),
 "N5_lib_rule_or": (D, "gain[f] > LIB_SLACK * ops and gain[f] > LIB_REL", "gain[f] > LIB_SLACK * ops or gain[f] > LIB_REL", A, "A"),
 "N6_lib_not_hot": (D, '    libhot = {f for f in lib - exact if not any(matches(o, f) for o in outfns)} if mode == "toggle" else set()', "    libhot = set()", A, "A"),
 "N7_tilde_in_toggle": (D, '    if mode == "whole":\n        outfns = outfns + timedfns', "    outfns = outfns + timedfns", A, "A"),
 "N8_scoped_tilde_everywhere": (D, "        elif name in m.group(1).split(\",\"):", "        else:", A, "A"),
 "N9_unknown_mode_taken": (D, 'if len(argv) != 8 or argv[6] not in ("toggle", "whole"):', "if len(argv) != 8:", A, "A"),
 "N10_trait_ignored": ("tests/source.rs", "            let tr = impl_trait(&s, at, open);", "            let tr: Option<String> = None;", SRC, None),
 "N11_macro_ignored": ("tests/source.rs", "                        if s.get(after) == Some(&'!')", "                        if false && s.get(after) == Some(&'!')", SRC, None),
 "N12_bad_ref_unchecked": ("scripts/cost.sh", 'if ! git -C "$ROOT" rev-parse --verify --quiet "$BASE^{commit}" >/dev/null; then', 'if false; then', A, "G"),
 "N13_no_new_fn_in_whole": (D, "    exact = {f for f in exact | listed if not", "    exact = {f for f in exact | listed if f in b and not", A, "A"),
}
def sha(p): return hashlib.sha256(open(p, "rb").read()).hexdigest()
def keep(f): return f"{U}/keep3/" + f.replace("/", "__")
def note(l):
    open(RES, "a").write(l + "\n"); print(l, flush=True)
for n in sys.argv[1:] or list(M):
    f, old, new, cmd, steps = M[n]
    p = f"{REPO}/{f}"
    s = open(p).read()
    if s.count(old) != 1:
        note(f"{n}\tNOT APPLIED ({s.count(old)})"); continue
    open(p, "w").write(s.replace(old, new))
    env = dict(os.environ, TMPDIR=TMP)
    if steps: env["COST_RED_STEPS"] = steps
    r = subprocess.run(cmd, cwd=REPO, env=env, capture_output=True, text=True)
    out = r.stdout + r.stderr
    open(f"{U}/mutant3-{n}.log", "w").write(out)
    open(p, "w").write(open(keep(f)).read())
    ok = sha(p) == sha(keep(f))
    why = [l for l in out.splitlines() if l.startswith("cost-red: ") and "ok" not in l or "panicked" in l or "assert" in l][:2]
    note(f"{n}\t{'KILLED' if r.returncode != 0 else 'SURVIVED'}\trestored={ok}\t{why}")
