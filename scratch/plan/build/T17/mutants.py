# T17's mutants: each an edit of one finished file, the tests it is held to,
# the verdict written as it comes, and the file put back by its sum after.
import subprocess, hashlib, sys, os, json, time
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T17"
T = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T17"
R = "/home/user/supdb"
KEEP = {"src/blob.rs": "blob.rs", "src/db.rs": "db.rs", "src/flatindex.rs": "flatindex.rs",
        "src/ordindex.rs": "ordindex.rs", "tests/settle.rs": "settle.rs"}
SUMS = dict(reversed(l.split()) for l in open(U + "/keep.sha256"))
def sha(p): return hashlib.sha256(open(p, "rb").read()).hexdigest()
SETTLE = ["--test", "settle"]
BLOB = ["--test", "blob"]
M = [
 ("M1_claim_taken_without_proof", "src/db.rs",
  "                    && (blk.identity == table.blob\n                        || key_eq(blk.key(&blk.ents[c - lo]), key));",
  "                    && (blk.identity == table.blob || true);", SETTLE),
 ("M2_compare_inverted", "src/db.rs",
  "                        || key_eq(blk.key(&blk.ents[c - lo]), key));",
  "                        || !key_eq(blk.key(&blk.ents[c - lo]), key));", SETTLE),
 ("M3_search_fallback_dropped", "src/db.rs",
  "                } else {\n                    let i = blk.lower_bound(key);",
  "                } else {\n                    let i = (c.saturating_sub(lo)).min(blk.ents.len());", SETTLE),
 ("M4_scan_start_claim_without_proof", "src/db.rs",
  "                                || blk.starts_at((start - lo).min(blk.ents.len()), cursor));",
  "                                || true);", SETTLE + ["--test", "db"]),
 ("M5_scan_piecewise_refusal_dropped", "src/blob.rs",
  "            return Err(by_the_piece());\n        }\n        while seen < limit {",
  "        }\n        while seen < limit {", BLOB + ["--lib"]),
 ("M6_seek_pieces_answers_the_end", "src/blob.rs",
  "        match self.pieces() {\n            Some((sec, idx)) => idx.seek_with(sec, key, true),\n            None => self.keys(),\n        }",
  "        self.keys()", BLOB),
 ("M7_rank_not_moved_by_the_fast_walk", "src/blob.rs",
  "        $rank += $seen - walked_from;\n", "        let _ = walked_from;\n", BLOB + ["--test", "dict"]),
 ("M8_search_reads_the_next_word", "src/flatindex.rs",
  "                dir.u32_at(mid * 4).and_then(|off| recs.key(off as usize))",
  "                dir.u32_at(mid * 4 + 4).and_then(|off| recs.key(off as usize))", BLOB),
 ("M9_search_unresolved_answers_hi", "src/flatindex.rs",
  "        ) else {\n            return lo;\n        };\n        while lo < hi {",
  "        ) else {\n            return hi;\n        };\n        while lo < hi {", BLOB + ["--test", "segwriter"]),
 ("M10_planned_heads_le", "src/ordindex.rs",
  "                u64::from_be_bytes(heads[base + i]) < h",
  "                u64::from_be_bytes(heads[base + i]) <= h", SETTLE + ["--lib"]),
 ("M11_bytes_kept_on_an_appended_run", "src/db.rs",
  "                    blk.dead += blk.ents[i][3] as usize;\n",
  "                    blk.dead += blk.ents[i][3] as usize;\n                    bytes_kept = !cloned;\n", SETTLE + ["--test", "db"]),
 ("M12_bytes_kept_ignores_clone", "src/db.rs",
  "                    bytes_kept = !cloned;", "                    bytes_kept = true;", SETTLE + ["--test", "db"]),
 ("M13_starts_at_takes_an_equal_key_as_below", "src/db.rs",
  "                .is_none_or(|e| key_cmp(self.key(e), from) != Ordering::Less)",
  "                .is_none_or(|e| key_cmp(self.key(e), from) == Ordering::Greater)", SETTLE + ["--test", "db"]),
 ("M14_key_eq_second_word_unread", "src/db.rs",
  "        word(a, 0) == word(b, 0) && word(a, 8) == word(b, 8) && (a.len() == 16 || a[16..] == b[16..])",
  "        word(a, 0) == word(b, 0) && (a.len() == 16 || a[16..] == b[16..])", SETTLE),
 ("M15_key_eq_tail_unread", "src/db.rs",
  "        word(a, 0) == word(b, 0) && word(a, 8) == word(b, 8) && (a.len() == 16 || a[16..] == b[16..])",
  "        word(a, 0) == word(b, 0) && word(a, 8) == word(b, 8)", SETTLE),
 ("M4b_scan_start_takes_a_stale_claim", "src/db.rs",
  "                            && blk.identity == seg.blob.id()\n                            && rank != usize::MAX;",
  "                            && blk.identity != 0\n                            && rank != usize::MAX;", SETTLE + ["--test", "db"]),
 ("M16_plan_seek_empty_top_taken_late", "src/ordindex.rs",
  "        let top = if !self.top_eager.is_empty() {", "        let top = if false {", SETTLE + ["--lib"]),
]
only = set(sys.argv[1:])
res = open(U + "/mutants.results", "a")
for name, f, old, new, tests in M:
    if only and name not in only: continue
    path = R + "/" + f
    src = open(path).read()
    assert sha(path) == SUMS[KEEP[f]], f"{f} is not the finished copy before {name}"
    if src.count(old) != 1:
        res.write(f"{name}\tNOT APPLIED (old text found {src.count(old)} times)\n"); res.flush(); continue
    open(path, "w").write(src.replace(old, new))
    t0 = time.time()
    env = dict(os.environ, TMPDIR=T, RUST_BACKTRACE="0")
    log = open(f"{U}/mutant-{name}.log", "w")
    p = subprocess.run(["timeout", "1500", "cargo", "test", "--profile", "checked", "--no-fail-fast"] + tests,
                       cwd=R, env=env, stdout=log, stderr=subprocess.STDOUT)
    log.close()
    open(path, "w").write(src)
    assert sha(path) == SUMS[KEEP[f]], f"{f} not put back after {name}"
    out = open(f"{U}/mutant-{name}.log").read()
    built = "error[" not in out and "could not compile" not in out
    failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
    verdict = ("NOT BUILT" if not built else ("KILLED by " + ",".join(failed) if failed else
               ("KILLED (exit %d, no test named)" % p.returncode if p.returncode else "SURVIVED")))
    res.write(f"{name}\t{verdict}\t{int(time.time()-t0)}s\n"); res.flush()
print(open(U + "/mutants.results").read())
