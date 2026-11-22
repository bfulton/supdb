#!/usr/bin/env python3
# Round 2's mutant runner: argv[1] the kept copy to mutate and put back
# (checked by sha256), argv[2] the results file, argv[3] the set name.
import hashlib, subprocess, sys, os
ROOT = "/home/user/supdb"
D = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T21"
F = ROOT + "/tests/source.rs"
KEEP, RES, SET = sys.argv[1], sys.argv[2], sys.argv[3]
def sha(p): return hashlib.sha256(open(p, "rb").read()).hexdigest()
want = sha(KEEP)
def restore():
    open(F, "wb").write(open(KEEP, "rb").read())
    assert sha(F) == want, "restore failed"
SETS = {
 "mid": [
  ("X7 the defs count's `>{next}` arm dropped",
   '\n                    || s.ends_with(&format!(">{next}"))', ''),
  ("X8 `made.len() != 1` made `made.is_empty()`",
   "if made.len() != 1 ||", "if made.is_empty() ||"),
 ],
 "final": [
  ("X9 the item spans' impl arm stops at `{` only",
   "\"impl\" => seal_until(&c, *i, &['{', ';']).1,", "\"impl\" => seal_until(&c, *i, &['{']).1,"),
  ("X9b the impl heads stop at `{` only",
   "\"impl\" => seal_until(&c, *i, &['{', ';']),", "\"impl\" => seal_until(&c, *i, &['{']),"),
  ("X10 the `struct ` exemption dropped",
   "&& !s.starts_with(\"struct \")\n", "\n"),
  ("R1 seal_until counts parentheses only",
   "'(' | '[' => depth += 1,\n            ')' | ']' => depth -= 1,", "'(' => depth += 1,\n            ')' => depth -= 1,"),
  ("R2 the token taken from the word's first row, not its home row",
   "let Some(token) = home.filter(|_| !l.test).map(|r| r[1]) else {",
   "let Some(token) = home.filter(|_| !l.test).and(calls.iter().find(|r| r[0] == w)).map(|r| r[1]) else {"),
  ("R2b the token taken from the word's last row",
   "let Some(token) = home.filter(|_| !l.test).map(|r| r[1]) else {",
   "let Some(token) = home.filter(|_| !l.test).and(calls.iter().rfind(|r| r[0] == w)).map(|r| r[1]) else {"),
  ("R3 the address token's attributes: any `#[cfg(` taken",
   "if !l.is_empty() && l != \"#[cfg(not(target_family = \\\"wasm\\\"))]\" {",
   "if !l.is_empty() && !l.starts_with(\"#[cfg(\") {"),
  ("R4 a declaration's word held by its line containing `use `/`impl `",
   "&& !items.iter().any(|(a, b)| a <= i && i < b)",
   "&& !(line(*i).code.contains(\"use \") || line(*i).code.contains(\"impl \"))"),
  ("O1 (own) a call's home row of any call word",
   ".find(|r| r[0] == w && r[2] == f &&", ".find(|r| r[2] == f &&"),
  ("O2 (own) the token read from the last word of the seal inside the call",
   ".find(|(p, x)| *p > j && *p < close && SEAL_WORDS.contains(&x.as_str()))",
   ".filter(|(p, x)| *p > j && *p < close && SEAL_WORDS.contains(&x.as_str())).last()"),
  ("X6 (review) a closure in a listed scope (probe: SURVIVED means the check passes it)",
   '    assert!(refused.is_empty(), "{}", refused.join("\\n"));\n}',
   '    green("x6", &seal_edit(&src, "src/db/zfile.rs", "    fn stride_bytes(&self) -> &[u8] {\\n        self.map.lend_whole_map(lend_seal::Token::new())\\n    }", "    fn stride_bytes(&self) -> &[u8] {\\n        let w = |m: &MmapBytes| -> &[u8] { m.lend_whole_map(lend_seal::Token::new()) };\\n        let _ = w as fn(&MmapBytes) -> &[u8];\\n        self.map.lend_whole_map(lend_seal::Token::new())\\n    }"));\n    assert!(refused.is_empty(), "{}", refused.join("\\n"));\n}'),
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
   "if made.as_deref().map(|m| m.trim_end_matches(',')) != Some(token) {", "if false {"),
  ("M5 the readers' trait given to every type (the impl check takes any header)",
   ".any(|r| r[0] == f && r[1] == head)", ".any(|r| r[0] == f)"),
  ("M6 (own r0) a use's bound name not held", "|| !SEAL_BOUND.contains(&bound)", ""),
  ("M7 (own r0) a use's group not refused", "if head.contains(['{', '*'])", "if head.contains('*')"),
  ("M9 (own r0) a call word with no parentheses let through",
   "say(*i, format!(\"names `{w}` without calling it\"));\n                continue;", "continue;"),
  ("M11 (own r0) the help left off the failure",
   'Err(format!("{}\\n{SEAL_HELP}", errs.join("\\n")))', 'Err(errs.join("\\n"))'),
  ("M12 (own r0) the token module's shape not held", "Ok(got) if got == want => {}", "Ok(_) => {}"),
  ("E3 (r1) a definition of a call word let through anywhere",
   "                if !decl(*i, w) {\n                    say(*i, outside(w));\n                }\n                continue;",
   "                continue;"),
  ("E4 (r1) the token compared without trimming its trailing comma",
   "made.as_deref().map(|m| m.trim_end_matches(','))", "made.as_deref()"),
  ("N1 (r1) a declaration's word let stand outside its use, impl or signature",
   "                    && !items.iter().any(|(a, b)| a <= i && i < b)\n", ""),
  ("N2 (r1) a listed scope's definitions not counted", "                if n != 1 {", "                if n > 1 && false {"),
  ("N3 (r1) a use is no item a declaration may name a word in",
   '                "use" => seal_until(&c, *i, &[\';\']).1,\n', ""),
  ("N4 (r1) any fn's signature, not only a call's, is such an item",
   '"fn" if calls.iter().any(|r| r[0] == next) =>', '"fn" =>'),
  ("N5 (r1) a module or struct scope not taken as held whole",
   '                } else if !s.starts_with("mod ")\n                    && !s.starts_with("struct ")\n', '                } else if true\n'),
  ("N1b (r1) a declaration's word let stand anywhere in its scope",
   "&& !items.iter().any(|(a, b)| a <= i && i < b)", "&& false"),
  ("O3 (own) a test line at a home row let through",
   "home.filter(|_| !l.test)", "home"),
 ],
}
restore()
for name, old, new in SETS[SET]:
    t = open(KEEP).read()
    if t.count(old) != 1:
        open(RES, "a").write(f"{name}: NOT APPLIED (matches {t.count(old)})\n"); continue
    open(F, "w").write(t.replace(old, new))
    tag = name.split()[0]
    log = f"{D}/mut-r2-{tag}.log"
    with open(log, "w") as lf:
        r = subprocess.run(["cargo", "test", "--profile", "checked", "--test", "source", "seal"],
                           cwd=ROOT, stdout=lf, stderr=subprocess.STDOUT)
    out = open(log).read()
    failed = [l.strip() for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
    built = "test result:" in out
    v = "KILLED" if (r.returncode != 0 and failed) else ("NO BUILD" if not built else "SURVIVED")
    open(RES, "a").write(f"{name}: {v} (exit {r.returncode}) {'; '.join(failed)}\n")
    restore()
open(RES, "a").write(f"DONE {SET}\n")
