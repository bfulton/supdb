# The bounds flake made wide: the upkeep thread held before its pass in the
# store with the option on, across the commit of round 0 and the seal, so the
# seal freezes the burst before the thread's fill walks its snapshot: the race
# the red run of after-r1.log lost, made to lose every run.
import sys
src, dst = sys.argv[1], sys.argv[2]
s = open(src).read()
a = "            write(&mut db, &mut want, 0);\n            let before = db.zpage_counts();\n            db.seal().unwrap();\n"
assert s.count(a) == 1, "anchor"
b = ("            if on {\n                db.hold_upkeep_pass(true);\n            }\n"
     "            write(&mut db, &mut want, 0);\n            let before = db.zpage_counts();\n            db.seal().unwrap();\n"
     "            db.hold_upkeep_pass(false);\n")
s = s.replace(a, b)
open(dst, "w").write(s)
