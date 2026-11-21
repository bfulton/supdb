import sys, os, glob
# compare.py DIR: the three base runs (before1..3) against the three after
# runs (after1..3), test by test. A call shape (the normalized key) the
# after runs make and no base run made is NEW; one every base run made and
# no after run made is GONE. Counts are printed as the range over the runs.
d = sys.argv[1]
def load(f):
    out = {}
    for line in open(f):
        n, *k = line.split(' ')
        out[' '.join(k).strip()] = int(n)
    return out
tests = sorted(os.path.basename(f)[:-5] for f in glob.glob(d + '/before1/*.norm'))
bad = 0
for t in tests:
    bs = [load(f'{d}/before{i}/{t}.norm') for i in (1, 2, 3)]
    a_s = [load(f'{d}/after{i}/{t}.norm') for i in (1, 2, 3)]
    bu = set().union(*bs); au = set().union(*a_s)
    bi = set.intersection(*[set(b) for b in bs])
    new = sorted(au - bu); gone = sorted(bi - au)
    print(f'== {t}: base shapes {len(bu)} (in all {len(bi)}), after {len(au)}; new {len(new)}, gone {len(gone)}')
    for k in new: print('  NEW', k, [a.get(k, 0) for a in a_s]); bad += 1
    for k in gone: print('  GONE', k, [b.get(k, 0) for b in bs]); bad += 1
    for k in sorted(bi & set.intersection(*[set(a) for a in a_s])):
        br = [b[k] for b in bs]; ar = [a[k] for a in a_s]
        if max(ar) > max(br) or min(ar) < min(br):
            print('  RANGE', k, 'base', br, 'after', ar)
print('VERDICT', 'same' if bad == 0 else f'{bad} differences')
