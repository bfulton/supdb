# Round 3's mutants. Each: (id, description, [(file, old, new)], [commands]).
# Applied one at a time; every command run in order, each status kept; the
# verdict is the LAST command's status (nonzero: killed). Files put back
# from the copy taken before the edit and checked by sha256 against it.
import subprocess, sys, hashlib, os
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = "/home/user/supdb"
RES = os.path.join(HERE, "results.txt")
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-7"
ENV = dict(os.environ, TMPDIR=TMP)
SRC = "cargo test --profile checked --test source -- seal"
CLIPPY = "cargo clippy --profile checked --lib -- -D warnings"
ORD = "use std::path::Path;\n"
ZF = "use std::sync::Arc;\n"
BL = "/// A section of the file: lent by the source"
DB = "use std::cell::UnsafeCell;\n"
BODY = '        let body = b"the fence key-00750 and a value nobody may read".repeat(300);\n'
PAGED = "            ptr: pager.base(Base(())),\n            lend: 0,\n"
M = [
 ("X-macro-hand", "ordindex hands a token out through ty!/mk! macros; db.rs spends it through call!(m, lent, ..)",
  [("src/ordindex.rs", ORD, ORD + "macro_rules! ty {\n    ($m:ident) => {\n        $m::Token\n    };\n}\nmacro_rules! mk {\n    ($m:ident) => {\n        $m::Token::new()\n    };\n}\n#[allow(dead_code)]\npub(crate) fn hand() -> ty!(lend) {\n    mk!(lend)\n}\n"),
   ("src/db.rs", DB, DB + "#[allow(dead_code)]\nfn sneak(m: &crate::bytes::MmapBytes) -> usize {\n    macro_rules! call {\n        ($b:expr, $m:ident, $t:expr) => {\n            $b.$m($t)\n        };\n    }\n    call!(m, lent, crate::ordindex::hand()).len()\n}\n")],
  [CLIPPY, SRC]),
 ("X-macro-home", "zfile: an unlisted whole reader ZFile::sneak through call!(self.map, lent, mk!(lend))",
  [("src/db/zfile.rs", ZF, ZF + "impl ZFile {\n    #[allow(dead_code)]\n    fn sneak(&self) -> &[u8] {\n        macro_rules! call {\n            ($b:expr, $m:ident, $t:expr) => {\n                $b.$m($t)\n            };\n        }\n        macro_rules! mk {\n            ($m:ident) => {\n                $m::Token::new()\n            };\n        }\n        call!(self.map, lent, mk!(lend))\n    }\n}\n")],
  [CLIPPY, SRC]),
 ("X-macro-blob", "control: blob hands its token out through ty!/mk!; zfile spends it through call!(src, lend_at, ..)",
  [("src/blob.rs", BL, "macro_rules! ty {\n    ($m:ident) => {\n        $m::Token\n    };\n}\nmacro_rules! mk {\n    ($m:ident) => {\n        $m::Token::new()\n    };\n}\n#[allow(dead_code)]\npub(crate) fn hand() -> ty!(lend) {\n    mk!(lend)\n}\n\n" + BL),
   ("src/db/zfile.rs", ZF, ZF + "#[allow(dead_code)]\nfn sneak(src: &MmapBytes) -> Option<&[u8]> {\n    macro_rules! call {\n        ($b:expr, $m:ident, $o:expr, $l:expr, $t:expr) => {\n            $b.$m($o, $l, $t)\n        };\n    }\n    call!(src, lend_at, 0, 1, crate::blob::hand())\n}\n")],
  [CLIPPY, SRC]),
 ("X-macro-use", "mine: ordindex: a macro_rules body imports the token module as mk on one line; hand() -> mk::Token",
  [("src/ordindex.rs", ORD, ORD + "macro_rules! u {\n    () => { use self::lend as mk; };\n}\nu!();\n#[allow(dead_code)]\npub(crate) fn hand() -> mk::Token {\n    mk::Token::new()\n}\n")],
  [CLIPPY, SRC]),
 ("V4", "outside_sources: examples/ not read",
  [("tests/source.rs", '    for d in ["tests", "examples", "bench"] {', '    for d in ["tests", "bench"] {')], [SRC]),
 ("V5", "seal_names base arm: (path && post != \"()\") -> path",
  [("tests/source.rs", '                        || (path && post != "()")) =>', '                        || path) =>')], [SRC]),
 ("V6", "opens_use: the pub(..) use disjunct dropped",
  [("tests/source.rs", '\n                || (ft.starts_with("pub(") && ft.contains(") use "));', ';')], [SRC]),
 ("V7", "seal_names: dot without the range exclusion",
  [("tests/source.rs", "        let dot = pre.ends_with('.') && !pre.ends_with(\"..\");", "        let dot = pre.ends_with('.');")], [SRC]),
 ("V8", "step 3: the whole::Reader disjunct dropped",
  [("tests/source.rs", '                || t.contains("whole::Reader")\n', '')], [SRC]),
 ("R1", "mine: SEAL_MACRO_WORDS matched by file and text, not scope",
  [("tests/source.rs", "!SEAL_MACRO_WORDS.contains(&(f.as_str(), l.scope.as_str(), t.as_str()))", "!SEAL_MACRO_WORDS.iter().any(|(a, _, c)| *a == f.as_str() && *c == t.as_str())")], [SRC]),
 ("R2", "mine: seal_in_macro: macro_rules' name not skipped (definitions not read)",
  [("tests/source.rs", '        if name == "macro_rules" {\n            while j < c.len() && lex::is_word(c[j]) {\n                j += 1;\n            }\n            j = skip(j);\n        }\n', '')], [SRC]),
 ("R3", "mine: seal_in_macro: only ( opens a macro",
  [("tests/source.rs", "matches!(c[j], '(' | '[' | '{')", "matches!(c[j], '(')")], [SRC]),
 ("R4", "mine: seal_in_macro: a macro never closes",
  [("tests/source.rs", "                    open.pop();\n", "")], [SRC]),
 ("R5", "mine: seal_in_macro: keywords read as macro names",
  [("tests/source.rs", "        if name.is_empty() || KEYWORDS.contains(&name.as_str()) {", "        if name.is_empty() {")], [SRC]),
 ("R6", "mine: inside a macro only lent refused, not lend",
  [("tests/source.rs", '(w == "lent" || w == "lend") && inside[*i]', 'w == "lent" && inside[*i]')], [SRC]),
]
only = set(sys.argv[1:])
def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()
for mid, desc, edits, cmds in M:
    if only and mid not in only:
        continue
    files = sorted({f for f, _, _ in edits})
    orig = {f: open(os.path.join(ROOT, f), "rb").read() for f in files}
    sums = {f: hashlib.sha256(orig[f]).hexdigest() for f in files}
    ok = True
    for f, old, new in edits:
        p = os.path.join(ROOT, f)
        t = open(p).read()
        if t.count(old) != 1:
            print(mid, "edit does not apply once:", f, t.count(old)); ok = False; break
        open(p, "w").write(t.replace(old, new, 1))
    statuses = []
    log = os.path.join(HERE, "mut", mid + ".log")
    if ok:
        with open(log, "w") as lf:
            for c in cmds:
                lf.write("$ " + c + "\n"); lf.flush()
                r = subprocess.run(c, shell=True, cwd=ROOT, env=ENV, stdout=lf, stderr=subprocess.STDOUT)
                lf.write("STATUS %d\n" % r.returncode); lf.flush()
                statuses.append(r.returncode)
    for f in files:
        open(os.path.join(ROOT, f), "wb").write(orig[f])
        assert sha(os.path.join(ROOT, f)) == sums[f], f
    verdict = "NOT-APPLIED" if not ok else ("killed" if statuses[-1] != 0 else "SURVIVED")
    with open(RES, "a") as r:
        r.write("%s\t%s\t%s\t%s\n" % (mid, verdict, statuses, desc))
    print(mid, verdict, statuses, flush=True)
print("EXIT-RUNNER")
