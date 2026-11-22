# Apply named edits to keep/tests/db.rs and write tests/db.rs.
import sys
K='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/M1/keep/tests/db.rs'
s=open(K).read()
E={
 # the race made wide (bounds-wide.py's edit)
 'wide': ("            write(&mut db, &mut want, 0);\n            let before = db.zpage_counts();\n            db.seal().unwrap();\n",
          "            if on {\n                db.hold_upkeep_pass(true);\n            }\n            write(&mut db, &mut want, 0);\n            let before = db.zpage_counts();\n            db.seal().unwrap();\n            db.hold_upkeep_pass(false);\n"),
 # the commit's fill differs between the two stores (a stand-in for the fills' room closed in one)
 'filldiff': ("                    upkeep: supdb::Upkeep::Inline,\n",
              "                    upkeep: supdb::Upkeep::Inline,\n                    commit_forms_build: !on,\n"),
 # BM1: the fix's inline upkeep taken out
 'bm1': ("                    upkeep: supdb::Upkeep::Inline,\n", ""),
 # BM2: the fix's deltas taken out
 'bm2': ("                c.bounds_by_head - before.bounds_by_head,\n                c.bounds_records - before.bounds_records,\n",
         "                c.bounds_by_head,\n                c.bounds_records,\n"),
 # BM3: the deltas taken from the counts before the commit's fill (before moved above the burst)
 'bm3': ("            write(&mut db, &mut want, 0);\n            let before = db.zpage_counts();\n",
         "            let before = db.zpage_counts();\n            write(&mut db, &mut want, 0);\n"),
}
for n in sys.argv[1:]:
    a,b=E[n]
    # 'wide' and 'bm3' share an anchor: apply against the current text
    assert s.count(a)==1, n
    s=s.replace(a,b)
open('/home/user/supdb/tests/db.rs','w').write(s)
