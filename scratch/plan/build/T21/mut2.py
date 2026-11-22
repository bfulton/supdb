#!/usr/bin/env python3
# The mutant runner: each mutant an edit of tests/source.rs, run against the
# seal's four tests, the verdict written as it comes, the file put back by
# its sha256 after each.
import hashlib, subprocess, sys, os
ROOT = "/home/user/supdb"
D = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T21"
F = ROOT + "/tests/source.rs"
KEEP = D + "/keep/final/tests/source.rs"
RES = D + "/mut-results-r1.txt"
want = [l.split()[0] for l in open(D + "/keep/final.sha256") if l.strip().endswith("./tests/source.rs")][0]

def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()

def restore():
    open(F, "wb").write(open(KEEP, "rb").read())
    assert sha(F) == want, "restore failed"

M = [
    ("M1 a word left out of SEAL_WORDS (OrdLendToken)",
     '    "OrdLendToken",\n    "StrideLendToken",\n    "BaseAddressToken",',
     '    "StrideLendToken",\n    "BaseAddressToken",'),
    ("M2 a scope removed from the allowed sites (Sec::get)",
     "src/blob.rs     | Sec::lend Sec::get", "src/blob.rs     | Sec::lend"),
    ("M3 the allowed sites keyed by file alone",
     "r[3].split_whitespace().any(|s| s == l.scope)", "true"),
    ("M3b the declarations keyed by file alone",
     "r[0] == f && r[1] == line(i).scope && r[2]", "r[0] == f && r[2]"),
    ("M4 the constructor's at-its-call rule dropped",
     "if made.len() != 1 || made[0].trim_end_matches(',') != token {", "if false {"),
    ("M5 the readers' trait given to every type (the impl check takes any header)",
     ".any(|r| r[0] == f && r[1] == head)", ".any(|r| r[0] == f)"),
    ("M6 (own) a use's bound name not held",
     "|| !SEAL_BOUND.contains(&bound)", ""),
    ("M7 (own) a use's group not refused",
     "if head.contains(['{', '*'])", "if head.contains('*')"),
    ("M8 (own) a call in test code at a listed scope's name",
     "if l.test || !rows.iter().any(|r| home(&r)) {", "if !rows.iter().any(|r| home(&r)) {"),
    ("M9 (own) a call word with no parentheses let through",
     "say(*i, format!(\"names `{w}` without calling it\"));\n                continue;",
     "continue;"),
    ("M11 (own) the help left off the failure",
     'Err(format!("{}\\n{SEAL_HELP}", errs.join("\\n")))', 'Err(errs.join("\\n"))'),
    ("M12 (own) the token module's shape not held",
     "Ok(got) if got == want => {}", "Ok(_) => {}"),
    ("E3 (review) a definition of a call word let through anywhere",
     "            if k > 0 && ws[k - 1].1 == \"fn\" {\n                if !decl(*i, w) {\n                    say(*i, outside(w));\n                }\n                continue;",
     "            if k > 0 && ws[k - 1].1 == \"fn\" {\n                continue;"),
    ("E4 (review) the token compared without trimming its trailing comma",
     "made[0].trim_end_matches(',') != token", "made[0] != token"),
    ("N1 (round 1) a declaration's word let stand outside its use, impl or signature",
     "                    && !items.iter().any(|(a, b)| a <= i && i < b)\n", ""),
    ("N2 (round 1) a listed scope's definitions not counted",
     "                if n != 1 {", "                if n > 1 && false {"),
    ("N3 (round 1) a use is no item a declaration may name a word in",
     '                "use" => seal_until(&c, *i, &[\';\']).1,\n', ""),
    ("N4 (round 1) any fn's signature, not only a call's, is such an item",
     '"fn" if calls.iter().any(|r| r[0] == next) =>', '"fn" =>'),
    ("N5 (round 1) a module or struct scope not taken as held whole",
     '                } else if !s.starts_with("mod ")\n                    && !s.starts_with("struct ")\n', '                } else if true\n'),
    ("N1b (round 1) a declaration's word let stand anywhere in its scope",
     "&& !items.iter().any(|(a, b)| a <= i && i < b)", "&& false"),
]

only = sys.argv[1:]
for name, old, new in M:
    if only and not any(name.startswith(o + " ") for o in only):
        continue
    restore()
    s = open(F).read()
    n = s.count(old)
    if n != 1:
        open(RES, "a").write(f"{name}: NOT APPLIED ({n} matches)\n")
        continue
    open(F, "w").write(s.replace(old, new))
    r = subprocess.run(["cargo", "test", "--profile", "checked", "--test", "source", "seal"],
                       cwd=ROOT, capture_output=True, text=True)
    out = r.stdout + r.stderr
    failed = [l for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
    build = "error[" in out or "error:" in out and "could not compile" in out
    verdict = "KILLED" if r.returncode != 0 else "SURVIVED"
    detail = "; ".join(failed) if failed else ("build failed" if build else "")
    open(RES, "a").write(f"{name}: {verdict} (exit {r.returncode}) {detail}\n")
    open(D + "/mut-r1-" + name.split()[0] + ".log", "w").write(out)
    restore()
restore()
open(RES, "a").write("DONE\n")
