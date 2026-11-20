#!/usr/bin/env python3
"""G1-3 round 1: the engine mutants. Each is a scratch copy of the tree with
an edit, counted by scripts/cost.sh against HEAD on the probes named; the
tree is never edited. The verdict goes to emutants.results as it comes.

    emutants.py NAME...   (or no names: all)
"""
import os, shutil, subprocess, sys, time

REPO = "/home/user/supdb"
U = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-3"
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-3"
M = f"{TMP}/em"
RT = f"{U}/rt"
RES = f"{U}/emutants.results"

IA = "    fn install_ahead(&self, unsealed: &Snapshot) -> Result<()> {\n"
LEND = """    fn lend_at(&self, off: u64, len: usize) -> Option<&[u8]> {
        let end = off.checked_add(len as u64)?;
        if end > self.lend {
            return None;
        }
        // SAFETY: as `slice_at`.
        Some(unsafe { std::slice::from_raw_parts(self.ptr.add(off as usize), len) })
    }"""
HELPER = """
#[inline(never)]
fn cost_mutant_helper(b: &MmapBytes, off: u64, len: usize) -> Option<&[u8]> {
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
RA = """        let entered = self.enter();
        let r = self.read_all_in(key, f);"""
CD = "    fn commit_direct("
# name: ([(file, old, new)], probes, expect: "red" | "refused", mark)
E = {
  "E1_install_ahead_branch": ([("src/db.rs", IA, IA + "        if std::hint::black_box(&self.shared.pager).is_some() {\n            std::hint::black_box(());\n        }\n")],
      "scan scan_long", "red", "HOT GAINED: supdb::db::Reader::install_ahead "),
  "E7_install_ahead_loop": ([("src/db.rs", IA, IA + "        for i in 0..8u32 {\n            std::hint::black_box(i);\n        }\n")],
      "scan", "red", "HOT GAINED: supdb::db::Reader::install_ahead "),
  "E2_outlined_helper": ([("src/bytes.rs", LEND, "    fn lend_at(&self, off: u64, len: usize) -> Option<&[u8]> {\n        cost_mutant_helper(self, off, len)\n    }"),
                          ("src/bytes.rs", "\nimpl Bytes for MmapBytes {", HELPER + "\nimpl Bytes for MmapBytes {")],
      "scan", "red", "HOT GAINED: supdb::bytes::cost_mutant_helper "),
  "E8_read_helper": ([("src/db.rs", RA, "        cost_read_helper(key.len() as u32);\n" + RA),
                      ("src/db.rs", "EOF", "\n#[inline(never)]\nfn cost_read_helper(x: u32) -> u32 {\n    std::hint::black_box(x).wrapping_add(std::hint::black_box(1)).wrapping_mul(std::hint::black_box(3))\n}\n")],
      "read", "red", "HOT GAINED: supdb::db::cost_read_helper "),
  "E4_commit_direct_loop": ([("src/db.rs", None, None)],
      "load_durable load_buffered", "red", "HOT GAINED: supdb::db::Db::commit_direct "),
  "E6_suite_offsets_engine": ([("src/db.rs", RA, "        if std::hint::black_box(&self.shared.pager).is_some() {\n            std::hint::black_box(());\n        }\n" + RA),
                               ("bench/src/engines.rs", "    r.read_all(key, |v| n += consume(v))", "    r.read_all(key, |v| n += v.len())")],
      "read", "refused", "differs between the base and the head"),
}

def copy(dest):
    shutil.rmtree(dest, ignore_errors=True)
    os.makedirs(dest)
    files = subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "--", ".", ":!scratch", ":!bench/runs", ":!docs/figures"],
                           cwd=REPO, capture_output=True, text=True, check=True).stdout
    subprocess.run(f"tar -cf - -T - | (cd {dest} && tar -xf -)", shell=True, cwd=REPO, input=files, text=True, check=True)

def commit_direct_edit(s):
    at = s.index(CD)
    body = s.index("{\n", at) + 2
    return s[:body] + "        for i in 0..100u32 {\n            std::hint::black_box(i);\n        }\n" + s[body:]

def note(line):
    with open(RES, "a") as r:
        r.write(line + "\n")
    print(line, flush=True)

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
            if old is None:
                s2 = commit_direct_edit(s)
            elif old == "EOF":
                s2 = s + new
            elif s.count(old) != 1:
                note(f"{n}\tNOT APPLIED ({f}: {s.count(old)} occurrences)")
                ok = False
                break
            else:
                s2 = s.replace(old, new)
            open(p, "w").write(s2)
        if not ok:
            continue
        env = dict(os.environ, TMPDIR=TMP, COST_HEAD=h, COST_HEAD_TARGET=RT, COST_BASE_TARGET=RT, COST_DIR=f"{M}/w")
        t = time.time()
        out = subprocess.run(["sh", "scripts/cost.sh", "HEAD"] + probes.split(), cwd=REPO, env=env, capture_output=True, text=True)
        text = out.stdout + out.stderr
        open(f"{U}/emutant-{n}.log", "w").write(text)
        want = 1 if expect == "red" else 2
        killed = out.returncode == want and mark in text
        verdict = "KILLED" if killed else f"SURVIVED (exit {out.returncode})"
        last = text.strip().splitlines()[-1] if text.strip() else ""
        note(f"{n}\t{verdict}\t{int(time.time() - t)}s\t{last}")
        shutil.rmtree(h, ignore_errors=True)

if __name__ == "__main__":
    main()
