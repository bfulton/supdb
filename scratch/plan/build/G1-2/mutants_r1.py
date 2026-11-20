#!/usr/bin/env python3
"""Round 1 mutants of the door. One verdict a line to mutants_r1.results as it comes.

kind test:   edit the real tree, run the source tests (filter), killed = non-zero exit;
             the tree is restored from keep_r1/tree.tar and checked by sha after each.
kind script: edit a scratch copy of the repo (mrepo/) and run its scripts/door/run.sh;
             killed = non-zero exit.
kind before: as script, but the copy carries round 0's scripts and config (keep/);
             'hole' = run.sh exits 0 (the input was not caught before this round).
"""
import os, subprocess, sys, shutil, hashlib, time

B = os.path.dirname(os.path.abspath(__file__))
R = "/home/user/supdb"
RES = os.path.join(B, "mutants_r1.results")
LOG = os.path.join(B, "mutants_r1.log")
MREPO = os.path.join(B, "mrepo")
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-2"
ANCHOR = "fn drop_written(file: &File) -> Result<()> {"


def first_in_drop_written(line):
    def f(t):
        assert t.count(ANCHOR) == 1
        return t.replace(ANCHOR, ANCHOR + "\n    " + line, 1)
    return f


def prepend(line):
    return lambda t: line + "\n" + t


def append(text):
    return lambda t: t + "\n" + text + "\n"


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


def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()


def log(s):
    with open(LOG, "a") as f:
        f.write(s + "\n")


def result(name, verdict, how):
    with open(RES, "a") as f:
        f.write(f"{name}\t{verdict}\t{how}\n")


def restore_tree():
    subprocess.run(["tar", "xf", os.path.join(B, "keep_r1/tree.tar")], cwd=R, check=True)
    bad = subprocess.run(["sha256sum", "-c", "--quiet", os.path.join(B, "keep_r1/TREE_SHA256SUMS")],
                         cwd=R, capture_output=True, text=True)
    if bad.returncode != 0:
        log("RESTORE FAILED " + bad.stdout + bad.stderr)
        sys.exit(2)


def run(cmd, cwd, env=None):
    e = dict(os.environ)
    e["TMPDIR"] = TMP
    if env:
        e.update(env)
    t0 = time.time()
    p = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True, shell=True)
    log(f"--- {cmd} (in {cwd}) exit {p.returncode} {time.time()-t0:.0f}s")
    log(p.stdout[-6000:])
    log(p.stderr[-6000:])
    return p


def make_mrepo(old):
    os.makedirs(MREPO, exist_ok=True)
    for d in ["src", "scripts", "tests", ".cargo"]:
        shutil.rmtree(os.path.join(MREPO, d), ignore_errors=True)
    for f in ["Cargo.toml", "Cargo.lock"]:
        shutil.copy(os.path.join(R, f), MREPO)
    shutil.copytree(os.path.join(R, "src"), os.path.join(MREPO, "src"))
    shutil.copytree(os.path.join(R, "scripts"), os.path.join(MREPO, "scripts"))
    os.makedirs(os.path.join(MREPO, "tests/data"))
    shutil.copy(os.path.join(R, "tests/data/door.txt"), os.path.join(MREPO, "tests/data"))
    if old:
        k = os.path.join(B, "keep")
        for f in ["clippy.toml", "red.sh", "run.sh"]:
            shutil.copy(os.path.join(k, "scripts_door_" + f), os.path.join(MREPO, "scripts/door", f))
        os.remove(os.path.join(MREPO, "scripts/door/lint.sh"))


def edit(root, path, fn):
    p = os.path.join(root, path)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    t = open(p).read() if os.path.exists(p) else ""
    open(p, "w").write(fn(t))


CARGO_A = '[build]\nrustflags = ["-Aclippy::disallowed_methods", "-Aclippy::disallowed_types"]\n'
CARGO_B = '[build]\nrustflags = ["-A", "clippy::disallowed_methods", "-A", "clippy::disallowed_types"]\n'
READ = 'let _ = std::fs::read("/");'

MUTANTS = [
    # name, kind, [(path, edit)], filter-or-env
    ("X1 thread module alias, use + t::spawn", "test",
     [("src/db/zfile.rs", chain(append("use std::thread as t;"), first_in_drop_written("t::spawn(|| {});")))], "door"),
    ("X2 thread glob, spawn and sleep", "test",
     [("src/db/zfile.rs", first_in_drop_written("{ use std::thread::*; spawn(|| {}); sleep(std::time::Duration::ZERO); }"))], "door"),
    ("X3 thread re-export from pager", "test",
     [("src/db/pager.rs", append("pub(crate) use std::thread as pth;")),
      ("src/db/zfile.rs", first_in_drop_written("crate::db::pager::pth::spawn(|| {});"))], "door"),
    ("X5 allow made by a macro", "test",
     [("src/db/zfile.rs", append(
         'macro_rules! quiet { ($l:ident, $($i:item)*) => { $(#[allow(clippy::$l)] $i)* } }\n'
         'quiet!(disallowed_methods, fn door_probe() { let _ = std::fs::read("/"); });'))], "door"),
    ("X11 block use of thread as t", "test",
     [("src/db/zfile.rs", first_in_drop_written("{ use std::thread as t; let _ = t::spawn(|| ()); }"))], "door"),
    ("X12 use std::thread::Builder", "test",
     [("src/db/zfile.rs", chain(append("use std::thread::Builder;"),
                                first_in_drop_written("let _ = Builder::new().spawn(|| ());")))], "door"),
    ("X9a type moved into the methods table (source test)", "test",
     [("scripts/door/clippy.toml", sub("disallowed-methods = [\n", 'disallowed-methods = [\n  "std::fs::OpenOptions",\n'))],
     "each_table"),
    ("X9b function moved into the types table (source test)", "test",
     [("scripts/door/clippy.toml", sub("disallowed-types = [\n", 'disallowed-types = [\n  "std::fs::read",\n'))],
     "each_table"),
    ("M1 silences: metavariable clause gone", "test",
     [("tests/source.rs", sub("                || name.contains('$')\n", ""))], "a_new_allow"),
    ("M2 thread_uses: rename not read", "test",
     [("tests/source.rs", sub('let why = if rest.starts_with(" as ") {', 'let why = if rest.starts_with(" as  ") {'))],
     "a_thread_call"),
    ("M3 thread_uses: glob not read", "test",
     [("tests/source.rs", sub("if r.starts_with('*') {", "if r.starts_with('?') {"))], "a_thread_call"),
    ("M4 THREAD_TYPES holds Builder", "test",
     [("tests/source.rs", sub('    "JoinHandle",\n', '    "JoinHandle",\n    "Builder",\n'))], "a_thread_call"),
    ("M5 self as t not read", "test",
     [("tests/source.rs", sub('} else if name == "self" && n.contains(" as ") {', '} else if name == "self" && n.contains(" as  ") {'))],
     "a_thread_call"),
    ("M6 conf kind compare always equal", "test",
     [("tests/source.rs", sub("if is_fn != wants_fn {", "if is_fn != wants_fn && false {"))], "each_table"),
    ("M7 door_table takes reasons too", "test",
     [("tests/source.rs", sub(".filter(|s| s.contains(\"::\") && !s.contains(' '))", ".filter(|s| s.contains(\"::\"))"))],
     "each_table"),
    ("M8 thread_uses only on non-door files", "test",
     [("tests/source.rs", sub("        for (ln, why) in thread_uses(&lines) {", "        for (ln, why) in thread_uses(&lines).into_iter().filter(|_| !door) {"))],
     "a_thread_call"),
    # scripts, on a copy, current scripts
    ("C0 control: clean copy passes", "script", [], {}),
    ("X4a .cargo/config.toml rustflags -Aclippy::.. + std::fs::read", "script",
     [(".cargo/config.toml", lambda t: CARGO_A), ("src/db/zfile.rs", first_in_drop_written(READ))], {}),
    ("X13 .cargo/config.toml rustflags [-A, clippy::..] + std::fs::read", "script",
     [(".cargo/config.toml", lambda t: CARGO_B), ("src/db/zfile.rs", first_in_drop_written(READ))], {}),
    ("X4b RUSTFLAGS in the environment + std::fs::read", "script",
     [("src/db/zfile.rs", first_in_drop_written(READ))],
     {"RUSTFLAGS": "-Aclippy::disallowed_methods -Aclippy::disallowed_types"}),
    ("X6 barred call inside a macro defined in vault.rs", "script",
     [("src/db/vault.rs", append('macro_rules! door_read { ($p:expr) => { std::fs::read($p) } }\npub(crate) use door_read;')),
      ("src/db/zfile.rs", first_in_drop_written('let _ = crate::db::vault::door_read!("/");'))], {}),
    ("X7 libc::pread64", "script",
     [("src/db/zfile.rs", first_in_drop_written("unsafe { libc::pread64(-1, std::ptr::null_mut(), 0, 0) };"))], {}),
    ("X14a libc::unlinkat", "script",
     [("src/db/zfile.rs", first_in_drop_written('unsafe { libc::unlinkat(libc::AT_FDCWD, c"/x".as_ptr(), 0); }'))], {}),
    ("X14b libc::renameat", "script",
     [("src/db/zfile.rs", first_in_drop_written('unsafe { libc::renameat(libc::AT_FDCWD, c"/x".as_ptr(), libc::AT_FDCWD, c"/y".as_ptr()); }'))], {}),
    ("X1c std::fs::DirBuilder", "script",
     [("src/db/zfile.rs", first_in_drop_written('let _ = std::fs::DirBuilder::new().create("/d");'))], {}),
    ("X1d libc::sync", "script",
     [("src/db/zfile.rs", first_in_drop_written("unsafe { libc::sync() };"))], {}),
    ("X8a CLIPPY_CONF_DIR dropped from the door command", "script",
     [("scripts/door/lint.sh", sub('CLIPPY_CONF_DIR="$2" cargo', 'cargo'))], {}),
    ("X8b --release for --profile checked in the door command", "script",
     [("scripts/door/lint.sh", sub("--profile checked", "--release"))], {}),
    ("X9c function in the types table (run.sh)", "script",
     [("scripts/door/clippy.toml", sub("disallowed-types = [\n", 'disallowed-types = [\n  "std::fs::read",\n'))], {}),
    ("X10 pattern reworded (stands in for a clippy rewording)", "script",
     [("scripts/door/lint.sh", sub("grep -q -e '--> .*clippy\\.toml:[0-9]'", "grep -q -e 'does not refer to an existing'"))], {}),
    ("R1 entry naming nothing in the main config", "script",
     [("scripts/door/clippy.toml", sub("disallowed-methods = [\n", 'disallowed-methods = [\n  "libc::no_such_fn_g12",\n'))], {}),
    ("R2b door_run's config check gone + entry naming nothing", "script",
     [("scripts/door/clippy.toml", sub("disallowed-methods = [\n", 'disallowed-methods = [\n  "libc::no_such_fn_g12",\n')),
      ("scripts/door/lint.sh", sub("if printf '%s\\n' \"$out\" | door_config_diag; then", "if false; then"))], {}),
    ("R2c door_run's check gone, nothing else", "script",
     [("scripts/door/lint.sh", sub("if printf '%s\\n' \"$out\" | door_config_diag; then", "if false; then"))], {}),
    ("R2d door_run returns clippy's status on a config diag", "script",
     [("scripts/door/lint.sh", sub("    return 2\n", "    return \"$status\"\n"))], {}),
    ("R3 red.sh's bogus entry not inserted", "script",
     [("scripts/door/red.sh", sub("""{ print "  \\"libc::no_such_fn_door_red\\","; n++ }""", "{ n++ }"))], {}),
    ("R4b red.sh's config-diag check gone + pattern reworded + entry naming nothing", "script",
     [("scripts/door/red.sh", sub("if ! printf '%s\\n' \"$out\" | grep -qxF \"$DOOR_NAMES_NOTHING\"; then", "if false; then")),
      ("scripts/door/lint.sh", sub("grep -q -e '--> .*clippy\\.toml:[0-9]'", "grep -q -e 'does not refer to an existing'")),
      ("scripts/door/clippy.toml", sub("disallowed-methods = [\n", 'disallowed-methods = [\n  "libc::no_such_fn_g12",\n'))], {}),
    ("R5 red.sh's debug_assertions line gone + --release", "script",
     [("scripts/door/red.sh", sub('''    print "    #[cfg(debug_assertions)] let _ = std::fs::metadata(\\"/\\");"\n''', "")),
      ("scripts/door/red.sh", sub("for k in 1 2 3 4 5 6 7 8; do", "for k in 1 2 3 4 5 6 7; do")),
      ("scripts/door/lint.sh", sub("--profile checked", "--release"))], {}),
    ("R6 red.sh's unlink line gone + libc::unlink entry gone", "script",
     [("scripts/door/red.sh", sub('''    print "    unsafe { libc::unlink(c\\"/\\".as_ptr()) };"\n''', "")),
      ("scripts/door/red.sh", sub("for k in 1 2 3 4 5 6 7 8; do", "for k in 1 2 3 4 5 6 7; do")),
      ("scripts/door/clippy.toml", sub('"libc::unlink", ', ""))], {}),
    ("R6b libc::unlink entry gone alone", "script",
     [("scripts/door/clippy.toml", sub('"libc::unlink", ', ""))], {}),
    ("R7 red copy outside the repo again + .cargo rustflags + std::fs::read", "script",
     [("scripts/door/run.sh", sub('red="$(mktemp -d "$PWD/target/door/red.XXXXXX")"', 'red="$(mktemp -d "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-2/red.XXXXXX")"')),
      (".cargo/config.toml", lambda t: CARGO_A), ("src/db/zfile.rs", first_in_drop_written(READ))], {}),
    # before: round 0's scripts and config
    ("B0 control: round 0 scripts, clean copy", "before", [], {}),
    ("B1 round 0: .cargo rustflags + std::fs::read", "before",
     [(".cargo/config.toml", lambda t: CARGO_A), ("src/db/zfile.rs", first_in_drop_written(READ))], {}),
    ("B2 round 0: libc::unlinkat", "before",
     [("src/db/zfile.rs", first_in_drop_written('unsafe { libc::unlinkat(libc::AT_FDCWD, c"/x".as_ptr(), 0); }'))], {}),
    ("B3 round 0: libc::pread64", "before",
     [("src/db/zfile.rs", first_in_drop_written("unsafe { libc::pread64(-1, std::ptr::null_mut(), 0, 0) };"))], {}),
    ("B4 round 0: function in the types table", "before",
     [("scripts/door/clippy.toml", sub("disallowed-types = [\n", 'disallowed-types = [\n  "std::fs::read",\n'))], {}),
    ("B5 round 0: pattern reworded + entry naming nothing", "before",
     [("scripts/door/run.sh", sub("grep -q 'does not refer to'", "grep -q 'does not refer to an existing'")),
      ("scripts/door/clippy.toml", sub("disallowed-methods = [\n", 'disallowed-methods = [\n  "libc::no_such_fn_g12",\n'))], {}),
]


def main():
    only = sys.argv[1:]
    done = set()
    if os.path.exists(RES):
        done = {l.split("\t")[0] for l in open(RES)}
    for name, kind, edits, extra in MUTANTS:
        if name in done or (only and not any(name.startswith(o) for o in only)):
            continue
        log(f"=== {name} {time.strftime('%T')}")
        if kind == "test":
            try:
                for path, fn in edits:
                    edit(R, path, fn)
                p = run(f"cargo test --profile checked --test source -- {extra}", R)
            finally:
                restore_tree()
            fails = [l for l in p.stdout.splitlines() if l.endswith("FAILED") and l.startswith("test ")]
            ran = "test result:" in p.stdout
            if not ran:
                verdict = "INVALID (did not compile)"
            else:
                verdict = "KILLED" if p.returncode != 0 and fails else "SURVIVED"
            msgs = [l.strip() for l in p.stdout.splitlines()
                    if "cannot follow" in l or "the door grew" in l or " is a " in l and "disallowed-" in l]
            result(name, verdict, "; ".join(fails) + (" | " + msgs[0][:200] if msgs else ""))
        else:
            make_mrepo(kind == "before")
            for path, fn in edits:
                edit(MREPO, path, fn)
            p = run("sh scripts/door/run.sh", MREPO, extra)
            tail = [l for l in (p.stdout + p.stderr).splitlines() if l.startswith("door")]
            how = " | ".join(tail[-2:])
            if kind == "before" or name.startswith("C0"):
                verdict = ("hole (run.sh exit 0)" if p.returncode == 0 else f"caught (exit {p.returncode})")
            else:
                verdict = "KILLED" if p.returncode != 0 else "SURVIVED"
            result(name, verdict, how)
    log("EXIT=0 " + time.strftime("%T"))


if __name__ == "__main__":
    main()
