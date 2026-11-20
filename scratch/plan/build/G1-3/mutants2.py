#!/usr/bin/env python3
"""G1-3 mutants: each applies one edit, runs its killer, records the verdict
as it gets it, and puts the file back from keep/ checked by its sum.

    mutants.py NAME...   (or no names: all)
"""
import hashlib, os, shutil, subprocess, sys, time

REPO = "/home/user/supdb"
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3"
KEEP = f"{U}/keep2"
RES = f"{U}/mutants2.results"
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3"
RT = f"{U}/rt"  # the red script's shared target, kept across runs

def red(steps):
    env = dict(os.environ, TMPDIR=TMP, COST_RED_TARGET=RT, COST_RED_STEPS=steps,
               COST_RED_DIR=f"{TMP}/red")
    return (["sh", "scripts/cost-red.sh"], env)

def srctest(name):
    env = dict(os.environ, TMPDIR=TMP)
    return (["cargo", "test", "--profile", "checked", "--test", "source", "--", "--exact", name], env)

# name: (file, old, new, killer, the line a kill must print)
M = {
  # The plan's four and the round before's, against the scripts as they are now.
  "M1_totals": ("scripts/cost-diff.py",
      "    bad = sorted(f for f in hot if (h.get(f, 0) - b.get(f, 0)) / ops > slack)",
      "    bad = [\"TOTAL\"] if (th - tb) / ops > slack else []",
      red("A"), "cost-red: A: "),
  "M2a_hotfile_empty": ("scripts/cost-hot.txt", None, "# emptied\n", red("S"), "cost-red: S: the job exited"),
  "M2b_hotlist_silent": ("scripts/cost-diff.py",
      "        sys.exit(2)\n    return fns, out",
      "        sys.exit(2)\n    return [], out",
      red("A"), "cost-red: A: "),
  "M3_base_sources": ("scripts/cost.sh",
      '    cp "$HEAD_TREE/bench/examples/$e.rs" "$W/base/bench/examples/$e.rs"',
      '    :',
      red("C"), "cost-red: C: the job exited"),
  "M4_exit_swallowed": ("scripts/cost.sh",
      '"$ROOT/scripts/cost-hot.txt" "$p" "$own" || status=1',
      '"$ROOT/scripts/cost-hot.txt" "$p" "$own" || true',
      red("B"), "cost-red: B: the job exited"),
  "O1_no_empty_check": ("scripts/cost-diff.py", "        if t < ops:", "        if False:",
      red("A"), "cost-red: A: a side that collected nothing"),
  "O2_head_ignored": ("scripts/cost.sh", "HEAD_TREE=${COST_HEAD:-$ROOT}", "HEAD_TREE=$ROOT",
      red("B"), "cost-red: B: the job exited"),
  "O3_toggle_dropped": ("scripts/cost.sh", '    if [ -n "$tog" ]; then', '    if false; then',
      red("T"), "cost-red: T: the job exited"),
  "O4_verdict_green": ("scripts/cost.sh", 'if [ "$status" -ne 0 ]; then', 'if false; then',
      red("B"), "cost-red: B: the job exited"),
  "O5_test_code_counted": ("tests/source.rs", "&& !lines[line_of[k]].test\n", "\n",
      srctest("a_new_entry_point_on_no_line_fails"), "test result: FAILED"),
  "O6_default_unchecked": ("tests/source.rs", "            } else if !probes.1.contains(p) {", "            } else if false {",
      srctest("a_line_the_tree_or_the_job_does_not_have_fails"), "test result: FAILED"),
  "O7_missing_line_unchecked": ("tests/source.rs", "        if !listed.contains_key(f) {", "        if false {",
      srctest("a_new_entry_point_on_no_line_fails"), "test result: FAILED"),
  "O9_substring_match": ("scripts/cost-diff.py", "    return c == line or c.startswith(line + \"::\")",
      "    return line in c", red("A"), "cost-red: A: "),
  "O10_raw_names": ("scripts/cost-diff.py", "    b, h = by_canon(rb, own), by_canon(rh, own)",
      "    b, h = rb, rh", red("A"), "cost-red: A: "),
  "O11_bang_ignored": ("scripts/cost-diff.py", "if not any(matches(o, f) for o in outfns)}", "}",
      red("A"), "cost-red: A: "),
  "O12_no_hot_unchecked": ("scripts/cost-diff.py", "    if not hot:", "    if False:",
      red("A"), "cost-red: A: a probe in which no hot line"),
  # The reviewers' extra mutants on scripts and tests.
  "X3_entry_files_two": ("tests/source.rs", '        if !file.starts_with("src/") || !file.ends_with(".rs") {',
      '        if file != "src/db.rs" && file != "src/blob.rs" {',
      srctest("a_new_entry_point_on_no_line_fails"), "test result: FAILED"),
  "X5_group_swallows_cost": ("scripts/check.sh", '${COST_PROBES:-} || exit 1', '${COST_PROBES:-} || true',
      red("G"), "cost-red: G: the group exited"),
  "X9_slack_5": ("scripts/cost-diff.py", "DEFAULT_SLACK = 0.05", "DEFAULT_SLACK = 5",
      red("A"), "cost-red: A: a tenth of an instruction"),
  "X9b_slack_0": ("scripts/cost-diff.py", "DEFAULT_SLACK = 0.05", "DEFAULT_SLACK = 0.0",
      red("A"), "cost-red: A: a hundredth of an instruction"),
  "X10_threshold_dropped": ("scripts/cost.sh", "--inclusive=no --threshold=100 ", "--inclusive=no ",
      red("S"), "cost-red: S: the job exited"),
  # Mine, on the lines this round wrote.
  "R1_own_not_passed": ("scripts/cost.sh", '        own="supdb::,supdb_bench::,$(echo "$spec" | cut -d\'|\' -f3)::"', '        own=""',
      red("U"), "cost-red: U: the job exited"),
  "R2_partial_sum_unchecked": ("scripts/cost-diff.py", "        elif sum(rows.values()) != total:", "        elif False:",
      red("A"), "cost-red: A: a listing whose functions fall short"),
  "R3_no_total_unchecked": ("scripts/cost-diff.py", "        if total is None:", "        if False:",
      red("A"), "cost-red: A: a listing with no program total"),
  "R4_suite_guard_dropped": ("scripts/cost.sh", 'if [ -n "$SUITE" ] && ! python3', 'if false && ! python3',
      red("V"), "cost-red: V: the job exited"),
  "R5_over_key_dropped": ("scripts/cost-diff.py", "        return c + OVER", "        return c",
      red("A"), "cost-red: A: in a probe with a toggle, a library function"),
  "R6_owned_ignored": ("scripts/cost-diff.py", "    owned = {f for f in fns if own and (f.startswith(own) or f.endswith(OVER))}", "    owned = set()",
      red("A"), "cost-red: A: in a probe with a toggle, a function of the engine"),
  "R7_group_swallows_red": ("scripts/check.sh", "      sh scripts/cost-red.sh || exit 1", "      sh scripts/cost-red.sh || true",
      red("G"), "cost-red: G: "),
  "R8_suite_attrs_ignored": ("scripts/cost.sh", '    while at > 0 and lines[at - 1].strip().startswith("#["):\n        at -= 1\n', '',
      red("V"), "cost-red: V: the job exited"),
}

def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()

def restore(f):
    shutil.copyfile(f"{KEEP}/{f.replace('/', '__')}", f"{REPO}/{f}")
    want = open(f"{KEEP}/SHA256SUMS").read()
    if sha(f"{REPO}/{f}") not in want:
        sys.exit(f"RESTORE FAILED for {f}")

def note(line):
    with open(RES, "a") as r:
        r.write(line + "\n")
    print(line, flush=True)

def main():
    names = sys.argv[1:] or [n for n in M if M[n][3]]
    for n in names:
        f, old, new, killer, mark = M[n]
        p = f"{REPO}/{f}"
        s = open(p).read()
        if old is None:
            s2 = new
        else:
            if s.count(old) != 1:
                note(f"{n}\tNOT APPLIED ({s.count(old)} occurrences)")
                continue
            s2 = s.replace(old, new)
        open(p, "w").write(s2)
        t = time.time()
        try:
            cmd, env = killer
            out = subprocess.run(cmd, cwd=REPO, env=env, capture_output=True, text=True)
            text = out.stdout + out.stderr
            open(f"{U}/mutant2-{n}.log", "w").write(text)
            killed = out.returncode != 0 and mark in text
            verdict = "KILLED" if killed else f"SURVIVED (exit {out.returncode})"
            last = [l for l in text.splitlines() if l.startswith("cost-red:") or "test result" in l]
            note(f"{n}\t{verdict}\t{int(time.time() - t)}s\t{last[-1] if last else ''}")
        finally:
            restore(f)

if __name__ == "__main__":
    main()
