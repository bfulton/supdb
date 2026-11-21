import sys, os
# gate.py OUT TEST...: each TEST's call shapes in OUT (one run) against the
# three base runs (before1..3): a shape no base run made (NEW) or one every
# base run made and this run did not (GONE) is red. Exit 1 on any, else 0.
d = os.path.dirname(os.path.abspath(__file__))
out = sys.argv[1]
def load(f):
    r = {}
    for line in open(f):
        n, *k = line.split(' ')
        r[' '.join(k).strip()] = int(n)
    return r
bad = 0
for t in sys.argv[2:]:
    bs = [load(f'{d}/before{i}/{t}.norm') for i in (1, 2, 3)]
    a = load(f'{out}/{t}.norm')
    bu = set().union(*bs); bi = set.intersection(*[set(b) for b in bs])
    if not a:
        print(f'{t}: no calls traced'); bad += 1
    for k in sorted(set(a) - bu): print(t, 'NEW', k, a[k]); bad += 1
    for k in sorted(bi - set(a)): print(t, 'GONE', k); bad += 1
print('VERDICT', 'same' if bad == 0 else f'{bad} differences')
sys.exit(1 if bad else 0)
