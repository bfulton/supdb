# T17 repair round 1's mutants: each an edit of one finished file (keep-r1, by
# its sha256), the tests it is held to, the verdict written as it comes, the
# file put back and checked by its sum after each.
import subprocess, hashlib, sys, os, time
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T17"
T = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T17"
R = "/home/user/supdb"
SUMS = dict(reversed(l.split()) for l in open(U + "/keep-r1.sha256"))
def sha(p): return hashlib.sha256(open(p, "rb").read()).hexdigest()
SETTLE = ["--test", "settle"]; BLOB = ["--test", "blob"]; LIB = ["--lib"]; DB = ["--test", "db"]
CIPHER = ["--test", "cipher"]
M = [
 ("R1_M5_refusal_dropped", "src/blob.rs",
  "            return Err(by_the_piece());\n        }\n        while seen < limit {",
  "        }\n        while seen < limit {", BLOB),
 ("R1_X1_planned_paged_arm_never", "src/ordindex.rs",
  "        if self.lim == 0 {\n            return self.seek_planned_by_piece(plan, key, key_at);",
  "        if false {\n            return self.seek_planned_by_piece(plan, key, key_at);", LIB + CIPHER),
 ("R1_X2_key_eq_no_length_guard", "src/db.rs",
  "    if a.len() != b.len() {\n        return false;\n    }\n    if a.len() >= 16 {",
  "    if a.len() >= 16 {", SETTLE),
 ("R1_X3_seek_pieces_none_answers_zero", "src/blob.rs",
  "            Some((sec, idx)) => idx.seek_with(sec, key, true),\n            None => self.keys(),",
  "            Some((sec, idx)) => idx.seek_with(sec, key, true),\n            None => 0,", BLOB),
 ("R1_X4_search_no_rank_guard", "src/flatindex.rs",
  "            let at = if mid < self.nkeys {", "            let at = if true {", BLOB + ["--test", "segwriter", "--test", "dict"] + LIB),
 ("R1_X5_bytes_kept_both_halves", "src/db.rs",
  "                    bytes_kept = !cloned;\n                } else if found {\n                    if self.fits() {\n                        fit_room(&mut blk.vals, run.len());\n                    }\n                    let at_run = blk.vals.len() as u32;\n                    blk.vals.extend_from_slice(run);\n                    blk.dead += blk.ents[i][3] as usize;\n",
  "                    bytes_kept = true;\n                } else if found {\n                    if self.fits() {\n                        fit_room(&mut blk.vals, run.len());\n                    }\n                    let at_run = blk.vals.len() as u32;\n                    blk.vals.extend_from_slice(run);\n                    blk.dead += blk.ents[i][3] as usize;\n                    bytes_kept = !cloned;\n",
  SETTLE + DB),
 ("R1_X6_key_eq_seventeen_unread", "src/db.rs",
  "            && (a.len() == 16 || a[16..] == b[16..])", "            && (a.len() <= 17 || a[16..] == b[16..])", SETTLE),
 ("R1_X7_rank_taken_on_full_blocks", "src/db.rs",
  "                    && (blk.identity == table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));",
  "                    && (blk.identity == table.blob\n                        || key_eq(blk.key(&blk.ents[c - lo]), key)\n                        || blk.ents.len() == CACHE_BLOCK);", SETTLE + DB),
 ("R1_X8_planned_always_by_piece", "src/ordindex.rs",
  "        if self.lim == 0 {\n            return self.seek_planned_by_piece(plan, key, key_at);",
  "        if true {\n            return self.seek_planned_by_piece(plan, key, key_at);", LIB + SETTLE),
 ("R1_X9_bytes_kept_after_the_match", "src/db.rs",
  "                    bytes_kept = !cloned;\n                } else if found {",
  "                } else if found {", SETTLE + DB),
 ("R1_O1_plan_late_never", "src/ordindex.rs",
  "        if self.top_eager.is_empty() {\n            return self.plan_seek_late(key, t);",
  "        if false {\n            return self.plan_seek_late(key, t);", LIB + SETTLE),
 ("R1_O2_plan_late_empty_index_plans", "src/ordindex.rs",
  "    fn plan_seek_late(&self, key: &[u8], t: &mut usize) -> Option<SeekPlan> {\n        if self.n == 0 {\n            return None;\n        }",
  "    fn plan_seek_late(&self, key: &[u8], t: &mut usize) -> Option<SeekPlan> {", LIB + SETTLE),
 ("R1_O3_held_tie_never", "src/ordindex.rs",
  "            Some(x) if u64::from_be_bytes(*x) == h => self.exact_tied(lo, h, key, key_at),",
  "            Some(x) if u64::from_be_bytes(*x) == h && false => self.exact_tied(lo, h, key, key_at),", LIB + SETTLE),
 ("R1_O4_unchecked_probe_le", "src/ordindex.rs",
  "                u64::from_be_bytes(*unsafe { stride.get_unchecked(i) }) < h",
  "                u64::from_be_bytes(*unsafe { stride.get_unchecked(i) }) <= h", LIB + SETTLE),
 ("R1_O5_stride_cut_past_hi", "src/ordindex.rs",
  "        let stride = &heads[base.min(hi)..hi];", "        let stride = &heads[base.min(hi)..heads.len()];", LIB + SETTLE),
 ("R1_O6_key_at_pieces_none", "src/blob.rs",
  "        let (sec, idx) = self.pieces()?;\n        idx.key_at(sec, rank)", "        let _ = self.pieces()?;\n        None", BLOB),
 ("R1_O7_tie_by_piece_untied", "src/ordindex.rs",
  "        self.exact_tied(lo, h, key, key_at)\n    }\n\n    /// `exact_at` where", "        (lo, self.uniform_len.map(|_| false))\n    }\n\n    /// `exact_at` where", LIB + CIPHER),
]
X9_ADD = ("        }\n        if was_clean {\n            self.list_built(at, b, table);",
          "        }\n        bytes_kept = !cloned;\n        if was_clean {\n            self.list_built(at, b, table);")
only = set(sys.argv[1:])
res = open(U + "/mutants-r1.results", "a")
for name, f, old, new, tests in M:
    if only and name not in only: continue
    path = R + "/" + f
    key = f.replace("/", "_")
    src = open(path).read()
    assert sha(path) == SUMS[key], f"{f} is not the finished copy before {name}"
    if src.count(old) != 1:
        res.write(f"{name}\tNOT APPLIED (old text found {src.count(old)} times)\n"); res.flush(); continue
    mut = src.replace(old, new)
    if name == "R1_X9_bytes_kept_after_the_match":
        assert mut.count(X9_ADD[0]) == 1; mut = mut.replace(X9_ADD[0], X9_ADD[1])
    open(path, "w").write(mut)
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
