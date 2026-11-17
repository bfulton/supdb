import itertools, replace
from pfm import Tally
res = {}
for mode in ('plain', 'pg'):
    o = dict(replace.OPT)
    bad = set()
    for n in range(1, 5):
        for ops in itertools.product('WUOBEXP', repeat=n):
            if ops.count('P') > 1: continue
            T = Tally('x')
            replace.run(mode, o, ops, T)
            if sum(T.bad.values()): bad.add(ops)
    res[mode] = bad
    print(mode, 'failing sequences', len(bad))
print('pg only', sorted(res['pg'] - res['plain'])[:10], len(res['pg'] - res['plain']))
print('plain only', sorted(res['plain'] - res['pg'])[:10], len(res['plain'] - res['pg']))
