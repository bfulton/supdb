"""Does the slot-index binding change WHICH state the open can be made to
take?  For every history (adversary.py's), the set of records the open
takes over all directories, with and without the index in the AAD; and the
set of (record taken, ROLLBACK?) outcomes."""
import itertools, sys
import adversary, slots
from slots import choose, OPT
maxlen = int(sys.argv[1]) if len(sys.argv) > 1 else 4
diff = same = 0
extra_refusals = 0
for noidx in (0,):
    pass
o0 = dict(OPT); o1 = dict(OPT, noidx=1)
for (t, W, pairs, orc) in adversary.histories(o0, maxlen):
    recs = sorted(W.pub)
    choices = ['EMPTY', 'GARB'] + recs
    took = {0: set(), 1: set()}
    for o, k in ((o0, 0), (o1, 1)):
        for a, b in itertools.product(choices, repeat=2):
            v = dict(adversary.place(W, a, 0, o)); v.update(adversary.place(W, b, 1, o))
            verdict, taken, reads = choose(W, v, o, o['NSEC'], o['SU'])
            took[k].add(taken[1][1] if verdict == 'OK' else verdict)
    # with the binding, the open can be made to take a subset of what it can without
    if took[0] - took[1]:
        print('binding adds an outcome?', t, took[0] - took[1]); sys.exit(1)
    if {x for x in took[0] if isinstance(x, int)} == {x for x in took[1] if isinstance(x, int)}:
        same += 1
    else:
        diff += 1
print('histories where the set of records the open can be made to take differs with the index bound:', diff, 'same:', same)
