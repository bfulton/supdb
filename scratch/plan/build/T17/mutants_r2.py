# T17 repair round 2's mutants: each an edit of one finished file (keep-r2, by
# its sha256), the tests it is held to, the verdict written as it comes, the
# file put back and checked by its sum after each.
import subprocess, hashlib, sys, os, time
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T17"
T = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T17"
R = "/home/user/supdb"
SUMS = dict(reversed(l.split()) for l in open(U + "/keep-r2.sha256"))
def sha(p): return hashlib.sha256(open(p, "rb").read()).hexdigest()
SETTLE = ["--test", "settle"]; BLOB = ["--test", "blob"]; LIB = ["--lib"]; DB = ["--test", "db"]
CIPHER = ["--test", "cipher"]
M = [
 ("R2_A_rank_by_cumulative_seen", "src/blob.rs",
  "        $rank += $seen - walked_from;", "        $rank += $seen;", BLOB + CIPHER + LIB),
 ("R2_B_window_rank_from_first_window", "src/blob.rs",
  "            if seen - before < n {\n                break;\n            }\n        }\n        self.scan_records(seen, rank, limit, f, cached)",
  "            rank += before;\n            if seen - before < n {\n                break;\n            }\n        }\n        self.scan_records(seen, rank, limit, f, cached)", BLOB + CIPHER + LIB),
 ("R2_C_own_claim_inverted", "src/db.rs",
  "                    && (blk.identity == table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));",
  "                    && (blk.identity != table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));", SETTLE),
 ("R2_D_tie_reads_head_before", "src/ordindex.rs",
  "        match heads.get(lo) {\n            Some(x) if u64::from_be_bytes(*x) == h",
  "        match heads.get(lo.saturating_sub(1)) {\n            Some(x) if u64::from_be_bytes(*x) == h", LIB + SETTLE),
 ("R2_E_plan_late_by_n", "src/ordindex.rs",
  "        if self.top_eager.is_empty() {\n            return self.plan_seek_late(key, t);",
  "        if self.n == 0 {\n            return self.plan_seek_late(key, t);", LIB + CIPHER),
 ("R2_F_stride_from_base", "src/ordindex.rs",
  "        let stride = &heads[base.min(hi)..hi];", "        let stride = &heads[base..hi.max(base)];", LIB + SETTLE),
 ("R2_G_claim_and_compare", "src/db.rs",
  "                    && (blk.identity == table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));",
  "                    && (blk.identity == table.blob && key_eq(blk.key(&blk.ents[c - lo]), key));", SETTLE + DB),
 ("R2_H_bytes_kept_in_place_always", "src/db.rs",
  "                    blk.ents[i][3] = run.len() as u32;\n                    bytes_kept = !cloned;",
  "                    blk.ents[i][3] = run.len() as u32;\n                    bytes_kept = true;", SETTLE + DB),
 ("R2_I_search_unresolved_answers_hi", "src/flatindex.rs",
  "        ) else {\n            return lo;\n        };\n        while lo < hi {",
  "        ) else {\n            return hi;\n        };\n        while lo < hi {", BLOB + ["--test", "segwriter", "--test", "dict"] + LIB),
 ("R2_A2_rank_by_cumulative_seen_blob", "src/blob.rs",
  "        $rank += $seen - walked_from;", "        $rank += $seen;", BLOB),
 ("R2_B2_window_rank_from_first_window_blob", "src/blob.rs",
  "            if seen - before < n {\n                break;\n            }\n        }\n        self.scan_records(seen, rank, limit, f, cached)",
  "            rank += before;\n            if seen - before < n {\n                break;\n            }\n        }\n        self.scan_records(seen, rank, limit, f, cached)", BLOB),
 ("R2_F2_stride_from_base_asserted", "src/ordindex.rs",
  "        let stride = &heads[base.min(hi)..hi];", "        let stride = &heads[base..hi.max(base)];", LIB + SETTLE),
 ("R2_O1_plan_order_unasserted", "src/ordindex.rs",
  "            plan.lo <= plan.hi && plan.hi as usize <= heads.len(),", "            true,", LIB),
 ("R2_O2_plan_order_half_unasserted", "src/ordindex.rs",
  "            plan.lo <= plan.hi && plan.hi as usize <= heads.len(),", "            plan.hi as usize <= heads.len(),", LIB),
 ("R2_O3_plan_end_unasserted", "src/ordindex.rs",
  "            plan.lo <= plan.hi && plan.hi as usize <= heads.len(),", "            plan.lo <= plan.hi,", LIB),
 ("R2_O4_assert_too_strict", "src/ordindex.rs",
  "            plan.lo <= plan.hi && plan.hi as usize <= heads.len(),", "            plan.lo < plan.hi && plan.hi as usize <= heads.len(),", LIB + SETTLE),
]
only = set(sys.argv[1:])
res = open(U + "/mutants-r2.results", "a")
for name, f, old, new, tests in M:
    if only and name not in only: continue
    path = R + "/" + f
    key = f.replace("/", "_")
    src = open(path).read()
    assert sha(path) == SUMS[key], f"{f} is not the finished copy before {name}"
    if src.count(old) != 1:
        res.write(f"{name}\tNOT APPLIED (old text found {src.count(old)} times)\n"); res.flush(); continue
    open(path, "w").write(src.replace(old, new))
    t0 = time.time()
    env = dict(os.environ, TMPDIR=T, RUST_BACKTRACE="0")
    log = open(f"{U}/mutant-{name}.log", "w")
    p = subprocess.run(["timeout", "1800", "cargo", "test", "--profile", "checked", "--no-fail-fast"] + tests,
                       cwd=R, env=env, stdout=log, stderr=subprocess.STDOUT)
    log.close()
    open(path, "w").write(src)
    assert sha(path) == SUMS[key], f"{f} not put back after {name}"
    txt = open(f"{U}/mutant-{name}.log").read()
    failed = [l.split()[1] for l in txt.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
    built = "error[E" not in txt and "could not compile" not in txt
    v = ("KILLED" if failed else ("BUILD FAIL" if not built else ("TIMEOUT" if p.returncode == 124 else ("KILLED(exit %d)" % p.returncode if p.returncode else "SURVIVED"))))
    res.write(f"{name}\t{v}\t{int(time.time()-t0)}s\t{' '.join(failed[:8])}\n"); res.flush()
print("done")
