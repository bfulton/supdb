# Apply named edits to keep/<file> and write it into the tree. usage: tmut.py <file> <edit>...
import sys
K='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1/keep/'
f=sys.argv[1]; s=open(K+f).read()
E={
 # R1 (extra 1): `before` from just before round 0's commit (above the burst's puts; the commit is the burst's last call), Inline kept
 'r1': ("            write(&mut db, &mut want, 0);\n            let before = db.zpage_counts();\n",
        "            let before = db.zpage_counts();\n            write(&mut db, &mut want, 0);\n"),
 # R9 (extra 9): `before` moved to just after the seal
 'r9': ("            let before = db.zpage_counts();\n            db.seal().unwrap();\n",
        "            db.seal().unwrap();\n            let before = db.zpage_counts();\n"),
 # the writer's timed asks taken out (round 2's noasks)
 'noasks': ("            db.zpages_convert_ask(step / 53);\n", ""),
 # R2/R5: always the first served partition
 'ask0': ("            db.zpages_convert_ask(nth);\n", "            db.zpages_convert_ask(0);\n"),
 # R6: the loop's bound 64 -> 1
 'bound1': ("            if nth == 64 {\n", "            if nth == 1 {\n"),
 # instrument: print the asks a seed took (not a mutant)
 'count': ("            nth += 1;\n        }\n    }\n", "            nth += 1;\n        }\n        eprintln!(\"ASKS seed {seed}: {nth}\");\n    }\n"),
 # FA: two asks, not three
 'fa': ("        for _ in 0..3 {\n            db.zpages_convert_ask(0);\n", "        for _ in 0..2 {\n            db.zpages_convert_ask(0);\n"),
 # FB: the asks not settled
 'fb': ("            db.zpages_convert_ask(0);\n            db.zpages_settle()\n                .map_err(|e| format!(\"zpages settle: {e}\"))?;\n        }\n", "            db.zpages_convert_ask(0);\n        }\n"),
 # FC: the check takes only moves all one way
 'fc': ("moved.0.abs_diff(moved.1) != 1 {", "moved.0.abs_diff(moved.1) != 3 {"),
 # FD (engine, src/db.rs): a block file asked never goes back into the stride form
 'fd': ("        if stride || zfile::StrideNo::askable(seg.z_stride_no.load(AtomicOrdering::Relaxed)) {\n", "        if stride {\n"),
 # the bounds test as it now stands: before taken back after the commit (round 2's narrowing)
 'narrow': ("            let before = db.zpage_counts();\n            write(&mut db, &mut want, 0);\n", "            write(&mut db, &mut want, 0);\n            let before = db.zpage_counts();\n"),
 # R9 against the test as it now stands: before after the seal
 'r9n': ("            let before = db.zpage_counts();\n            write(&mut db, &mut want, 0);\n            db.seal().unwrap();\n", "            write(&mut db, &mut want, 0);\n            db.seal().unwrap();\n            let before = db.zpage_counts();\n"),
 # the fill made to differ between the stores (round 2's filldiff stand-in)
 'filldiff': ("                    upkeep: supdb::Upkeep::Inline,\n", "                    upkeep: supdb::Upkeep::Inline,\n                    commit_forms_build: !on,\n"),
 # FE: the settled asks taken out whole
 'fe': ("    if convert {\n        // The asks above are taken", "    if false {\n        // The asks above are taken"),
}
for n in sys.argv[2:]:
    a,b=E[n]; assert s.count(a)==1, n; s=s.replace(a,b)
open('/home/user/supdb/'+f,'w').write(s)
