#!/usr/bin/env python3
"""G1-3 round 2: the reviewers' engine mutants. Each is a scratch copy of the
tree with an edit, counted by scripts/cost.sh against HEAD on the probes
named; the tree is never edited. Each mutant's listings are kept under
em2/<name>/out, so the round-1 comparator (keep2/) can be run over the same
counts: the verdict before the repair and after, from one count. The
verdict goes to emutants2.results as it comes.

    emutants2.py NAME...   (or no names: all)
"""
import os, re, shutil, subprocess, sys, time

REPO = "/home/user/supdb"
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3"
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3"
M = f"{TMP}/em"
RT = f"{U}/rt"
RES = f"{U}/emutants2.results"
OLD = f"{U}/keep2/scripts__cost-diff.py"
OLDHOT = f"{U}/keep2/scripts__cost-hot.txt"

PUSH = "self.own_row.push(self.own_crc);"
LEND = """    fn lend_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        let end = off.checked_add(len as u64)?;
        if end > self.lend {
            return None;
        }
        // SAFETY: as `slice_at`.
        Some(unsafe { std::slice::from_raw_parts(self.ptr.add(off as usize), len) })
    }"""
NOMANGLE = """
#[no_mangle]
extern "C" fn cost_mutant_helper(b: &MmapBytes, off: u64, len: usize) -> Option<&[u8]> {
    std::hint::black_box(1u32);
    std::hint::black_box(2u32);
    std::hint::black_box(3u32);
    let end = off.checked_add(len as u64)?;
    if end > b.lend {
        return None;
    }
    // SAFETY: as `slice_at`.
    Some(unsafe { std::slice::from_raw_parts(b.ptr.add(off as usize), len) })
}
"""
FW = """    let mut w = [0u8; 8];
    let n = tail.len().min(8);
    w[..n].copy_from_slice(&tail[..n]);
    u64::from_be_bytes(w)"""
FW2 = """    let mut w = [0u8; 16];
    let n = tail.len().min(16);
    w[..n].copy_from_slice(&tail[..n]);
    u64::from_be_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]])"""
SR = """    fn sync_run(&mut self) -> Result<()> {
"""
EL = """    fn leave(&mut self, to: usize) -> Result<()> {
        self.r += 1;
"""
# name: ([(file, old, new)], probes, expect "red", mark)
E = {
  "E9_seal_piece_field": ([("src/db.rs", "RE", None)], "seal merge", "red", "HOT GAINED: "),
  "E12_seal_piece_blackbox": ([("src/db.rs", "RE0", None)], "seal", "red", "HOT GAINED: "),
  "E10_libc_length": ([("src/db/zpages.rs", FW, FW2)], "read_z", "red", "LIBRARY GAINED: "),
  "E11_no_mangle_helper": ([("src/bytes.rs", LEND, "    fn lend_at(&self, off: u64, len: usize) -> Option<&[u8]> {\n        cost_mutant_helper(self, off, len)\n    }"),
                            ("src/bytes.rs", "\nimpl Bytes for MmapBytes {", NOMANGLE + "\nimpl Bytes for MmapBytes {")],
      "scan", "red", "HOT GAINED: cost_mutant_helper "),
  "E11b_no_mangle_out_of_line": ([("src/bytes.rs", LEND, "    fn lend_at(&self, off: u64, len: usize) -> Option<&[u8]> {\n        cost_mutant_helper(self, off, len)\n    }"),
                            ("src/bytes.rs", "\nimpl Bytes for MmapBytes {", NOMANGLE.replace("#[no_mangle]", "#[no_mangle]\n#[inline(never)]") + "\nimpl Bytes for MmapBytes {")],
      "scan", "red", "HOT GAINED: cost_mutant_helper "),
  "E14_per_commit_loop": ([("src/db.rs", SR, SR + "        for i in 0..13u32 {\n            std::hint::black_box(i);\n        }\n")],
      "load_durable", "red", "HOT GAINED: supdb::db::Db::sync_run "),
  "E15_emitter_per_block": ([("src/db.rs", EL, EL + "        if self.r % 64 == 0 {\n            std::hint::black_box(());\n        }\n")],
      "merge", "red", "HOT GAINED: supdb::db::Emitter::leave "),
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

SPEC = {"read_z": ("200000", "read_only", True), "scan": ("1000", "lag0_probe", True), "seal": ("100000", "seal_probe", True),
        "merge": ("200000", "merge_walk", False), "load_durable": ("100000", "load_only", False)}

def old_verdicts(w, probes):
    """The round-1 comparator over the same listings: its slack 0.05 an op,
    OWN only for a toggled probe."""
    out = []
    for p in probes:
        ops, ex, tog = SPEC[p]
        own = f"supdb::,supdb_bench::,{ex}::" if tog else ""
        r = subprocess.run(["python3", OLD, f"{w}/out/base.{p}.self", f"{w}/out/head.{p}.self", ops, "", OLDHOT, p, own],
                           capture_output=True, text=True)
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
            if old == "RE":
                k = s.count(PUSH)
                s2 = s.replace(PUSH, "if std::hint::black_box(&self.section).is_some() {\n                        std::hint::black_box(());\n                    }\n                    " + PUSH)
            elif old == "RE0":
                k = s.count(PUSH)
                s2 = s.replace(PUSH, "std::hint::black_box(0u32);\n                    " + PUSH)
            elif s.count(old) != 1:
                note(f"{n}\tNOT APPLIED ({f}: {s.count(old)} occurrences)")
                ok = False
                break
            else:
                s2 = s.replace(old, new)
            open(p, "w").write(s2)
        if not ok:
            continue
        # One working directory for every mutant: the base's worktree is
        # built at its path, and a path a mutant would be a build each.
        env = dict(os.environ, TMPDIR=TMP, COST_HEAD=h, COST_HEAD_TARGET=RT, COST_BASE_TARGET=RT, COST_DIR=f"{M}/w")
        t = time.time()
        out = subprocess.run(["sh", "scripts/cost.sh", "HEAD"] + probes.split(), cwd=REPO, env=env, capture_output=True, text=True)
        text = out.stdout + out.stderr
        open(f"{U}/emutant2-{n}.log", "w").write(text)
        killed = out.returncode == 1 and mark in text
        verdict = "KILLED" if killed else f"SURVIVED (exit {out.returncode})"
        gained = [l.strip() for l in text.splitlines() if "GAINED" in l]
        w = f"{U}/em2/{n}"
        shutil.rmtree(w, ignore_errors=True)
        os.makedirs(w)
        if os.path.isdir(f"{M}/w/out"):
            shutil.copytree(f"{M}/w/out", f"{w}/out", ignore=shutil.ignore_patterns("*.out"))
        before = old_verdicts(w, probes.split()) if os.path.isdir(f"{w}/out") else "no listings"
        note(f"{n}\t{verdict}\t{int(time.time() - t)}s\tnow: {gained}\tround-1 comparator: {before}")
        shutil.rmtree(h, ignore_errors=True)

if __name__ == "__main__":
    main()
