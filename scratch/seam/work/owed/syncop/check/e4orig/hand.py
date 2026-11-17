import sys
from logrun import Runner
o = {'R': int(sys.argv[1])}
r = Runner(o, 'e4orig hand R=%s' % sys.argv[1])
for t in sys.argv[2:]:
    r.run(tuple(int(x) if x.isdigit() else x for x in t.split('_')))
r.T.report()
