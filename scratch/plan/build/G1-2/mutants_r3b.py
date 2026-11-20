#!/usr/bin/env python3
"""Round 3 mutants of the door. One verdict a line to mutants_r3.results as it comes.

kind test:   edit the real tree (keep_r3f as base), run the source binary; KILLED = a named
             test FAILED; INVALID = it did not compile. Tree restored from keep_r3f/tree.tar
             (sha checked) and added files removed after each.
kind tbefore: as test, but tests/source.rs and tests/data/door.txt are round 2's (keep_r3):
             'hole' = it passes (the input was not caught before this round).
kind door:   a copy of the repo (mrepo/), the edit, door_run (lint + config check) on the
             copy; KILLED = non-zero exit.
kind dbefore: as door, the copy carrying round 1's clippy.toml.
kind script: a copy, the edit, the whole scripts/door/run.sh; KILLED = non-zero exit.
"""
import os, subprocess, sys, shutil, hashlib, time, tarfile, io

B = os.path.dirname(os.path.abspath(__file__))
R = "/home/user/supdb"
RES = os.path.join(B, "mutants_r3.results")
LOG = os.path.join(B, "mutants_r3.log")
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
    subprocess.run(["tar", "xf", os.path.join(B, "keep_r3f/tree.tar")], cwd=R, check=True)
    bad = subprocess.run(["sha256sum", "-c", "--quiet", os.path.join(B, "keep_r3f/TREE_SHA256SUMS")],
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



def make_mrepo(before):
    os.makedirs(MREPO, exist_ok=True)
    for d in ["src", "scripts", "tests"]:
        shutil.rmtree(os.path.join(MREPO, d), ignore_errors=True)
    for f in ["Cargo.toml", "Cargo.lock"]:
        shutil.copy(os.path.join(R, f), MREPO)
    shutil.copytree(os.path.join(R, "src"), os.path.join(MREPO, "src"))
    shutil.copytree(os.path.join(R, "scripts"), os.path.join(MREPO, "scripts"))
    shutil.copytree(os.path.join(R, "tests/data"), os.path.join(MREPO, "tests/data"))
    if before:
        names = ["scripts/door/clippy.toml"] if before == "config" else [
            "scripts/door/clippy.toml", "scripts/door/run.sh", "scripts/door/red.sh", "scripts/door/lint.sh"]
        with tarfile.open(os.path.join(B, "keep_r3/tree.tar")) as tf:
            for n in names:
                open(os.path.join(MREPO, n), "wb").write(tf.extractfile(n).read())


def r2_tests():
    with tarfile.open(os.path.join(B, "keep_r3/tree.tar")) as tf:
        for p in ["tests/source.rs", "tests/data/door.txt"]:
            open(os.path.join(R, p), "wb").write(tf.extractfile(p).read())


def libc_call(code):
    return [("src/db/zfile.rs", in_drop_written(code))]


FFI = ('macro_rules! ffi { ($a:literal) => { extern $a { fn unlink(p: *const std::ffi::c_char) -> i32; } } }\n'
       'ffi!("C");\npub fn door_probe_ffi() -> i32 { unsafe { unlink(c"/x".as_ptr()) } }')
QUIET_DEF = 'macro_rules! quiet { ($a:meta; $e:expr) => { #[$a] let _ = $e; } }'
X5 = [("src/db/zfile.rs", chain(append(QUIET_DEF), in_drop_written('quiet!(allow(clippy::disallowed_methods); std::fs::remove_file("/"));')))]
X6 = [("src/db.rs", sub("        #[cfg(not(debug_assertions))]\n        let _ = unsealed;\n",
                        "        #[cfg(not(debug_assertions))]\n        let _ = (unsealed, { use std::fs as f; f::remove_file(\"/\") });\n"))]
X7 = [("src/db.rs", sub('    #[cfg(not(target_os = "linux"))]\n    {\n        f()\n    }\n}',
                        '    #[cfg(not(target_os = "linux"))]\n    {\n        { use std::fs::*; let _ = remove_file("/"); }\n        f()\n    }\n}'))]
X8 = [("src/db.rs", sub('std::fs::read_to_string("/proc/self/status")', 'std::fs::read_to_string(concat!("/pro", "c/self/status"))'))]
X13 = [("tests/source.rs", sub('''    for p in door_table(conf, "disallowed-types") {
        out.insert(p.rsplit("::").next().unwrap_or("").to_string());
    }''', "")),
       ("src/db/zfile.rs", append('#[cfg(not(target_os = "linux"))]\nfn door_probe() {\n    let _m: Option<memmap2::Mmap> = None;\n}'))]
SOCK = ('unsafe { let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0); '
        'let mut a: libc::sockaddr_un = std::mem::zeroed(); a.sun_family = libc::AF_UNIX as libc::sa_family_t; '
        'libc::bind(fd, &a as *const libc::sockaddr_un as *const libc::sockaddr, '
        'std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t); }')
REDSH = "scripts/door/red.sh"
RUNSH = "scripts/door/run.sh"
CONF = "scripts/door/clippy.toml"

X5R = [("src/db/zfile.rs", chain(sub(ANCHOR, QUIET_DEF + "\n" + ANCHOR), in_drop_written('quiet!(allow(clippy::disallowed_methods); std::fs::remove_file("/"));')))]
X8C = [("src/db/zfile.rs", in_drop_written('let _ = concat!("/pro", "c/self/status");'))]
MUTANTS = [
    ("X5r quiet! defined before drop_written, allow handed through #[$a]", "test", X5R),
    ("X5r' same, round 2's tests", "tbefore", X5R),
    ("X5r'' same, the door run", "door", X5R),
    ("X8c a new concat! /proc literal in drop_written", "test", X8C),
    ("X8c' same, round 2's tests", "tbefore", X8C),
]

def done():
    if not os.path.exists(RES):
        return set()
    return {l.split("\t")[0] for l in open(RES) if l.strip()}


def failed_tests(out):
    return [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]


def main():
    skip = done()
    only = sys.argv[1:]
    for name, kind, edits in MUTANTS:
        if name in skip or (only and not any(name.startswith(o) for o in only)):
            continue
        log(f"=== {name} ({kind}) {time.strftime('%H:%M:%S')}")
        if kind in ("test", "tbefore"):
            added = [p for p, _ in edits if not os.path.exists(os.path.join(R, p))]
            try:
                if kind == "tbefore":
                    r2_tests()
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
            before = {"dbefore": "config", "sbefore": "scripts"}.get(kind)
            make_mrepo(before)
            for p, fn in edits:
                edit(MREPO, p, fn)
            if kind in ("script", "sbefore"):
                p = run("sh scripts/door/run.sh", MREPO)
            else:
                p = run('. scripts/door/lint.sh && cargo clean --profile checked --target-dir target/door -p supdb -q && door_run "$PWD" "$PWD/scripts/door" "$PWD/target/door"', MREPO)
            out = p.stdout + p.stderr
            dis = [l for l in out.splitlines() if "disallowed" in l or "door red:" in l or "unknown lint" in l]
            if p.returncode != 0:
                v = "KILLED" if not before else "caught-before"
                how = (dis[0] if dis else out.strip().splitlines()[-1] if out.strip() else "")[:200]
            else:
                v = "SURVIVED" if not before else "hole-before"
                how = "door passes"
        result(name, v, how)
        log(f"=> {v} {how}")
    log("ALL DONE " + time.strftime('%H:%M:%S'))


main()
