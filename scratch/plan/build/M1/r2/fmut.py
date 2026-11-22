# usage: fmut.py <source fuzz.rs> <edit>... -> tests/fuzz.rs
import sys
s=open(sys.argv[1]).read()
E={
 # the race the stride group lost, made certain: the writer's timed asks never taken
 'noasks': ("            db.zpages_convert_ask(step / 53);\n", ""),
 # FM1: the fix's settled asks taken out
 'fm1': ("    if convert {\n        // The asks above are taken", "    if false {\n        // The asks above are taken"),
 # FM2: the loop's test inverted
 'fm2': ("        while db.zpage_counts().stride_converts_back == back {", "        while db.zpage_counts().stride_converts_back != back {"),
 # FM3: the asks not settled
 'fm3': ("            db.zpages_convert_ask(nth);\n            db.zpages_settle()\n                .map_err(|e| format!(\"zpages settle: {e}\"))?;\n", "            db.zpages_convert_ask(nth);\n"),
}
for n in sys.argv[2:]:
    a,b=E[n]; assert s.count(a)==1, n; s=s.replace(a,b)
open('/home/user/supdb/tests/fuzz.rs','w').write(s)
