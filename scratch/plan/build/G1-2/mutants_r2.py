#!/usr/bin/env python3
"""Round 2 mutants of the door. One verdict a line to mutants_r2.results as it comes.

kind test:   edit the real tree (keep_r2 as base), run the source binary; KILLED = a named
             test FAILED; INVALID = it did not compile. Tree restored from keep_r2/tree.tar
             (sha checked) and added files removed after each.
kind tbefore: as test, but tests/source.rs and tests/data/door.txt are round 1's (keep_r1):
             'hole' = it passes (the input was not caught before this round).
kind door:   a copy of the repo (mrepo/), the edit, door_run (lint + config check) on the
             copy; KILLED = non-zero exit.
kind dbefore: as door, the copy carrying round 1's clippy.toml.
kind script: a copy, the edit, the whole scripts/door/run.sh; KILLED = non-zero exit.
"""
import os, subprocess, sys, shutil, hashlib, time, tarfile, io

B = os.path.dirname(os.path.abspath(__file__))
R = "/home/user/supdb"
RES = os.path.join(B, "mutants_r2.results")
LOG = os.path.join(B, "mutants_r2.log")
MREPO = os.path.join(B, "mrepo")
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-2"
ANCHOR = "fn drop_written(file: &File) -> Result<()> {"
SEAL = "    pub fn seal(&mut self) -> Result<()> {"


def at_anchor(anchor, line):
    def f(t):
        assert t.count(anchor) == 1, anchor
        return t.replace(anchor, anchor + "\n    " + line, 1)
    return f


def in_drop_written(line):
    return at_anchor(ANCHOR, line)


def in_seal(line):
    return at_anchor(SEAL, line)


def append(text):
    return lambda t: t + "\n" + text + "\n"


def new_file(text):
    return lambda t: text


def sub(old, new):
    def f(t):
        assert t.count(old) >= 1, old
        return t.replace(old, new, 1)
    return f


def chain(*fs):
    def f(t):
        for g in fs:
            t = g(t)
        return t
    return f


def log(s):
    with open(LOG, "a") as f:
        f.write(s + "\n")


def result(name, verdict, how):
    with open(RES, "a") as f:
        f.write(f"{name}\t{verdict}\t{how}\n")


def restore_tree(added):
    for p in added:
        fp = os.path.join(R, p)
        if os.path.exists(fp):
            os.remove(fp)
        d = os.path.dirname(fp)
        if d.endswith("src/db/vault") and os.path.isdir(d) and not os.listdir(d):
            os.rmdir(d)
    subprocess.run(["tar", "xf", os.path.join(B, "keep_r2/tree.tar")], cwd=R, check=True)
    bad = subprocess.run(["sha256sum", "-c", "--quiet", os.path.join(B, "keep_r2/TREE_SHA256SUMS")],
                         cwd=R, capture_output=True, text=True)
    st = subprocess.run(["git", "status", "--short"], cwd=R, capture_output=True, text=True).stdout
    if bad.returncode != 0 or "src/db/vault/" in st or "door_probe" in st:
        log("RESTORE FAILED " + bad.stdout + bad.stderr + st)
        sys.exit(2)


def run(cmd, cwd, env=None):
    e = dict(os.environ)
    e["TMPDIR"] = TMP
    if env:
        e.update(env)
    t0 = time.time()
    p = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True, shell=True)
    log(f"--- {cmd} (in {cwd}) exit {p.returncode} {time.time()-t0:.0f}s")
    log(p.stdout[-5000:])
    log(p.stderr[-5000:])
    return p


def make_mrepo(before):
    os.makedirs(MREPO, exist_ok=True)
    for d in ["src", "scripts", "tests"]:
        shutil.rmtree(os.path.join(MREPO, d), ignore_errors=True)
    for f in ["Cargo.toml", "Cargo.lock"]:
        shutil.copy(os.path.join(R, f), MREPO)
    shutil.copytree(os.path.join(R, "src"), os.path.join(MREPO, "src"))
    shutil.copytree(os.path.join(R, "scripts"), os.path.join(MREPO, "scripts"))
    os.makedirs(os.path.join(MREPO, "tests/data"))
    shutil.copy(os.path.join(R, "tests/data/door.txt"), os.path.join(MREPO, "tests/data"))
    if before:
        with tarfile.open(os.path.join(B, "keep_r1/tree.tar")) as tf:
            data = tf.extractfile("scripts/door/clippy.toml").read()
        open(os.path.join(MREPO, "scripts/door/clippy.toml"), "wb").write(data)


def edit(root, path, fn):
    p = os.path.join(root, path)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    t = open(p).read() if os.path.exists(p) else ""
    open(p, "w").write(fn(t))


def r1_tests():
    with tarfile.open(os.path.join(B, "keep_r1/tree.tar")) as tf:
        for p in ["tests/source.rs", "tests/data/door.txt"]:
            open(os.path.join(R, p), "wb").write(tf.extractfile(p).read())


HELPER = "pub(crate) fn f() {\n    let _ = std::fs::remove_file(\"/x\");\n}\n"
MAPPROBE = "pub(crate) fn door_probe_map() -> memmap2::MmapMut {\n    memmap2::MmapMut::map_anon(1).unwrap()\n}\n"

E1 = [("src/db.rs", in_seal('#[cfg(not(clippy))] let _ = std::fs::remove_file("/x");'))]
E2 = [("src/db.rs", sub("        #[cfg(not(debug_assertions))]\n        let _ = unsealed;\n",
                        "        #[cfg(not(debug_assertions))]\n        let _ = unsealed;\n        #[cfg(not(debug_assertions))]\n        let _ = std::path::Path::new(\"/x\").exists();\n"))]
E3 = [("src/db.rs", append('#[cfg(not(target_os = "linux"))]\nfn door_probe() {\n    let _ = std::fs::read("/x");\n}'))]
E9 = [("src/db/vault.rs", append("pub(crate) mod door_helper;")),
      ("src/db/vault/door_helper.rs", new_file(HELPER)),
      ("src/db.rs", in_seal("crate::db::vault::door_helper::f();"))]
E13 = [("src/crypt.rs", sub("    pub fn parse(bytes: &[u8]) -> Result<Header> {\n",
                            "    pub fn parse(bytes: &[u8]) -> Result<Header> {\n        std::thread::spawn(|| {});\n"))]

MUTANTS = [
    # the reviewers' extra mutants: source-test kind
    ("E1 cfg(not(clippy)) remove_file in Db::seal", "test", E1),
    ("E1b same, round 1's tests", "tbefore", E1),
    ("E1c same, the door run", "door", E1),
    ("E2 cfg(not(debug_assertions)) Path::exists in convert_walked", "test", E2),
    ("E2b same, round 1's tests", "tbefore", E2),
    ("E2c same, the door run", "door", E2),
    ("E3 cfg(not(linux)) fn door_probe reading a file", "test", E3),
    ("E3b same, round 1's tests", "tbefore", E3),
    ("E9 mod door_helper; in vault.rs, its file calls remove_file, called from db.rs", "test", E9),
    ("E9b same, round 1's tests", "tbefore", E9),
    ("E9c same, the door run", "door", E9),
    ("E13 thread::spawn in crypt.rs code", "test", E13),
    ("E13b same, round 1's tests", "tbefore", E13),
    # the reviewers' extra mutants: door kind
    ("E4 libc::remove in drop_written", "door", [("src/db/zfile.rs", in_drop_written('unsafe { libc::remove(c"/x".as_ptr()) };'))]),
    ("E4b same, round 1's config", "dbefore", [("src/db/zfile.rs", in_drop_written('unsafe { libc::remove(c"/x".as_ptr()) };'))]),
    ("E5 allow(deprecated) fs::soft_link", "door", [("src/db/zfile.rs", in_drop_written('#[allow(deprecated)] let _ = std::fs::soft_link("/a", "/b");'))]),
    ("E5b same, round 1's config", "dbefore", [("src/db/zfile.rs", in_drop_written('#[allow(deprecated)] let _ = std::fs::soft_link("/a", "/b");'))]),
    ("E6 Command::new(rm)", "door", [("src/db/zfile.rs", in_drop_written('let _ = std::process::Command::new("rm").arg("/x").status();'))]),
    ("E6b same, round 1's config", "dbefore", [("src/db/zfile.rs", in_drop_written('let _ = std::process::Command::new("rm").arg("/x").status();'))]),
    ("E7 UnixListener::bind", "door", [("src/db/zfile.rs", in_drop_written('let _ = std::os::unix::net::UnixListener::bind("/x");'))]),
    ("E7b same, round 1's config", "dbefore", [("src/db/zfile.rs", in_drop_written('let _ = std::os::unix::net::UnixListener::bind("/x");'))]),
    ("E8 libc::lchown + libc::setxattr", "door", [("src/db/zfile.rs", in_drop_written('unsafe { libc::lchown(c"/x".as_ptr(), 0, 0); libc::setxattr(c"/x".as_ptr(), c"u".as_ptr(), std::ptr::null(), 0, 0); }'))]),
    ("E8b same, round 1's config", "dbefore", [("src/db/zfile.rs", in_drop_written('unsafe { libc::lchown(c"/x".as_ptr(), 0, 0); libc::setxattr(c"/x".as_ptr(), c"u".as_ptr(), std::ptr::null(), 0, 0); }'))]),
    ("E8x libc::setxattr alone", "door", [("src/db/zfile.rs", in_drop_written('unsafe { libc::setxattr(c"/x".as_ptr(), c"u".as_ptr(), std::ptr::null(), 0, 0); }'))]),
    ("E10 allow(clippy::DISALLOWED_METHODS) fn door_probe", "door", [("src/db/zfile.rs", append('#[allow(clippy::DISALLOWED_METHODS)]\nfn door_probe() {\n    let _ = std::fs::read("/");\n}'))]),
    ("E10b same, round 1's config", "dbefore", [("src/db/zfile.rs", append('#[allow(clippy::DISALLOWED_METHODS)]\nfn door_probe() {\n    let _ = std::fs::read("/");\n}'))]),
    ("E12a libc::open_by_handle_at", "door", [("src/db/zfile.rs", in_drop_written('unsafe { libc::open_by_handle_at(-1, std::ptr::null_mut(), 0) };'))]),
    ("E12b libc::write on the door handle's fd", "door", [("src/db/zfile.rs", in_drop_written('unsafe { use std::os::unix::io::AsRawFd; libc::write(file.as_raw_fd(), std::ptr::null(), 0) };'))]),
    ("E12c libc::write, round 1's config", "dbefore", [("src/db/zfile.rs", in_drop_written('unsafe { use std::os::unix::io::AsRawFd; libc::write(file.as_raw_fd(), std::ptr::null(), 0) };'))]),
    ("E15a MmapMut from a door fn, .flush() in db.rs", "door", [("src/db/vault.rs", append(MAPPROBE)), ("src/db.rs", in_seal("let _ = crate::db::vault::door_probe_map().flush();"))]),
    ("E15b MmapMut from a door fn, .advise() in db.rs", "door", [("src/db/vault.rs", append(MAPPROBE)), ("src/db.rs", in_seal("let _ = crate::db::vault::door_probe_map().advise(memmap2::Advice::DontNeed);"))]),
    ("E15c .flush(), round 1's config", "dbefore", [("src/db/vault.rs", append(MAPPROBE)), ("src/db.rs", in_seal("let _ = crate::db::vault::door_probe_map().flush();"))]),
    ("E15d MmapMut from a door fn, .advise(Sequential) in db.rs (E15b did not compile)", "door", [("src/db/vault.rs", append(MAPPROBE)), ("src/db.rs", in_seal("let _ = crate::db::vault::door_probe_map().advise(memmap2::Advice::Sequential);"))]),
    ("E15e same, round 1's config", "dbefore", [("src/db/vault.rs", append(MAPPROBE)), ("src/db.rs", in_seal("let _ = crate::db::vault::door_probe_map().advise(memmap2::Advice::Sequential);"))]),
    # my own, on the lines this round wrote
    ("O1 door_cfg answers every key=value true", "test", [("tests/source.rs", sub("            Some(v == have)\n", "            Some(!v.is_empty() || v == have)\n"))]),
    ("O2 attr_target_end: no else continuation", "test", [("tests/source.rs", sub('if next == "else" || s.get(n) == Some(&\'.\')', 'if s.get(n) == Some(&\'.\')'))]),
    ("O3 compiled_out scans no text", "test", [("tests/source.rs", sub("        for t in texts {\n            for i in text_hits(&flat, c.end, end, t) {", "        for t in texts.iter().take(0) {\n            for i in text_hits(&flat, c.end, end, t) {"))]),
    ("O4 files_compiled_out finds none", "test", [("tests/source.rs", sub("    while let Some(f) = todo.pop() {", "    todo.clear();\n    while let Some(f) = todo.pop() {"))]),
    ("O5 door_children ignores include!", "test", [("tests/source.rs", sub('            "include" => s.get(k) == Some(&\'!\'),', '            "include" => false,'))]),
    ("O6 a cfg attribute ends at its predicate (regions empty)", "test", [("tests/source.rs", sub("            j if in_attr && s.get(j) == Some(&']') => j + 1,", "            j if in_attr && s.get(j) == Some(&']') => j,"))]),
    ("O7 door modules' threads not counted", "test", [("tests/source.rs", sub("            let n = thread_paths(&l.flat);\n            if n > 0 {", "            let n = thread_paths(&l.flat);\n            if n > 0 && !door {"))]),
    ("O8 a clippy predicate passes", "test", [("tests/source.rs", sub("        if names_clippy {", "        if names_clippy && false {"))]),
    ("O9 a cfg_attr applying a cfg passes", "test", [("tests/source.rs", sub("        if c.applies_cfg {", "        if c.applies_cfg && false {"))]),
    ("O12 door_texts: no Path method text", "test", [("tests/source.rs", sub('        if segs[n - 2] == "Path" {', '        if segs[n - 2] == "Path_" {'))]),
    ("O13 cfgout not a kind of door.txt", "test", [("tests/source.rs", sub('"thread", "cfgout"]', '"thread"]'))]),
    ("O14 door module code scanned and counted as cfgout (door flag ignored)", "test", [("tests/source.rs", sub("        if door || lines[ln].test || cfg_eval(&c.pred) == Some(true) {", "        if lines[ln].test || cfg_eval(&c.pred) == Some(true) {"))]),
    ("O15 an unknown predicate read as compiled in", "test", [("tests/source.rs", sub("        if door || lines[ln].test || cfg_eval(&c.pred) == Some(true) {", "        if door || lines[ln].test || cfg_eval(&c.pred) != Some(false) {"))]),
    ("O10 libc::remove entry gone (red.sh's 9th line)", "script", [("scripts/door/clippy.toml", sub('  "libc::remove", ', '  '))]),
    ("O11 Command entries gone (red.sh's 10th line)", "script", [("scripts/door/clippy.toml", chain(sub('  { path = "std::process::Command::new", reason = "a program run acts on files past the door" },\n', ''), sub('  { path = "std::process::Command", reason = "a program run acts on files past the door" },\n', '')))]),
]


def done():
    if not os.path.exists(RES):
        return set()
    return {l.split("\t")[0] for l in open(RES) if l.strip()}


def failed_tests(out):
    return [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]


def main():
    skip = done()
    for name, kind, edits in MUTANTS:
        if name in skip:
            continue
        log(f"=== {name} ({kind}) {time.strftime('%H:%M:%S')}")
        if kind in ("test", "tbefore"):
            added = [p for p, _ in edits if not os.path.exists(os.path.join(R, p))]
            try:
                if kind == "tbefore":
                    r1_tests()
                for p, fn in edits:
                    edit(R, p, fn)
                p = run("cargo test --profile checked --test source", R)
                out = p.stdout + p.stderr
                fails = failed_tests(out)
                if "error[E" in out or "could not compile" in out:
                    v, how = "INVALID", "did not compile"
                elif fails:
                    v, how = ("KILLED" if kind == "test" else "caught-before"), ",".join(fails)
                elif p.returncode == 0:
                    v, how = ("SURVIVED" if kind == "test" else "hole-before"), "source tests pass"
                else:
                    v, how = "ERROR", f"exit {p.returncode}, no failed test named"
            finally:
                restore_tree(added)
        else:
            make_mrepo(kind == "dbefore")
            for p, fn in edits:
                edit(MREPO, p, fn)
            if kind == "script":
                p = run("sh scripts/door/run.sh", MREPO)
            else:
                p = run('. scripts/door/lint.sh && cargo clean --profile checked --target-dir target/door -p supdb -q && door_run "$PWD" "$PWD/scripts/door" "$PWD/target/door"', MREPO)
            out = p.stdout + p.stderr
            dis = [l for l in out.splitlines() if "disallowed" in l or "unknown lint" in l]
            if p.returncode != 0:
                v = "KILLED" if kind != "dbefore" else "caught-before"
                how = (dis[0] if dis else out.strip().splitlines()[-1] if out.strip() else "")[:160]
            else:
                v = "SURVIVED" if kind != "dbefore" else "hole-before"
                how = "door passes"
        result(name, v, how)
        log(f"=> {v} {how}")
    log("ALL DONE " + time.strftime('%H:%M:%S'))


main()
