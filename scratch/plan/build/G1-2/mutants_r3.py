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

MUTANTS = [
    # the reviewers' extra mutants
    ("X1 libc::utime in drop_written", "door", libc_call('unsafe { libc::utime(c"/".as_ptr(), std::ptr::null()) };')),
    ("X1b same, round 2's config", "dbefore", libc_call('unsafe { libc::utime(c"/".as_ptr(), std::ptr::null()) };')),
    ("X2a libc::mknodat", "door", libc_call('unsafe { libc::mknodat(libc::AT_FDCWD, c"/x".as_ptr(), 0, 0) };')),
    ("X2a' same, round 2's config", "dbefore", libc_call('unsafe { libc::mknodat(libc::AT_FDCWD, c"/x".as_ptr(), 0, 0) };')),
    ("X2b libc::execveat", "door", libc_call('unsafe { libc::execveat(libc::AT_FDCWD, c"/x".as_ptr(), std::ptr::null(), std::ptr::null(), 0) };')),
    ("X2b' same, round 2's config", "dbefore", libc_call('unsafe { libc::execveat(libc::AT_FDCWD, c"/x".as_ptr(), std::ptr::null(), std::ptr::null(), 0) };')),
    ("X2c libc::dlopen", "door", libc_call('unsafe { libc::dlopen(c"/x".as_ptr(), 0) };')),
    ("X2c' same, round 2's config", "dbefore", libc_call('unsafe { libc::dlopen(c"/x".as_ptr(), 0) };')),
    ("X2d libc::lutimes", "door", libc_call('unsafe { libc::lutimes(c"/x".as_ptr(), std::ptr::null()) };')),
    ("X2d' same, round 2's config", "dbefore", libc_call('unsafe { libc::lutimes(c"/x".as_ptr(), std::ptr::null()) };')),
    ("X3 libc::bind of a sockaddr_un", "door", libc_call(SOCK)),
    ("X3b same, round 2's config", "dbefore", libc_call(SOCK)),
    ("X4 ffi! macro's extern $a block in db.rs, unlink called", "test", [("src/db.rs", append(FFI))]),
    ("X4b same, round 2's tests", "tbefore", [("src/db.rs", append(FFI))]),
    ("X4c same, the door run", "door", [("src/db.rs", append(FFI))]),
    ("X5 quiet!(allow(..); remove_file) through #[$a]", "test", X5),
    ("X5b same, round 2's tests", "tbefore", X5),
    ("X5c same, the door run", "door", X5),
    ("X6 cfg(not(debug_assertions)) use std::fs as f; f::remove_file in convert_walked", "test", X6),
    ("X6b same, round 2's tests", "tbefore", X6),
    ("X6c same, the door run", "door", X6),
    ("X7 use std::fs::* + remove_file in with_idle_io's not-Linux twin", "test", X7),
    ("X7b same, round 2's tests", "tbefore", X7),
    ("X8 concat! of /proc/self/status", "test", X8),
    ("X8b same, round 2's tests", "tbefore", X8),
    ("X9a run.sh's config dir $PWD/scripts", "script", [(RUNSH, sub('conf="$PWD/scripts/door"', 'conf="$PWD/scripts"'))]),
    ("X9a' same, round 2's scripts", "sbefore", [(RUNSH, sub('door_run "$PWD" "$PWD/scripts/door" "$PWD/target/door"', 'door_run "$PWD" "$PWD/scripts" "$PWD/target/door"'))]),
    ("X9b run.sh's config dir $PWD", "script", [(RUNSH, sub('conf="$PWD/scripts/door"', 'conf="$PWD"'))]),
    ("X9b' same, round 2's scripts", "sbefore", [(RUNSH, sub('door_run "$PWD" "$PWD/scripts/door" "$PWD/target/door"', 'door_run "$PWD" "$PWD" "$PWD/target/door"'))]),
    ("X10a door_cfg panic => abort", "test", [("tests/source.rs", sub('"panic" => "unwind"', '"panic" => "abort"'))]),
    ("X10a' same, round 2's tests", "tbefore", [("tests/source.rs", sub('"panic" => "unwind"', '"panic" => "abort"'))]),
    ("X10b door_cfg target_pointer_width => 32", "test", [("tests/source.rs", sub('"target_pointer_width" => "64"', '"target_pointer_width" => "32"'))]),
    ("X10c door_cfg doc read true", "test", [("tests/source.rs", chain(
        sub('("test" | "windows" | "doc" | "miri" | "supdb_emulated", None) => Some(false),', '("test" | "windows" | "miri" | "supdb_emulated", None) => Some(false),\n        ("doc", None) => Some(true),')))]),
    ("X12a red.sh's line check skips the last line (-lt)", "script", [(REDSH, sub('  while [ "$k" -le "$added" ]; do\n    printf', '  while [ "$k" -lt "$added" ]; do\n    printf'))]),
    ("X12b same with libc::utime's entry gone", "script", [(REDSH, sub('  while [ "$k" -le "$added" ]; do\n    printf', '  while [ "$k" -lt "$added" ]; do\n    printf')), (CONF, sub('"libc::acct", "libc::utime", ', '"libc::acct", '))]),
    ("X12c red.sh counts one line fewer (added-1)", "script", [(REDSH, sub('listed=$(wc -l < "$copy/lines")', 'listed=$(wc -l < "$copy/lines")\nadded=$((added - 1))'))]),
    ("X12d O11 again: the Command entries gone", "script", [(CONF, chain(sub('  { path = "std::process::Command::new", reason = "a program run acts on files past the door" },\n', ''), sub('  { path = "std::process::Command", reason = "a program run acts on files past the door" },\n', '')))]),
    ("X13 door_texts without types + Option<memmap2::Mmap> in compiled-out code", "test", X13),
    # mine
    ("O16 foreign_blocks: the next word read at any offset", "test", [("tests/source.rs", sub("            .filter(|(o, _)| *o == 0)\n            .map(|(_, n)| n);", "            .map(|(_, n)| n);"))]),
    ("O16b foreign_blocks: `fn` not a function's", "test", [("tests/source.rs", sub('Some("fn") | Some("crate")', 'Some("crate")'))]),
    ("O17 metavariable attribute not read", "test", [("tests/source.rs", sub("let metavar = s.get(skip_ws(&s, j + 1)) == Some(&'$');", "let metavar = false;"))]),
    ("O18 allow outside an attribute not read", "test", [("tests/source.rs", sub('if (w != "allow" && w != "expect") || spans', 'if true || spans'))]),
    ("O19 use_hits: glob not read", "test", [("tests/source.rs", sub("if body.contains(&'*') {", "if false {"))]),
    ("O20 use_hits: rename not read", "test", [("tests/source.rs", sub('} else if names.iter().any(|(_, n)| n == "as") {', '} else if false {'))]),
    ("O21 use_hits: a function taken out not read", "test", [("tests/source.rs", sub(".find(|(_, n)| fns.contains(n.as_str()))", ".find(|(_, n)| n.is_empty())"))]),
    ("O22 concat! rule gone", "test", [("tests/source.rs", sub('.any(|(at, w)| w == "concat" &&', '.any(|(at, w)| w == "concat_" &&'))]),
    ("O23 libc scan: *mut c_char not read", "test", [("tests/source.rs", sub('.or_else(|| ty.strip_prefix("*mut"))', ''))]),
    ("O24 libc_unclassed: an entry barred as well passes", "test", [("tests/source.rs", sub('        } else if barred.contains(n) {', '        } else if barred.contains(n) && false {'))]),
    ("O25 LIBC_FILES without linux_l4re_shared.rs", "test", [("tests/source.rs", sub('    "src/unix/linux_like/linux_l4re_shared.rs",\n', ''))]),
    ("O26 libc_unclassed: a call unclassed passes", "test", [("tests/source.rs", sub('        if !barred.contains(n) && !notfile.contains_key(n) {', '        if false {'))]),
    ("O27 the config's utime entry gone (red's 11th line)", "script", [(CONF, sub('"libc::acct", "libc::utime", ', '"libc::acct", '))]),
    ("O28 run.sh hands red.sh no config", "script", [(RUNSH, sub('sh scripts/door/red.sh "$conf"', 'sh scripts/door/red.sh'))]),
    ("O29 red.sh lints its copy under its own copy of scripts/door (not CONFDIR)", "script", [(REDSH, sub('cp "$conf/clippy.toml" "$copy/conf/door.toml"', 'cp "$root/scripts/door/clippy.toml" "$copy/conf/door.toml"')), (RUNSH, sub('conf="$PWD/scripts/door"', 'conf="$PWD/scripts"'))]),
    ("O30 red phase 2: the lines' entries not taken out", "script", [(REDSH, sub('      if (gsub(e ",? ?", "")) found[e]++', '      found[e]++'))]),
    # earlier rounds' gate mutants, red.sh rewritten
    ("R-O10 libc::remove entry gone", "script", [(CONF, sub('  "libc::remove", ', '  '))]),
    ("R-X8a CLIPPY_CONF_DIR dropped from door_lint", "script", [("scripts/door/lint.sh", sub('CLIPPY_CONF_DIR="$2" cargo clippy', 'cargo clippy'))]),
    ("R-X8b --release for --profile checked", "script", [("scripts/door/lint.sh", sub('cargo clippy --profile checked --lib', 'cargo clippy --release --lib'))]),
    ("R-R3 red's bogus entry not inserted", "script", [(REDSH, sub('  /^disallowed-methods = \\[/ { print "  \\"libc::no_such_fn_door_red\\","; n++ }', '  /^disallowed-methods = \\[/ { n++ }'))]),
    ("R-R2d door_run returns clippy's status on a config diagnostic", "script", [("scripts/door/lint.sh", sub('    return 2\n', '    return "$status"\n'))]),
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
