#!/usr/bin/env python3
"""G1-3 round 3: the reviewers' engine mutants (and mine), each a scratch copy
of the tree with an edit, counted by scripts/cost.sh against HEAD on the
probes named; the tree is never edited. Listings kept under em3/<name>/out,
so the round-2 comparator (keep3/) can be run over the same counts: the
verdict before the repair and after, from one count. Verdicts go to
emutants3.results as they come.

    emutants3.py NAME...   (or no names: all)
"""
import os, shutil, subprocess, sys, time

REPO = "/home/user/supdb"
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3"
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3"
M = f"{TMP}/em"
RT = f"{U}/rt"
RES = f"{U}/emutants3.results"
OLD = f"{U}/keep3/scripts__cost-diff.py"
OLDHOT = f"{U}/keep3/scripts__cost-hot.txt"

LANDING = '        if self.shared.fail_landing.swap(false, AtomicOrdering::AcqRel) {\n            return Err(err("a test failed this landing"));\n        }\n'
SORT = '        let l0_aligned = sort_segs(&mut segs);\n        // One fresh table for every attempt'
ASK = '            if let Some(shared) = ask {\n                ask_bounds_heads(shared, seg, p);\n            }\n'
SWEEP = '    fn sweep_retired_states(&self) {\n'
WALK = '        self.last_ok = false;\n        self.resume();\n        let mut n = 0usize;\n        while n < limit {\n'
PW = 'fn pwritev_all(file: &File, at: u64, parts: &[&[u8]], fault: &mut u8) -> std::io::Result<()> {\n'
OPEN = '    pub fn open(dir: &Path, mut opts: Options) -> Result<Db> {\n        check_options(&opts)?;\n'
WB = '    fn write_buf_with(&mut self, hint: bool) -> std::io::Result<()> {\n        let at = self.written;\n'
FIN = '    pub fn finish(self, generation: u64) -> Result<()> {\n'
HELPER = '\n#[inline(always)]\nfn cost_mutant_per_key(n: usize) {\n    for _ in 0..n {\n        std::hint::black_box(0u32);\n    }\n}\n'
# name: ([(file, old, new)], probes, expect "red"|"green", mark)
E = {
  "F1_land_readable_per_key": ([("src/db.rs", LANDING, LANDING + "        for i in 0..table.len() {\n            std::hint::black_box(i);\n        }\n")],
      "merge load_durable load_buffered work land", "red", "HOT GAINED: supdb::db::Maint::land_readable "),
  "F2_sweep_loop8": ([("src/db.rs", SWEEP, SWEEP + "        for i in 0..8u32 {\n            std::hint::black_box(i);\n        }\n")],
      "merge work load_durable land", "red", "HOT GAINED: supdb::db::Reader::sweep_retired_states "),
  "F5_long_scan_form": ([("src/db/zfile.rs", WALK, WALK + "            if n > 1024 {\n                std::hint::black_box(&self.rank);\n            }\n")],
      "scan_long_z scan_z", "red", "HOT GAINED: supdb::db::zfile::"),
  "F6_publish_per_block": ([("src/db.rs", SORT, "        for s in &segs {\n            for i in 0..s.blob.keys() / 64 {\n                std::hint::black_box(i);\n            }\n        }\n" + SORT)],
      "merge load_durable land", "red", "HOT GAINED: supdb::db::Maint::publish_segs_with "),
  "F7_bounds_helper_per_key": ([("src/db.rs", ASK, ASK + "            cost_mutant_per_key(p.blob.keys());\n"),
                                ("src/db.rs", "\nfn bound_pieces_of(", HELPER + "\nfn bound_pieces_of(")],
      "merge work land", "red", "HOT GAINED: "),
  "F8_stream_per_piece_unit": ([("src/db.rs", PW, PW + "    let total: usize = parts.iter().map(|p| p.len()).sum();\n    for _ in 0..total / 16384 {\n        std::hint::black_box(());\n    }\n")],
      "load_durable load_buffered commit_durable commit_buffered", "red", "HOT GAINED: "),
  "F8b_stream_per_piece_u32": ([("src/db.rs", PW, PW + "    let total: usize = parts.iter().map(|p| p.len()).sum();\n    for i in 0..total / 16384 {\n        std::hint::black_box(i);\n    }\n")],
      "load_durable load_buffered commit_durable commit_buffered", "red", "HOT GAINED: "),
  "F9_open_fixed_1500": ([("src/db.rs", OPEN, OPEN + "        for i in 0..300u32 {\n            std::hint::black_box(i);\n        }\n")],
      "work merge", "green", "cost: green"),
  "F10_segment_fixed_40": ([("src/db.rs", WB, WB + "        if at == 0 {\n            for i in 0..10u32 {\n                std::hint::black_box(i);\n            }\n        }\n")],
      "seal", "red", "HOT GAINED: supdb::db::AlignedWriter::write_buf_with "),
  "F10b_segment_finish_fixed_40": ([("src/db.rs", FIN, FIN + "        for i in 0..10u32 {\n            std::hint::black_box(i);\n        }\n")],
      "seal", "red", "HOT GAINED: "),
  "F10c_segment_finish_fixed_40x": ([("src/db.rs", FIN, FIN + "        for i in 0..40u32 {\n            std::hint::black_box(i);\n        }\n")],
      "seal", "red", "HOT GAINED: "),
}

def copy(dest):
    shutil.rmtree(dest, ignore_errors=True)
    os.makedirs(dest)
    files = subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "--", ".", ":!scratch", ":!bench/runs", ":!docs/figures"],
                           cwd=REPO, capture_output=True, text=True, check=True).stdout
    subprocess.run(f"tar -cf - -T - | (cd {dest} && tar -xf -)", shell=True, cwd=REPO, input=files, text=True, check=True)

def note(line):
    with open(RES, "a") as r:
        r.write(line + "\n")
    print(line, flush=True)

SPEC = {"merge": ("200000", "merge_walk", False), "load_durable": ("100000", "load_only", False),
        "load_buffered": ("100000", "load_only", False), "work": ("300000", "work_probe", False),
        "seal": ("100000", "seal_probe", True), "scan_z": ("1000", "lag0_probe", True)}

def old_verdicts(w, probes):
    """The round-2 comparator over the same listings, for the probes it had."""
    out = []
    for p in probes:
        if p not in SPEC:
            out.append(f"{p}: no such probe then")
            continue
        ops, ex, tog = SPEC[p]
        r = subprocess.run(["python3", OLD, f"{w}/out/base.{p}.self", f"{w}/out/head.{p}.self", ops, OLDHOT, p,
                            "toggle" if tog else "whole", f"supdb::,supdb_bench::,{ex}::"], capture_output=True, text=True)
        gained = [l.strip() for l in r.stdout.splitlines() if "GAINED" in l]
        out.append(f"{p}: exit {r.returncode} {gained}")
    return "; ".join(out)

def main():
    names = sys.argv[1:] or list(E)
    for n in names:
        edits, probes, expect, mark = E[n]
        h = f"{M}/h"
        copy(h)
        ok = True
        for f, old, new in edits:
            p = f"{h}/{f}"
            s = open(p).read()
            if s.count(old) != 1:
                note(f"{n}\tNOT APPLIED ({f}: {s.count(old)} occurrences)")
                ok = False
                break
            open(p, "w").write(s.replace(old, new))
        if not ok:
            continue
        env = dict(os.environ, TMPDIR=TMP, COST_HEAD=h, COST_HEAD_TARGET=RT, COST_BASE_TARGET=RT, COST_DIR=f"{M}/w")
        t = time.time()
        out = subprocess.run(["sh", "scripts/cost.sh", "HEAD"] + probes.split(), cwd=REPO, env=env, capture_output=True, text=True)
        text = out.stdout + out.stderr
        open(f"{U}/emutant3-{n}.log", "w").write(text)
        if expect == "red":
            got = out.returncode == 1 and mark in text
            verdict = "KILLED" if got else f"SURVIVED (exit {out.returncode})"
        else:
            got = out.returncode == 0 and mark in text
            verdict = "GREEN as predicted" if got else f"NOT GREEN (exit {out.returncode})"
        gained = [l.strip() for l in text.splitlines() if "GAINED" in l or l.startswith("cost:")]
        w = f"{U}/em3/{n}"
        shutil.rmtree(w, ignore_errors=True)
        os.makedirs(w)
        if os.path.isdir(f"{M}/w/out"):
            shutil.copytree(f"{M}/w/out", f"{w}/out", ignore=shutil.ignore_patterns("*.out"))
        before = old_verdicts(w, probes.split()) if os.path.isdir(f"{w}/out") else "no listings"
        note(f"{n}\t{verdict}\t{int(time.time() - t)}s\tnow: {gained}\tround-2 comparator: {before}")
        shutil.rmtree(h, ignore_errors=True)

if __name__ == "__main__":
    main()
