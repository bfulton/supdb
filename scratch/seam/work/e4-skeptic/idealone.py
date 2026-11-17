import sys, ast
sys.path.insert(0, '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e4-skeptic')
import idealhdr_lib
import one
o = ast.literal_eval(sys.argv[1]); ops = ast.literal_eval(sys.argv[2])
one.run(o, ops, int(sys.argv[3]) if len(sys.argv) > 3 else 1)
