import sys
p=sys.argv[1]
s=open(p).read()
old="""        // reached by nothing.)
"""
new="""        // reached by nothing.)
        //
        // The count starts with the builder at rest. A plain settle joins
        // no form build and takes no ask, so with the builder free at the
        // loop's end and its last ask not yet taken, the segment work's
        // turn behind the settle above started that conversion, it landed
        // inside the first settled ask, and three asks read as four moves
        // -- (1, 3), once in a gate's stride group and in every run with
        // that state built. The form's own settle takes what was asked and
        // waits for it.
        db.zpages_settle()
            .map_err(|e| format!("zpages settle: {e}"))?;
"""
assert s.count(old)==1, s.count(old)
open(p,'w').write(s.replace(old,new))
